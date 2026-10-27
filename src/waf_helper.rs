use evdev::{Device, InputEventKind, Key};
use log::{error, info, warn};
use std::fs;
use std::cmp::Ordering;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const BIND_ADDR: &str = "127.0.0.1:8322";
const INPUT_DIR: &str = "/dev/input";
const INITCTL: &str = "/sbin/initctl";
const SERVICE: &str = "kindle-button-mapper";

// (kind, id, label) — kind matches the tab in the WAF action picker and
// selects the script the binding is written against. "auto" routes at press
// time: KOReader when it is up, the native reader otherwise.
const ACTIONS: &[(&str, &str, &str)] = &[
    ("auto", "next_page", "Next page"),
    ("auto", "prev_page", "Previous page"),
    ("auto", "next_page_tap", "Next page (tap)"),
    ("auto", "prev_page_tap", "Previous page (tap)"),
    ("auto", "menu", "Menu / toolbar"),
    ("auto", "brightness 1", "Brightness +1"),
    ("auto", "brightness -1", "Brightness -1"),
    ("auto", "brightness 5", "Brightness +5"),
    ("auto", "brightness -5", "Brightness -5"),
    ("auto", "brightness_toggle", "Toggle frontlight"),
    ("auto", "dark_mode_toggle", "Toggle dark mode"),
    ("kindle", "next_page", "Next page"),
    ("kindle", "prev_page", "Previous page"),
    ("kindle", "next_page_tap", "Next page (tap)"),
    ("kindle", "prev_page_tap", "Previous page (tap)"),
    ("kindle", "home", "Home screen"),
    ("kindle", "back", "Back"),
    ("kindle", "toolbar", "Toggle toolbar"),
    ("kindle", "brightness 1", "Brightness +1"),
    ("kindle", "brightness -1", "Brightness -1"),
    ("kindle", "brightness 5", "Brightness +5"),
    ("kindle", "brightness -5", "Brightness -5"),
    ("kindle", "brightness_toggle", "Toggle frontlight"),
    ("kindle", "dark_mode_toggle", "Toggle dark mode"),
    ("kindle", "suspend", "Sleep / screen off"),
    ("koreader", "next_page", "Next page"),
    ("koreader", "prev_page", "Previous page"),
    ("koreader", "brightness 1", "Brightness +1"),
    ("koreader", "brightness -1", "Brightness -1"),
    ("koreader", "brightness 10", "Brightness +10"),
    ("koreader", "brightness -10", "Brightness -10"),
    ("koreader", "brightness_toggle", "Toggle frontlight"),
    ("koreader", "warmth 1", "Warmth +1"),
    ("koreader", "warmth -1", "Warmth -1"),
    ("koreader", "warmth 5", "Warmth +5"),
    ("koreader", "warmth -5", "Warmth -5"),
    ("koreader", "night_mode", "Toggle night mode"),
    ("koreader", "font_up 1", "Font +1"),
    ("koreader", "font_down 1", "Font -1"),
    ("koreader", "menu", "Show menu"),
    ("koreader", "toggle_status_bar", "Toggle status bar"),
    ("koreader", "rotate", "Rotate screen"),
];

static CAPTURE_LOCK: Mutex<()> = Mutex::new(());

// Clears the capture pause flag on every capture() exit path.
struct PauseGuard;
impl Drop for PauseGuard {
    fn drop(&mut self) {
        crate::pause::end();
    }
}

pub fn run(config_path: String) -> Result<(), String> {
    let listener = TcpListener::bind(BIND_ADDR)
        .map_err(|e| format!("Cannot bind {}: {}", BIND_ADDR, e))?;
    info!("WAF helper listening on {}", BIND_ADDR);

    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let cfg = config_path.clone();
                std::thread::spawn(move || {
                    if let Err(e) = handle(s, &cfg) {
                        warn!("Request error: {}", e);
                    }
                });
            }
            Err(e) => warn!("Accept failed: {}", e),
        }
    }
    Ok(())
}

fn handle(mut stream: TcpStream, config_path: &str) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .ok();

    let (method, path, body) = read_request(&mut stream)?;
    info!("{} {}", method, path);

    let (status, body_text) = route(&method, &path, &body, config_path);
    write_response(&mut stream, status, &body_text);
    Ok(())
}

