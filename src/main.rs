mod config;
mod input;
mod mapper;
mod pause;
mod vkeyboard;
mod waf_helper;

use config::Config;
use evdev::InputEventKind;
use input::InputHandler;
use log::{error, info, warn};
use mapper::Mapper;
use nix::poll::{poll, PollFd, PollFlags};
use nix::sys::signal::{signal, SigHandler, Signal};
use std::env;
use std::fs;
use std::io::Write;
use std::os::fd::BorrowedFd;
use std::os::unix::io::AsRawFd;
use std::process::{self, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

// Fallback interval: 5 minutes (300 seconds)
const DEFAULT_KEEP_AWAKE_INTERVAL: Duration = Duration::from_secs(300);
const KEEP_AWAKE_POKE: &str = "lipc-set-prop -i com.lab126.powerd touchScreenSaverTimeout 1";
const KEEP_AWAKE_RELEASE: &str = "lipc-set-prop com.lab126.powerd preventScreenSaver 0";

/// Dynamically parse auto_suspend_timeout_seconds from KOReader configuration
fn get_koreader_keep_awake_interval() -> Duration {
    let settings_path = "/mnt/us/koreader/settings.reader.lua";
    if let Ok(content) = fs::read_to_string(settings_path) {
        // Match ["auto_suspend_timeout_seconds"] = 900,
        for line in content.lines() {
            if line.contains("auto_suspend_timeout_seconds") {
                if let Some(num_str) = line.split('=').nth(1) {
                    let cleaned = num_str.trim().trim_matches(|c| c == ',' || c == '"' || c == '\'');
                    if let Ok(secs) = cleaned.parse::<i64>() {
                        // If KOReader sets -1 (disabled), fallback to default 300s
                        if secs > 60 {
                            let safe_interval = (secs / 2) as u64; // Set refresh interval to half of the timeout
                            info!(
                                "KOReader auto_suspend_timeout_seconds detected: {}s. Setting keep-awake interval to {}s.",
                                secs, safe_interval
                            );
                            return Duration::from_secs(safe_interval);
                        }
                    }
                }
            }
        }
    }
    info!("Could not read KOReader auto_suspend_timeout_seconds. Using default interval: 300s.");
    DEFAULT_KEEP_AWAKE_INTERVAL
}

fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info")
    ).init();

    let args: Vec<String> = env::args().collect();

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!(
            "kindle-button-mapper {} (build {})",
            env!("CARGO_PKG_VERSION"),
            env!("BUILD_SHA")
        );
        return;
    }

    if args.iter().any(|a| a == "--waf-helper") {
        let cfg = args
            .iter()
            .skip(1)
            .find(|a| !a.starts_with("--"))
            .cloned()
            .unwrap_or_else(|| "config.ini".to_string());
        if let Err(e) = waf_helper::run(cfg) {
            error!("WAF helper failed: {}", e);
            process::exit(1);
        }
        return;
    }

    let config_path = if args.len() > 1 {
        &args[1]
    } else {
        "config.ini"
    };

    let config = match Config::load(config_path) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to load config: {}", e);
            process::exit(1);
        }
    };

    info!(
        "Kindle Button Mapper {} (build {}) starting...",
        env!("CARGO_PKG_VERSION"),
        env!("BUILD_SHA")
    );
    info!(
        "Config: devices={}, debounce={}ms, long_press={}ms, repeat={}ms",
        config.devices.len(),
        config.debounce_ms,
        config.long_press_ms,
        config.repeat_ms,
    );

    unsafe {
        signal(Signal::SIGINT, SigHandler::Handler(handle_signal)).ok();
        signal(Signal::SIGTERM, SigHandler::Handler(handle_signal)).ok();
    }

    let _vkeyboard = vkeyboard::try_init();

    let mut handles = Vec::new();
    let settings = WorkerSettings {
        debounce_ms: config.debounce_ms,
        long_press_ms: config.long_press_ms,
        repeat_ms: config.repeat_ms,
        log_buttons: config.log_buttons,
        keep_awake: config.keep_awake,
        on_connect: config.on_connect.clone(),
        on_disconnect: config.on_disconnect.clone(),
    };
    for device in config.devices {
        let id = device.id.clone();
        let settings = settings.clone();
        let h = thread::Builder::new()
            .name(format!("dev:{}", id))
            .spawn(move || device_worker(device, settings))
            .expect("spawn device thread");
        handles.push(h);
    }

    if handles.is_empty() {
        info!("No devices configured — idling. Add a device via the Button Mapper WAF app and restart.");
        loop {
            thread::sleep(Duration::from_secs(60));
        }
    }
    for h in handles {
        let _ = h.join();
    }
}

#[derive(Clone)]
struct WorkerSettings {
    debounce_ms: u64,
    long_press_ms: u64,
    repeat_ms: u64,
    log_buttons: bool,
    keep_awake: bool,
    on_connect: Option<String>,
    on_disconnect: Option<String>,
}

