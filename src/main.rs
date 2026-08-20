mod action;
mod config;
mod input;
mod koreader;
mod layout;
mod mapper;
mod pause;
mod reload;
mod vkeyboard;
mod waf_helper;

use config::Config;
use evdev::InputEventKind;
use input::InputHandler;
use layout::LayoutOverride;
use log::{error, info, warn};
use mapper::Mapper;
use nix::poll::{poll, PollFd, PollFlags};
use nix::sys::signal::{signal, SigHandler, Signal};
use std::env;
use std::os::fd::BorrowedFd;
use std::os::unix::io::AsRawFd;
use std::process::{self, Command};
use std::thread;
use std::time::{Duration, Instant};

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

    thread::spawn(|| {
        thread::sleep(Duration::from_secs(120));
        let _ = std::fs::remove_file("/mnt/us/kindle-button-mapper/boot_attempts");
    });

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
        // The WAF helper sends this instead of restarting us, so the uinput
        // keyboard survives a mapping edit.
        signal(Signal::SIGHUP, SigHandler::Handler(reload::handle_sighup)).ok();
    }

    reload::publish(&config);

    // Virtual keyboard via uinput — kept alive for the daemon's lifetime by the
    // FIFO thread, which injects the keys scripts/key.sh asks for. Page turns
    // on a model with page buttons go into that node instead, so the FIFO is
    // worth serving even where uinput is missing.
    vkeyboard::serve(vkeyboard::try_init());

    // System-wide XKB override, set up once. First device that names a layout wins.
    let _layout = config
        .devices
        .iter()
        .find_map(|d| d.keyboard_layout.as_deref())
        .filter(|l| !l.is_empty())
        .and_then(LayoutOverride::new);

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
        if reload::claim(&device.id) {
            spawn_worker(device, settings.clone());
        }
    }

    // Reload thread doubles as the supervisor: a device added to the config
    // since startup gets its worker here rather than waiting for a restart.
    let path = config_path.to_string();
    loop {
        for device in reload::poll(&path) {
            info!("[{}] new in the config, starting a worker", device.id);
            spawn_worker(device, settings.clone());
        }
        thread::sleep(Duration::from_millis(200));
    }
}