fn route(method: &str, path: &str, body: &str, config_path: &str) -> (u16, String) {
    let (route, query) = split_query(path);

    match (method, route) {
        ("GET", "/") | ("GET", "/health") => (200, json_ok()),
        ("GET", "/status") => (200, status_json(config_path)),
        ("GET", "/koreader/status") => (200, koreader_status_json()),
        ("GET", "/logs") => (200, logs_text()),
        ("GET", "/config") => match fs::read_to_string(config_path) {
            Ok(s) => (200, s),
            Err(_) => (200, String::new()),
        },
        ("POST", "/config") => match fs::write(config_path, body) {
            Ok(_) => (200, json_ok()),
            Err(e) => (500, json_err(&format!("write failed: {}", e))),
        },
        ("GET", "/devices") => (200, devices_json()),
        ("GET", "/resolve") => resolve(&query),
        ("GET", "/actions") => (200, actions_json()),
        ("GET", "/layouts") => (200, layouts_json()),
        ("GET", "/capture") => capture(&query, config_path),
        ("GET", "/record-gesture") => record_gesture(&query),
        ("POST", "/reload") => reload_daemon(),
        ("POST", "/stop") => stop_daemon(),
        ("POST", "/start") => start_daemon(),
        ("POST", "/quit") => {
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_millis(100));
                let _ = Command::new("lipc-set-prop")
                    .args([
                        "com.lab126.appmgrd",
                        "stop",
                        "app://com.lzampier.mappermanager",
                    ])
                    .status();
                std::process::exit(0);
            });
            (200, json_ok())
        }
        ("POST", "/exit-app") => {
            let _ = Command::new("lipc-set-prop")
                .args([
                    "com.lab126.appmgrd",
                    "start",
                    "app://com.lab126.booklet.home",
                ])
                .status();
            (200, json_ok())
        }
        _ => (404, json_err("not found")),
    }
}

// ---- request parsing ----

fn read_request(stream: &mut TcpStream) -> Result<(String, String, String), String> {
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| format!("read line: {}", e))?;

    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 2 {
        return Err(format!("bad request line: {:?}", line));
    }
    let method = parts[0].to_string();
    let path = parts[1].to_string();

    let mut content_length = 0usize;
    loop {
        let mut h = String::new();
        reader.read_line(&mut h).map_err(|e| e.to_string())?;
        if h == "\r\n" || h == "\n" || h.is_empty() {
            break;
        }
        let lower = h.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            content_length = rest.trim().parse().unwrap_or(0);
        }
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader
            .read_exact(&mut body)
            .map_err(|e| format!("read body: {}", e))?;
    }
    let body = String::from_utf8_lossy(&body).into_owned();
    Ok((method, path, body))
}

fn write_response(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Error",
    };
    let resp = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n{}",
        status,
        reason,
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes());
}

fn split_query(path: &str) -> (&str, std::collections::HashMap<String, String>) {
    let mut map = std::collections::HashMap::new();
    let (route, q) = match path.find('?') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => (path, ""),
    };
    for pair in q.split('&') {
        if pair.is_empty() {
            continue;
        }
        let mut it = pair.splitn(2, '=');
        let k = url_decode(it.next().unwrap_or(""));
        let v = url_decode(it.next().unwrap_or(""));
        map.insert(k, v);
    }
    (route, map)
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(b'?'));
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---- handlers ----

type Node = (String, String, String, usize);

fn sort_nodes(nodes: &mut [Node]) {
    nodes.sort_by(|a, b| {
        let (left, right) = (crate::input::bare_addr(&a.2), crate::input::bare_addr(&b.2));
        left.cmp(&right)
            .then_with(|| if left.is_empty() { Ordering::Equal } else { b.3.cmp(&a.3) })
            .then_with(|| a.0.cmp(&b.0))
    });
}

