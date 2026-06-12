mod config;
mod input;
mod mapper;
mod vkeyboard;
mod waf_helper;

use config::Config;
use evdev::InputEventKind;
use input::InputHandler;
use log::{error, info};
use mapper::Mapper;
use nix::sys::signal::{signal, SigHandler, Signal};
use std::env;
use std::process::{self, Command};
use std::thread;
use std::time::Duration;

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
    let _vkeyboard = vkeyboard::try_init();

    let mut handles = Vec::new();
    let on_connect = config.on_connect.clone();
    let on_disconnect = config.on_disconnect.clone();
    for device in config.devices {
        let id = device.id.clone();
        let debounce_ms = config.debounce_ms;
        let long_press_ms = config.long_press_ms;
        let repeat_ms = config.repeat_ms;
        let log_buttons = config.log_buttons;
        let on_conn = on_connect.clone();
        let on_disc = on_disconnect.clone();
        let h = thread::Builder::new()
            .name(format!("dev:{}", id))
            .spawn(move || device_worker(device, debounce_ms, long_press_ms, repeat_ms, log_buttons, on_conn, on_disc))
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

fn device_worker(
    cfg: config::DeviceConfig,
    debounce_ms: u64,
    long_press_ms: u64,
    repeat_ms: u64,
    log_buttons: bool,
    on_connect: Option<String>,
    on_disconnect: Option<String>,
) {
    let mut mapper = Mapper::new(&cfg, debounce_ms, long_press_ms, repeat_ms, log_buttons);

    loop {
        let handler = InputHandler::new(cfg.name.clone(), cfg.path.clone(), cfg.uniq.clone(), cfg.grab);
        match handler.open() {
            Ok(mut device) => {
                info!("[{}] device connected", cfg.id);
                if let Some(ref script) = on_connect {
                    info!("[{}] running on_connect script", cfg.id);
                    execute_script(script);
                }
                if let Err(e) = run_event_loop(&mut device, &mut mapper) {
                    error!("[{}] event loop error: {}", cfg.id, e);
                    if let Some(ref script) = on_disconnect {
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

fn run_event_loop(device: &mut evdev::Device, mapper: &mut Mapper) -> Result<(), String> {
    loop {
        let events = device.fetch_events()
            .map_err(|e| format!("Read error: {}", e))?;

        for event in events {
            match event.kind() {
                InputEventKind::Key(key) => {
                    match event.value() {
                        1 => mapper.handle_press(key),  // Press
                        2 => mapper.handle_held(key),   // Held/repeat
                        0 => mapper.handle_release(key), // Release
                        _ => {}
                    }
                }
                InputEventKind::AbsAxis(axis) => {
                    let code = axis.0;
                    match code {
                        // D-pad: Hat0X (16) and Hat0Y (17)
                        16 | 17 => mapper.handle_dpad(code, event.value()),
                        // Triggers: Gas (9) = RT, Brake (10) = LT
                        9 | 10 => mapper.handle_trigger(code, event.value()),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
}

extern "C" fn handle_signal(_: i32) {
    // Exit immediately - fetch_events() blocks so a shutdown flag would
    // never be checked. _exit is async-signal-safe; process::exit is not.
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