fn spawn_worker(device: config::DeviceConfig, settings: WorkerSettings) {
    let id = device.id.clone();
    thread::Builder::new()
        .name(format!("dev:{}", id))
        .spawn(move || device_worker(device, settings))
        .expect("spawn device thread");
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

/// Why the event loop gave the device back.
enum Exit {
    Disconnected(String),
    /// The config no longer lists this device, so stop touching its node.
    Removed,
}

const PARK_POLL: Duration = Duration::from_millis(500);

/// Sit out until a reload says something about this device changed. Parking
/// rather than returning matters because a worker only ever holds a node it
/// should be holding, and an exiting one would leave the id claimed forever.
fn park_until_reconfigured(id: &str, generation: &mut u64) -> config::DeviceConfig {
    loop {
        thread::sleep(PARK_POLL);
        let current = reload::generation();
        if current == *generation {
            continue;
        }
        *generation = current;
        if let Some(cfg) = reload::device_config(id) {
            info!("[{}] config changed, picking the device back up", id);
            return cfg;
        }
    }
}

fn device_worker(mut cfg: config::DeviceConfig, settings: WorkerSettings) {
    let build = |cfg: &config::DeviceConfig| {
        Mapper::new(
            cfg,
            settings.debounce_ms,
            settings.long_press_ms,
            settings.repeat_ms,
            settings.log_buttons,
        )
    };
    let mut mapper = build(&cfg);
    let mut generation = reload::generation();

    loop {
        let handler = InputHandler::new(cfg.name.clone(), cfg.uniq.clone(), cfg.grab);
        match handler.open() {
            Ok((mut device, grabbed_at_open)) => {
                // Only the opened node says whether this is a pad or a
                // keyboard, and the two want opposite treatment.
                let gamepad = is_gamepad(&device);
                if cfg.is_unmapped() {
                    if gamepad {
                        cfg.apply_default_layout();
                        mapper = build(&cfg);
                    } else if cfg.keyboard_layout.is_none() {
                        info!(
                            "[{}] no mappings and not a gamepad — leaving it alone until something is mapped",
                            cfg.id
                        );
                        // Dropping the device releases the grab.
                        drop(device);
                        cfg = park_until_reconfigured(&cfg.id, &mut generation);
                        mapper = build(&cfg);
                        continue;
                    }
                }

                // Three ways to own a device. Grabbing it exclusively is right
                // for a pad, where an unmapped button should do nothing, and
                // wrong for a keyboard, where it would swallow every key this
                // config does not name. Passthrough is the middle ground: hold
                // the device, but relay what we don't map. Nothing to guess
                // when the file says so, otherwise decide from the node.
                let mut grab = effective_grab(&cfg, gamepad) && grabbed_at_open;
                if grabbed_at_open && !grab {
                    match device.ungrab() {
                        Ok(()) => info!("[{}] not a gamepad, sharing it instead of grabbing", cfg.id),
                        Err(e) => {
                            warn!("[{}] cannot release the grab: {}", cfg.id, e);
                            grab = true;
                        }
                    }
                }
                // Relaying keys we do not hold would deliver every keystroke
                // twice, once from us and once from whoever does hold it.
                if cfg.passthrough && !grab {
                    warn!(
                        "[{}] passthrough needs the exclusive grab and something else has it, relaying is off",
                        cfg.id
                    );
                    cfg.passthrough = false;
                    mapper = build(&cfg);
                }
                if grab && cfg.passthrough && !vkeyboard::available() {
                    warn!(
                        "[{}] passthrough is on but there is no uinput keyboard, unmapped keys will be lost",
                        cfg.id
                    );
                }

                info!(
                    "[{}] device connected ({})",
                    cfg.id,
                    match (grab, cfg.passthrough) {
                        (false, _) => "shared",
                        (true, true) => "exclusive, unmapped keys passed through",
                        (true, false) => "exclusive",
                    }
                );
                if let Some(ref script) = settings.on_connect {
                    info!("[{}] running on_connect script", cfg.id);
                    execute_script(script);
                }
                match run_event_loop(&mut device, &mut mapper, grab, gamepad,
                    &mut cfg, &mut generation, &settings) {
                    Exit::Removed => {
                        drop(device);
                        cfg = park_until_reconfigured(&cfg.id, &mut generation);
                        mapper = build(&cfg);
                        continue;
                    }
                    Exit::Disconnected(e) => {
                        error!("[{}] event loop error: {}", cfg.id, e);
                        if let Some(ref script) = settings.on_disconnect {
                            info!("[{}] device disconnected, running on_disconnect script", cfg.id);
                            execute_script(script);
                        }
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
    gamepad: bool,
    cfg: &mut config::DeviceConfig,
    generation: &mut u64,
    settings: &WorkerSettings,
) -> Exit {
    // Non-blocking + poll so we can notice a capture pause while idle.
    set_nonblocking(device.as_raw_fd());
    let mut grab = grab;
    let mut grabbed = grab;

    let mut last_poke: Option<Instant> = if settings.keep_awake {
        execute_script(KEEP_AWAKE_RELEASE);
        Some(Instant::now())
    } else {
        None
    };

    loop {
        let current = reload::generation();
        if current != *generation {
            *generation = current;
            match reload::device_config(&cfg.id) {
                Some(fresh) => {
                    info!("[{}] applying reloaded mappings", cfg.id);
                    *mapper = Mapper::new(
                        &fresh,
                        settings.debounce_ms,
                        settings.long_press_ms,
                        settings.repeat_ms,
                        settings.log_buttons,
                    );
                    // Switching who owns the device has to bite now rather
                    // than on the next reconnect, or the toggle looks broken.
                    let want = effective_grab(&fresh, gamepad);
                    if want != grab {
                        let changed = if want { device.grab() } else { device.ungrab() };
                        match changed {
                            Ok(()) => {
                                grab = want;
                                grabbed = want;
                                info!("[{}] now {}", cfg.id,
                                    if want { "holding the device" } else { "sharing the device" });
                            }
                            Err(e) => warn!("[{}] cannot change who holds the device: {}", cfg.id, e),
                        }
                    }
                    *cfg = fresh;
                }
                None => {
                    info!("[{}] gone from the config, releasing the device", cfg.id);
                    return Exit::Removed;
                }
            }
        }

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
            Err(e) => return Exit::Disconnected(format!("poll error: {}", e)),
        }

        let events = match device.fetch_events() {
            Ok(ev) => ev,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) => return Exit::Disconnected(format!("Read error: {}", e)),
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

        if settings.keep_awake && activity {
            let now = Instant::now();
            if last_poke.is_none_or(|t| now.duration_since(t) >= KEEP_AWAKE_INTERVAL) {
                info!("keep-awake: re-armed screensaver idle timer");
                execute_script(KEEP_AWAKE_POKE);
                last_poke = Some(now);
            }
        }
    }
}

/// Whether the daemon should hold this device exclusively. An explicit `grab`
/// in the file is taken at its word; without one only a gamepad is claimed,
/// since grabbing a keyboard would swallow every key nothing maps.
fn effective_grab(cfg: &config::DeviceConfig, gamepad: bool) -> bool {
    cfg.grab && (cfg.grab_explicit || gamepad)
}

fn is_gamepad(dev: &evdev::Device) -> bool {
    dev.supported_keys()
        .is_some_and(|k| k.contains(evdev::Key::BTN_SOUTH))
        || dev
            .supported_absolute_axes()
            .is_some_and(|a| a.contains(evdev::AbsoluteAxisType::ABS_HAT0X))
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