fn devices_json() -> String {
    let mut entries = Vec::new();
    if let Ok(dir) = fs::read_dir(INPUT_DIR) {
        for entry in dir.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            if !name.starts_with("event") {
                continue;
            }
            let (dev_name, dev_uniq, keys) = Device::open(&path)
                .ok()
                .map(|d| {
                    let n = d.name().unwrap_or("").to_string();
                    let u = d.unique_name().unwrap_or("").to_string();
                    (n, u, crate::input::mappable_keys(d.supported_keys(), false))
                })
                .unwrap_or_default();
            if dev_name.starts_with("kindle-button-mapper") {
                continue;
            }
            entries.push((path.display().to_string(), dev_name, dev_uniq, keys));
        }
    }
    sort_nodes(&mut entries);
    let items: Vec<String> = entries
        .iter()
        .map(|(p, n, u, _)| format!("{{\"path\":\"{}\",\"name\":\"{}\",\"uniq\":\"{}\"}}", esc(p), esc(n), esc(u)))
        .collect();
    format!("{{\"ok\":true,\"devices\":[{}]}}", items.join(","))
}

fn resolve(query: &std::collections::HashMap<String, String>) -> (u16, String) {
    match resolve_handler(query).node_paths() {
        Ok(paths) if paths.is_empty() => (200, json_err("Device not connected")),
        Ok(paths) => (
            200,
            format!(
                "{{\"ok\":true,\"paths\":[{paths}]}}",
                paths = paths
                    .iter()
                    .map(|path| format!("\"{path}\"", path = esc(&path.display().to_string())))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        ),
        Err(e) => (200, json_err(&e)),
    }
}

fn resolve_handler(
    query: &std::collections::HashMap<String, String>,
) -> crate::input::InputHandler {
    let param = |key: &str| query.get(key).filter(|value| !value.is_empty()).cloned();
    crate::input::InputHandler::new(param("name"), param("uniq"), false, false)
}

fn actions_json() -> String {
    let items: Vec<String> = ACTIONS
        .iter()
        .map(|(kind, id, label)| {
            format!(
                "{{\"kind\":\"{}\",\"id\":\"{}\",\"label\":\"{}\"}}",
                esc(kind),
                esc(id),
                esc(label)
            )
        })
        .collect();
    format!("{{\"ok\":true,\"actions\":[{}]}}", items.join(","))
}

const XKB_RULES_LST: &str = "/usr/share/X11/xkb/rules/evdev.lst";

fn layouts_json() -> String {
    let content = fs::read_to_string(XKB_RULES_LST).unwrap_or_default();
    let mut in_section = false;
    let mut items: Vec<String> = Vec::new();
    for line in content.lines() {
        let header = line.trim();
        if header.starts_with('!') {
            in_section = header == "! layout";
            continue;
        }
        if !in_section || header.is_empty() {
            continue;
        }
        let mut parts = header.splitn(2, char::is_whitespace);
        let code = parts.next().unwrap_or("").trim();
        let name = parts.next().unwrap_or("").trim();
        if code.is_empty() {
            continue;
        }
        items.push(format!("{{\"code\":\"{}\",\"name\":\"{}\"}}", esc(code), esc(name)));
    }
    format!("{{\"ok\":true,\"layouts\":[{}]}}", items.join(","))
}

fn status_json(config_path: &str) -> String {
    let (running, pid) = daemon_status();
    format!(
        "{{\"ok\":true,\"running\":{},\"pid\":{},\"config\":\"{}\",\"version\":\"{}\",\"build\":\"{}\"}}",
        running,
        pid,
        esc(config_path),
        env!("CARGO_PKG_VERSION"),
        env!("BUILD_SHA")
    )
}

const KOREADER_SETTINGS_PATH: &str = "/mnt/us/koreader/settings.reader.lua";

const KOPLUGIN_EVENTSERVER_PATH: &str =
    "/mnt/us/koreader/plugins/hidpassthrough.koplugin/eventserver.lua";

fn koreader_status_json() -> String {
    let autostart = fs::read_to_string(KOREADER_SETTINGS_PATH)
        .map(|s| httpinspector_autostart_enabled(&s))
        .unwrap_or(false);
    let plugin = std::path::Path::new(KOPLUGIN_EVENTSERVER_PATH).exists();
    format!(
        "{{\"ok\":true,\"autostart\":{},\"plugin\":{}}}",
        autostart, plugin
    )
}

fn httpinspector_autostart_enabled(lua: &str) -> bool {
    lua.split_once("[\"httpinspector\"]")
        .and_then(|(_, rest)| rest.split_once('}'))
        .is_some_and(|(table, _)| table.contains("[\"autostart\"] = true"))
}

fn logs_text() -> String {
    const LOG_PATH: &str = "/var/log/kindle-button-mapper.log";
    match fs::read_to_string(LOG_PATH) {
        Ok(s) => {
            let lines: Vec<&str> = s.lines().collect();
            let start = lines.len().saturating_sub(200);
            lines[start..].join("\n")
        }
        Err(e) => format!("cannot read {}: {}", LOG_PATH, e),
    }
}

fn record_gesture(query: &std::collections::HashMap<String, String>) -> (u16, String) {
    let lock = match CAPTURE_LOCK.try_lock() {
        Ok(g) => g,
        Err(_) => return (200, json_err("capture already running")),
    };
    let paths = match query.get("device") {
        Some(p) if !p.is_empty() => p.clone(),
        _ => return (200, json_err("missing device param")),
    };
    let timeout_ms: u64 = query
        .get("timeout")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10000)
        .min(20000);

    let _pause = PauseGuard;
    let _ = crate::pause::begin();
    std::thread::sleep(Duration::from_millis(300));

    let mut nodes = match open_nodes(&paths) {
        Ok(nodes) => nodes,
        Err(e) => {
            drop(lock);
            return (200, json_err(&e));
        }
    };

    let ranges: Vec<_> = nodes.iter().map(crate::stick_ranges).collect();

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut strokes: Vec<crate::gesture::Stroke> = nodes
        .iter()
        .map(|_| crate::gesture::Stroke::default())
        .collect();

    while Instant::now() < deadline {
        let events = match fetch_all(&mut nodes) {
            Ok(events) if events.is_empty() => {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            Ok(events) => events,
            Err(e) => {
                drop(lock);
                return (200, json_err(&format!("read: {}", e)));
            }
        };
        for (node, ev) in events {
            match ev.kind() {
                InputEventKind::Synchronization(_) => strokes[node].sync(),
                InputEventKind::Key(k) if k.code() == 330 => {
                    if ev.value() == 1 {
                        strokes[node].press();
                    } else if let Some(path) = strokes[node].release() {
                        if crate::gesture::travel(&path) >= 10.0 {
                            let pts = crate::gesture::normalize(&path);
                            drop(lock);
                            return (
                                200,
                                format!(
                                    "{{\"ok\":true,\"points\":\"{}\"}}",
                                    crate::gesture::to_string(&pts)
                                ),
                            );
                        }
                    }
                }
                InputEventKind::AbsAxis(axis) => {
                    let code = axis.0;
                    if code == 0 || code == 53 {
                        strokes[node]
                            .axis(true, crate::stick_percent(&ranges[node], code, ev.value()));
                    } else if code == 1 || code == 54 {
                        strokes[node]
                            .axis(false, crate::stick_percent(&ranges[node], code, ev.value()));
                    }
                }
                _ => {}
            }
        }
    }
    drop(lock);
    (200, json_err("timeout"))
}

fn capture(query: &std::collections::HashMap<String, String>, config_path: &str) -> (u16, String) {
    let lock = match CAPTURE_LOCK.try_lock() {
        Ok(g) => g,
        Err(_) => return (200, json_err("capture already running")),
    };
    let paths = match query.get("device") {
        Some(p) if !p.is_empty() => p.clone(),
        _ => return (200, json_err("missing device param")),
    };
    // Capped to stay within the pause-flag freshness window.
    let timeout_ms: u64 = query
        .get("timeout")
        .and_then(|s| s.parse().ok())
        .unwrap_or(8000)
        .min(15000);

    // Make the daemon drop its grab, then let its workers ungrab before reading.
    let _pause = PauseGuard;
    let _ = crate::pause::begin();
    std::thread::sleep(Duration::from_millis(300));

    let mut nodes = match open_nodes(&paths) {
        Ok(nodes) => nodes,
        Err(e) => return (200, json_err(&e)),
    };
    let ranges: Vec<_> = nodes.iter().map(crate::stick_ranges).collect();
    let contacts: Vec<bool> = nodes.iter().map(reports_contacts).collect();
    let mut touching = vec![false; nodes.len()];
    let deadzone = crate::config::Config::load(config_path).map_or(50, |c| c.stick_deadzone);

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        let events = match fetch_all(&mut nodes) {
            Ok(events) if events.is_empty() => {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            Ok(events) => events,
            Err(e) => {
                drop(lock);
                return (200, json_err(&format!("read: {}", e)));
            }
        };
        for (node, ev) in events {
            match ev.kind() {
                InputEventKind::Key(Key::BTN_TOUCH) if contacts[node] => {
                    touching[node] = ev.value() != 0;
                }
                InputEventKind::Key(k) if contacts[node] && (touching[node] || is_tool(k)) => {}
                InputEventKind::Key(k) => {
                    if ev.value() != 1 {
                        continue;
                    }
                    drop(lock);
                    return (
                        200,
                        format!("{{\"ok\":true,\"kind\":\"key\",\"code\":{}}}", k.code()),
                    );
                }
                InputEventKind::AbsAxis(axis) => {
                    let code = axis.0;
                    let kind = match code {
                        16 | 17 => "dpad",
                        9 | 10 => "trigger",
                        0 | 1 | 3 | 4 => "stick",
                        _ => continue,
                    };
                    let v = if kind == "stick" {
                        crate::stick_percent(&ranges[node], code, ev.value())
                    } else {
                        ev.value()
                    };
                    let idle = if kind == "stick" { v.abs() < deadzone } else { v == 0 };
                    if idle {
                        continue;
                    }
                    drop(lock);
                    return (
                        200,
                        format!(
                            "{{\"ok\":true,\"kind\":\"{}\",\"code\":{},\"value\":{}}}",
                            kind, code, v
                        ),
                    );
                }
                _ => {}
            }
        }
    }
    drop(lock);
    (200, json_err("timeout"))
}

/// A remote that reports its buttons as contacts, BTN_TOUCH next to ABS_X and
/// ABS_Y on a node that is no touchscreen (like an F3 ring), tells them apart
/// by the contact's motion alone: its tool keys, and the keys that go down
/// with a contact, BTN_TOUCH among them, are the same for every button.
fn reports_contacts(device: &Device) -> bool {
    device
        .supported_keys()
        .is_some_and(|keys| keys.contains(Key::BTN_TOUCH))
        && crate::input::reports_position(device.supported_absolute_axes())
        && !crate::input::is_touchscreen(device)
}

fn is_tool(key: Key) -> bool {
    matches!(
        key,
        Key::BTN_TOOL_PEN
            | Key::BTN_TOOL_RUBBER
            | Key::BTN_TOOL_BRUSH
            | Key::BTN_TOOL_PENCIL
            | Key::BTN_TOOL_AIRBRUSH
            | Key::BTN_TOOL_FINGER
            | Key::BTN_TOOL_MOUSE
            | Key::BTN_TOOL_LENS
            | Key::BTN_TOOL_QUINTTAP
            | Key::BTN_TOOL_DOUBLETAP
            | Key::BTN_TOOL_TRIPLETAP
            | Key::BTN_TOOL_QUADTAP
    )
}

// Non-blocking so the deadline check actually fires when the device
// is idle. Without this the read parks forever, holding CAPTURE_LOCK.
fn open_nodes(paths: &str) -> Result<Vec<Device>, String> {
    use std::os::unix::io::AsRawFd;
    let nodes: Vec<Device> = paths
        .split(',')
        .filter_map(|path| {
            Device::open(path)
                .inspect_err(|e| warn!("Capture skips {path}: {e}"))
                .ok()
        })
        .inspect(|device| crate::set_nonblocking(device.as_raw_fd()))
        .collect();
    (!nodes.is_empty())
        .then_some(nodes)
        .ok_or_else(|| format!("cannot open {paths}"))
}

fn fetch_all(nodes: &mut [Device]) -> std::io::Result<Vec<(usize, evdev::InputEvent)>> {
    nodes
        .iter_mut()
        .enumerate()
        .try_fold(Vec::new(), |mut events, (node, device)| {
            match device.fetch_events() {
                Ok(fetched) => events.extend(fetched.map(|event| (node, event))),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e),
            }
            Ok(events)
        })
}