fn device_worker(cfg: config::DeviceConfig, settings: WorkerSettings) {
    let mut mapper = Mapper::new(
        &cfg,
        settings.debounce_ms,
        settings.long_press_ms,
        settings.repeat_ms,
        settings.log_buttons,
    );

    loop {
        let handler = InputHandler::new(cfg.name.clone(), cfg.uniq.clone(), cfg.grab);
        match handler.open() {
            Ok(mut device) => {
                info!("[{}] device connected", cfg.id);
                if let Some(ref script) = settings.on_connect {
                    info!("[{}] running on_connect script", cfg.id);
                    execute_script_detach(script);
                }
                if let Some(ref layout) = cfg.keyboard_layout {
                    info!("[{}] applying keyboard layout '{}'", cfg.id, layout);
                    apply_keyboard_layout(layout);
                }
                if let Err(e) = run_event_loop(&mut device, &mut mapper, cfg.grab, settings.keep_awake) {
                    error!("[{}] event loop error: {}", cfg.id, e);
                    if let Some(ref script) = settings.on_disconnect {
                        info!("[{}] device disconnected, running on_disconnect script", cfg.id);
                        execute_script_detach(script);
                    }
                }
            }
            Err(e) => {
                error!("[{}] failed to open device: {}", cfg.id, e);
            }
        }
        info!("[{}] reconnecting in 1 second...", cfg.id);
        thread::sleep(Duration::from_secs(1));
    }
}

fn run_event_loop(device: &mut evdev::Device, mapper: &mut Mapper, grab: bool, keep_awake: bool) -> Result<(), String> {
    set_nonblocking(device.as_raw_fd());
    let mut grabbed = grab;

    // Dynamically retrieve Keep-Awake interval
    let keep_awake_interval = if keep_awake {
        get_koreader_keep_awake_interval()
    } else {
        DEFAULT_KEEP_AWAKE_INTERVAL
    };

    let mut last_poke: Option<Instant> = if keep_awake {
        execute_script_detach(KEEP_AWAKE_RELEASE);
        Some(Instant::now())
    } else {
        None
    };

    loop {
        let paused = pause::active();
        if paused && grabbed {
            let _ = device.ungrab();
            grabbed = false;
            info!("Released exclusive grab for capture");
        } else if !paused && grab && !grabbed {
            match device.grab() {
                Ok(()) => info!("Re-grabbed device after capture"),
                Err(e) => warn!("Cannot re-grab device: {}", e),
            }
            grabbed = true;
        }

        let mut fds = [PollFd::new(
            unsafe { BorrowedFd::borrow_raw(device.as_raw_fd()) },
            PollFlags::POLLIN,
        )];
        match poll(&mut fds, 250u16) {
            Ok(0) => continue,
            Ok(_) => {}
            Err(nix::errno::Errno::EINTR) => continue,
            Err(e) => return Err(format!("poll error: {}", e)),
        }

        let events = match device.fetch_events() {
            Ok(ev) => ev,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) => return Err(format!("Read error: {}", e)),
        };

        if paused {
            for _ in events {}
            continue;
        }

        let mut activity = false;
        for event in events {
            match event.kind() {
                InputEventKind::Key(key) => {
                    activity = true;
                    match event.value() {
                        1 => mapper.handle_press(key),
                        2 => mapper.handle_held(key),
                        0 => mapper.handle_release(key),
                        _ => {}
                    }
                }
                InputEventKind::AbsAxis(axis) => {
                    let code = axis.0;
                    match code {
                        16 | 17 => {
                            activity = true;
                            mapper.handle_dpad(code, event.value());
                        }
                        9 | 10 => {
                            activity = true;
                            mapper.handle_trigger(code, event.value());
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        // Re-arm screensaver idle timer only after the interval has elapsed
        if keep_awake && activity {
            let now = Instant::now();
            if last_poke.is_none_or(|t| now.duration_since(t) >= keep_awake_interval) {
                info!("keep-awake: re-armed screensaver idle timer (interval: {}s)", keep_awake_interval.as_secs());
                execute_script_detach(KEEP_AWAKE_POKE);
                last_poke = Some(now);
            }
        }
    }
}

fn set_nonblocking(fd: std::os::unix::io::RawFd) {
    use nix::libc;
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL, 0);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}

extern "C" fn handle_signal(_: i32) {
    unsafe { nix::libc::_exit(0) }
}

const XKB_DISPLAY: &str = ":0";

fn apply_keyboard_layout(layout: &str) {
    let keymap = format!(
        "xkb_keymap {{\n\
         \x20 xkb_keycodes {{ include \"evdev+aliases(qwerty)\" }};\n\
         \x20 xkb_types {{ include \"complete\" }};\n\
         \x20 xkb_compat {{ include \"complete\" }};\n\
         \x20 xkb_symbols {{ include \"pc+{layout}\" }};\n\
         \x20 xkb_geometry {{ include \"pc(pc105)\" }};\n\
         }};\n"
    );
    let mut child = match Command::new("xkbcomp")
        .args(["-I/usr/share/X11/xkb", "-", XKB_DISPLAY])
        .stdin(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            error!("xkbcomp failed to start: {}", e);
            return;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(keymap.as_bytes());
    }
    let _ = child.wait();
}

fn execute_script_detach(script: &str) {
    let script = script.to_string();
    thread::spawn(move || {
        match Command::new("/bin/sh").args(["-c", &script]).spawn() {
            Ok(mut child) => {
                let _ = child.wait();
            }
            Err(e) => {
                error!("Failed to execute detached script '{}': {}", script, e);
            }
        }
    });
}
