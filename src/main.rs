mod config;
mod input;
mod layout;
mod mapper;
mod pause;
mod vkeyboard;
mod waf_helper;

use config::Config;
use evdev::uinput::VirtualDevice;
use evdev::{EventType, InputEvent, InputEventKind};
use input::InputHandler;
use layout::LayoutAsserter;
use log::{error, info, warn};
use mapper::Mapper;
use nix::poll::{poll, PollFd, PollFlags};
use nix::sys::signal::{signal, SigHandler, Signal};
use std::env;
use std::os::fd::BorrowedFd;
use std::os::unix::io::AsRawFd;
use std::process::{self, Command};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// A key that isn't seen again within this gap starts a new "burst". The
/// framework re-pins `us` on focus-in, so we re-assert the layout at burst
/// start (not per key, to leave in-layout group toggling alone).
const BURST_GAP: Duration = Duration::from_millis(400);

type SharedKeyboard = Option<Arc<Mutex<VirtualDevice>>>;

const KEEP_AWAKE_INTERVAL: Duration = Duration::from_secs(60);
const KEEP_AWAKE_POKE: &str = "lipc-set-prop -i com.lab126.powerd touchScreenSaverTimeout 1";
const KEEP_AWAKE_RELEASE: &str = "lipc-set-prop com.lab126.powerd preventScreenSaver 0";

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

    // Virtual keyboard via uinput — kept alive for the daemon's lifetime.
    // The path is written to /var/run/kindle-button-mapper-key-target so
    // scripts/key.sh can inject events into it.
    let vkeyboard: SharedKeyboard = vkeyboard::try_init().map(|d| Arc::new(Mutex::new(d)));

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
        let vkbd = vkeyboard.clone();
        let h = thread::Builder::new()
            .name(format!("dev:{}", id))
            .spawn(move || device_worker(device, settings, vkbd))
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

fn device_worker(cfg: config::DeviceConfig, settings: WorkerSettings, vkeyboard: SharedKeyboard) {
    let mut mapper = Mapper::new(
        &cfg,
        settings.debounce_ms,
        settings.long_press_ms,
        settings.repeat_ms,
        settings.log_buttons,
    );

    // Precompile the layout once (None if unset or the symbols file is missing).
    let mut layout = cfg
        .keyboard_layout
        .as_deref()
        .and_then(|l| LayoutAsserter::new(&cfg.id, l));
    if layout.is_none() {
        if let Some(l) = cfg.keyboard_layout.as_deref() {
            warn!("[{}] keyboard_layout '{}' unavailable — passthrough will use us", cfg.id, l);
        }
    }

    loop {
        let handler = InputHandler::new(cfg.name.clone(), cfg.uniq.clone(), cfg.grab);
        match handler.open() {
            Ok(mut device) => {
                info!("[{}] device connected", cfg.id);
                if let Some(ref script) = settings.on_connect {
                    info!("[{}] running on_connect script", cfg.id);
                    execute_script(script);
                }
                if let Some(la) = layout.as_mut() {
                    la.assert();
                }
                if let Err(e) = run_event_loop(
                    &mut device,
                    &mut mapper,
                    cfg.grab,
                    settings.keep_awake,
                    vkeyboard.as_ref(),
                    layout.as_mut(),
                ) {
                    error!("[{}] event loop error: {}", cfg.id, e);
                    if let Some(ref script) = settings.on_disconnect {
                        info!("[{}] device disconnected, running on_disconnect script", cfg.id);
                        execute_script(script);
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

fn run_event_loop(
    device: &mut evdev::Device,
    mapper: &mut Mapper,
    grab: bool,
    keep_awake: bool,
    vkeyboard: Option<&Arc<Mutex<VirtualDevice>>>,
    mut layout: Option<&mut LayoutAsserter>,
) -> Result<(), String> {
    // Non-blocking + poll so we can notice a capture pause while idle.
    set_nonblocking(device.as_raw_fd());
    let mut grabbed = grab;
    let mut last_key: Option<Instant> = None;

    let mut last_poke: Option<Instant> = if keep_awake {
        execute_script(KEEP_AWAKE_RELEASE);
        Some(Instant::now())
    } else {
        None
    };

    loop {
        // Release the grab while capture is paused, restore it after.
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
            for _ in events {} // drain; these presses belong to capture
            continue;
        }

        let mut activity = false;
        for event in events {
            match event.kind() {
                InputEventKind::Key(key) => {
                    activity = true;
                    if mapper.is_mapped(key) {
                        match event.value() {
                            1 => mapper.handle_press(key),   // Press
                            2 => mapper.handle_held(key),    // Held/repeat
                            0 => mapper.handle_release(key), // Release
                            _ => {}
                        }
                    } else if let Some(vkbd) = vkeyboard {
                        // Unmapped key: pass it through the virtual keyboard so
                        // the layout maps it. Re-assert the layout at burst
                        // start — the framework re-pins us on focus-in.
                        if event.value() == 1 {
                            let burst_start =
                                last_key.map_or(true, |t| t.elapsed() > BURST_GAP);
                            if burst_start {
                                if let Some(la) = layout.as_deref_mut() {
                                    la.assert();
                                }
                            }
                            last_key = Some(Instant::now());
                        }
                        if let Ok(mut d) = vkbd.lock() {
                            let _ = d.emit(&[InputEvent::new(
                                EventType::KEY,
                                key.code(),
                                event.value(),
                            )]);
                        }
                    }
                }
                InputEventKind::AbsAxis(axis) => {
                    let code = axis.0;
                    match code {
                        // D-pad: Hat0X (16) and Hat0Y (17)
                        16 | 17 => {
                            activity = true;
                            mapper.handle_dpad(code, event.value());
                        }
                        // Triggers: Gas (9) = RT, Brake (10) = LT
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

        if keep_awake && activity {
            let now = Instant::now();
            if last_poke.is_none_or(|t| now.duration_since(t) >= KEEP_AWAKE_INTERVAL) {
                info!("keep-awake: re-armed screensaver idle timer");
                execute_script(KEEP_AWAKE_POKE);
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
    // _exit is async-signal-safe; process::exit is not.
    unsafe { nix::libc::_exit(0) }
}

fn execute_script(script: &str) {
    match Command::new("/bin/sh").args(["-c", script]).spawn() {
        Ok(mut child) => {
            // Wait for completion (blocking) for disconnect script
            let _ = child.wait();
        }
        Err(e) => {
            error!("Failed to execute '{}': {}", script, e);
        }
    }
}