// The daemon runs as an upstart service, which — unlike the old SysV init
// script — writes no pidfile. Ask upstart directly instead of stat-ing a
// pidfile that never gets created.
fn daemon_status() -> (bool, u32) {
    let output = match Command::new(INITCTL).args(["status", SERVICE]).output() {
        Ok(o) => o,
        Err(_) => return (false, 0),
    };
    let text = String::from_utf8_lossy(&output.stdout);
    // e.g. "kindle-button-mapper start/running, process 1234"
    let running = text.contains("start/running");
    if !running {
        return (false, 0);
    }
    let pid = text
        .rsplit("process ")
        .next()
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);
    (true, pid)
}

fn run_initctl(action: &str) -> (u16, String) {
    match Command::new(INITCTL).args([action, SERVICE]).status() {
        Ok(s) if s.success() => (200, json_ok()),
        Ok(s) => (
            500,
            json_err(&format!("{} exited with {}", action, s.code().unwrap_or(-1))),
        ),
        Err(e) => {
            error!("initctl {}: {}", action, e);
            (500, json_err(&format!("initctl: {}", e)))
        }
    }
}

/// SIGHUP rather than a restart: restarting drops the uinput keyboard, and
/// anything holding that node open (KOReader) sees its input device vanish and
/// rebuilds its UI, throwing the user out of the menu they were editing from.
/// Falls back to a restart for a daemon too old to handle the signal.
fn reload_daemon() -> (u16, String) {
    let (running, pid) = daemon_status();
    if running && pid > 0 {
        let target = Pid::from_raw(pid as i32);
        match kill(target, Signal::SIGHUP) {
            Ok(()) => {
                info!("Asked the daemon to reload its config in place");
                return (200, json_ok());
            }
            Err(e) => warn!("SIGHUP to {} failed: {}, restarting instead", pid, e),
        }
    }
    restart_daemon()
}

