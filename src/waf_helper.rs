use evdev::{Device, InputEventKind};
use log::{error, info, warn};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const BIND_ADDR: &str = "127.0.0.1:8322";
const INPUT_DIR: &str = "/dev/input";
const INITCTL: &str = "/sbin/initctl";
const SERVICE: &str = "kindle-button-mapper";

const ACTIONS: &[(&str, &str)] = &[
    ("next_page", "Next page"),
    ("prev_page", "Previous page"),
    ("brightness 1", "Brightness +1"),
    ("brightness -1", "Brightness -1"),
    ("brightness 10", "Brightness +10"),
    ("brightness -10", "Brightness -10"),
    ("brightness_toggle", "Toggle frontlight"),
    ("night_mode", "Toggle night mode"),
    ("font_up 1", "Font +1"),
    ("font_down 1", "Font -1"),
    ("menu", "Show menu"),
    ("toggle_status_bar", "Toggle status bar"),
    ("rotate", "Rotate screen"),
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
        ("GET", "/actions") => (200, actions_json()),
        ("GET", "/layouts") => (200, layouts_json()),
        ("GET", "/capture") => capture(&query),
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
            let (dev_name, dev_uniq) = Device::open(&path)
                .ok()
                .map(|d| {
                    let n = d.name().unwrap_or("").to_string();
                    let u = d.unique_name().unwrap_or("").to_string();
                    (n, u)
                })
                .unwrap_or_default();
            if dev_name == "kindle-button-mapper" {
                continue;
            }
            entries.push((path.display().to_string(), dev_name, dev_uniq));
        }
    }
    entries.sort();
    let items: Vec<String> = entries
        .iter()
        .map(|(p, n, u)| format!("{{\"path\":\"{}\",\"name\":\"{}\",\"uniq\":\"{}\"}}", esc(p), esc(n), esc(u)))
        .collect();
    format!("{{\"ok\":true,\"devices\":[{}]}}", items.join(","))
}

fn actions_json() -> String {
    let items: Vec<String> = ACTIONS
        .iter()
        .map(|(id, label)| format!("{{\"id\":\"{}\",\"label\":\"{}\"}}", esc(id), esc(label)))
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

fn koreader_status_json() -> String {
    let autostart = fs::read_to_string(KOREADER_SETTINGS_PATH)
        .map(|s| httpinspector_autostart_enabled(&s))
        .unwrap_or(false);
    format!("{{\"ok\":true,\"autostart\":{}}}", autostart)
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

fn capture(query: &std::collections::HashMap<String, String>) -> (u16, String) {
    let lock = match CAPTURE_LOCK.try_lock() {
        Ok(g) => g,
        Err(_) => return (200, json_err("capture already running")),
    };
    let path = match query.get("device") {
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

    let mut device = match Device::open(&path) {
        Ok(d) => d,
        Err(e) => return (200, json_err(&format!("open {}: {}", path, e))),
    };
    // Non-blocking so the deadline check actually fires when the device
    // is idle. Without this the read parks forever, holding CAPTURE_LOCK.
    use nix::libc;
    use std::os::unix::io::AsRawFd;
    unsafe {
        let fd = device.as_raw_fd();
        let flags = libc::fcntl(fd, libc::F_GETFL, 0);
        libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        let events = match device.fetch_events() {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            Err(e) => {
                drop(lock);
                return (200, json_err(&format!("read: {}", e)));
            }
        };
        for ev in events {
            match ev.kind() {
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
                    if ev.value() == 0 {
                        continue;
                    }
                    let code = axis.0;
                    let v = ev.value();
                    let kind = match code {
                        16 | 17 => "dpad",
                        9 | 10 => "trigger",
                        _ => continue,
                    };
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

fn reload_daemon() -> (u16, String) {
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