fn restart_daemon() -> (u16, String) {
    run_initctl("restart")
}

fn stop_daemon() -> (u16, String) {
    run_initctl("stop")
}

fn start_daemon() -> (u16, String) {
    run_initctl("start")
}

// ---- JSON helpers ----

fn json_ok() -> String {
    "{\"ok\":true}".to_string()
}

fn json_err(msg: &str) -> String {
    format!("{{\"ok\":false,\"error\":\"{}\"}}", esc(msg))
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{resolve_handler, sort_nodes, split_query};
    use crate::input::InputHandler;

    #[test]
    fn resolve_reads_a_section_the_way_mapper_manager_sends_it() {
        // resolveDevicePath's query, encoded by encodeURIComponent.
        let resolved = |url| resolve_handler(&split_query(url).1);

        assert_eq!(
            resolved("/resolve?name=F3%20Touchscreen&uniq=EB%3A80%3A03%3A88%3A70%3A8B"),
            InputHandler::new(
                Some("F3 Touchscreen".to_string()),
                Some("EB:80:03:88:70:8B".to_string()),
                false,
                false,
            )
        );
        // An unset field is no identity at all, so node_paths refuses it
        // rather than listing every node.
        assert_eq!(
            resolved("/resolve?name=&uniq="),
            InputHandler::new(None, None, false, false)
        );
    }

    #[test]
    fn a_stick_that_centres_off_zero_still_names_both_directions() {
        let ranges = std::collections::HashMap::from([(0u16, (128, 127))]);
        assert_eq!(crate::stick_percent(&ranges, 0, 0), -100);
        assert_eq!(crate::stick_percent(&ranges, 0, 255), 100);
        assert_eq!(crate::stick_percent(&ranges, 0, 128), 0);
    }

    fn node(path: &str, name: &str, uniq: &str, keys: usize) -> super::Node {
        (path.into(), name.into(), uniq.into(), keys)
    }

    #[test]
    fn nodes_of_one_address_lead_with_the_keys_the_rest_keep_path_order() {
        let mut nodes = vec![
            node("/dev/input/event3", "Smart 1-P Consumer Control", "E0:F6:B5:BC:1C:7F/P", 20),
            node("/dev/input/event1", "goodix-ts", "", 4),
            node("/dev/input/event4", "Smart 1-P Keyboard", "e0:f6:b5:bc:1c:7f", 120),
            node("/dev/input/event0", "bd71828-pwrkey", "", 1),
        ];
        sort_nodes(&mut nodes);
        let paths: Vec<&str> = nodes.iter().map(|n| n.0.as_str()).collect();
        assert_eq!(
            paths,
            [
                "/dev/input/event0",
                "/dev/input/event1",
                "/dev/input/event4",
                "/dev/input/event3"
            ]
        );
    }
}
