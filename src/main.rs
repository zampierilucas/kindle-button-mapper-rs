mod action;
mod config;
mod input;
mod kolayout;
mod keysym;
mod koreader;
mod layout;
mod mapper;
mod pause;
mod reload;
mod vkeyboard;
mod xkey;
mod gesture;
mod waf_helper;

use config::Config;
use std::collections::HashMap;
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
        "Config: devices={}, gestures={}, debounce={}ms, long_press={}ms, repeat={}ms",
        config.devices.len(),
        config.gesture_templates.len(),
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

    let settings = WorkerSettings::from_config(&config);
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
pub(crate) struct WorkerSettings {
    debounce_ms: u64,
    long_press_ms: u64,
    repeat_ms: u64,
    log_buttons: bool,
    stick_deadzone: i32,
    gesture_min_percent: i32,
    gesture_tolerance: f32,
    gesture_templates: Vec<(String, Vec<gesture::Point>)>,
    keep_awake: bool,
    on_connect: Option<String>,
    on_disconnect: Option<String>,
}

impl WorkerSettings {
    pub(crate) fn from_config(c: &config::Config) -> Self {
        Self {
            debounce_ms: c.debounce_ms,
            long_press_ms: c.long_press_ms,
            repeat_ms: c.repeat_ms,
            log_buttons: c.log_buttons,
            stick_deadzone: c.stick_deadzone,
            gesture_min_percent: c.gesture_min_percent,
            gesture_tolerance: c.gesture_tolerance,
            gesture_templates: c.gesture_templates.clone(),
            keep_awake: c.keep_awake,
            on_connect: c.on_connect.clone(),
            on_disconnect: c.on_disconnect.clone(),
        }
    }
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

fn device_worker(mut cfg: config::DeviceConfig, mut settings: WorkerSettings) {
    let mut mapper = Mapper::new(&cfg, &settings);
    let mut generation = reload::generation();

    loop {
        let handler = InputHandler::new(cfg.name.clone(), cfg.uniq.clone(), cfg.grab, cfg.mouse);
        match handler.open() {
            Ok((mut device, grabbed_at_open)) => {
                // Only the opened node says whether this is a pad or a
                // keyboard, and the two want opposite treatment.
                let gamepad = is_gamepad(&device);
                if cfg.is_unmapped() {
                    if gamepad {
                        cfg.apply_default_layout();
                        mapper = Mapper::new(&cfg, &settings);
                    } else if cfg.keyboard_layout.is_none() {
                        info!(
                            "[{}] no mappings and not a gamepad — leaving it alone until something is mapped",
                            cfg.id
                        );
                        // Dropping the device releases the grab.
                        drop(device);
                        cfg = park_until_reconfigured(&cfg.id, &mut generation);
                        mapper = Mapper::new(&cfg, &settings);
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
                if downgrade_relay(&mut cfg, grab) {
                    mapper = Mapper::new(&cfg, &settings);
                }
                if grab && cfg.passthrough && !vkeyboard::available() {
                    warn!(
                        "[{}] passthrough is on but there is no uinput keyboard, unmapped keys will be lost",
                        cfg.id
                    );
                }
                if grab && cfg.mouse && !vkeyboard::pointer_ready() {
                    warn!(
                        "[{}] no uinput pointer, the cursor will not move while we hold the mouse",
                        cfg.id
                    );
                }

                info!(
                    "[{}] device connected ({})",
                    cfg.id,
                    match (grab, cfg.mouse, cfg.passthrough) {
                        (false, _, _) => "shared",
                        (true, true, _) => "exclusive, motion and unmapped buttons relayed",
                        (true, false, true) => "exclusive, unmapped keys passed through",
                        (true, false, false) => "exclusive",
                    }
                );
                if let Some(ref script) = settings.on_connect {
                    info!("[{}] running on_connect script", cfg.id);
                    execute_script(script);
                }
                match run_event_loop(&mut device, &mut mapper, grab, gamepad,
                    &mut cfg, &mut generation, &mut settings) {
                    Exit::Removed => {
                        drop(device);
                        cfg = park_until_reconfigured(&cfg.id, &mut generation);
                        mapper = Mapper::new(&cfg, &settings);
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
    settings: &mut WorkerSettings,
) -> Exit {
    // Non-blocking + poll so we can notice a capture pause while idle.
    set_nonblocking(device.as_raw_fd());
    let ranges = stick_ranges(device);
    // ABS_X/ABS_Y mean a contact on a touch device and a stick on a pad, so
    // the device decides which once, not every event.
    let touch_device = device
        .supported_keys()
        .is_some_and(|k| k.contains(evdev::Key::BTN_TOUCH));
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
                Some(mut fresh) => {
                    info!("[{}] applying reloaded mappings", cfg.id);
                    if let Some(globals) = reload::settings() {
                        *settings = globals;
                    }
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
                    downgrade_relay(&mut fresh, grab);
                    *mapper = Mapper::new(&fresh, settings);
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

        let relay = cfg.mouse && grab;

        let mut activity = false;
        for event in events {
            match event.kind() {
                InputEventKind::Synchronization(_) => {
                    mapper.handle_sync();
                    if relay {
                        vkeyboard::pointer_sync();
                    }
                }
                InputEventKind::Key(key) if key.code() == 330 => {  // BTN_TOUCH
                    activity = true;
                    mapper.handle_touch(event.value() == 1);
                }
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
                        0 | 1 if touch_device => {
                            activity = true;
                            mapper.handle_touch_move(code == 0, stick_percent(&ranges, code, event.value()));
                        }
                        0 | 1 | 3 | 4 => {
                            activity = true;
                            mapper.handle_stick(code, stick_percent(&ranges, code, event.value()));
                        }
                        53 | 54 => {
                            activity = true;
                            mapper.handle_touch_move(code == 53, stick_percent(&ranges, code, event.value()));
                        }
                        _ => {}
                    }
                }
                InputEventKind::RelAxis(axis) if relay => {
                    activity = true;
                    vkeyboard::pointer_motion(axis.0, event.value());
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
/// in the file is taken at its word; without one only a gamepad or a mouse is
/// claimed, since grabbing a keyboard would swallow every key nothing maps.
fn effective_grab(cfg: &config::DeviceConfig, gamepad: bool) -> bool {
    cfg.grab && (cfg.grab_explicit || gamepad || cfg.mouse)
}

/// Relaying a device we do not hold would deliver everything twice. True when
/// something was turned off and the mapper has to be rebuilt.
fn downgrade_relay(cfg: &mut config::DeviceConfig, grab: bool) -> bool {
    if grab || !(cfg.passthrough || cfg.mouse) {
        return false;
    }
    warn!(
        "[{}] relaying needs the exclusive grab and something else has it, it is off",
        cfg.id
    );
    cfg.passthrough = false;
    cfg.mouse = false;
    true
}

/// Centre and half-travel per axis; pads disagree on range, the mapper sees a percentage.
fn stick_ranges(dev: &evdev::Device) -> HashMap<u16, (i32, i32)> {
    let states = match dev.get_abs_state() {
        Ok(s) => s,
        Err(_) => return HashMap::new(),
    };
    [0u16, 1, 3, 4, 53, 54]
        .iter()
        .filter_map(|&code| {
            let i = states.get(code as usize)?;
            let half = ((i.maximum - i.minimum) / 2).max(1);
            Some((code, ((i.minimum + i.maximum) / 2, half)))
        })
        .collect()
}

fn stick_percent(ranges: &HashMap<u16, (i32, i32)>, code: u16, value: i32) -> i32 {
    match ranges.get(&code) {
        Some((centre, half)) => ((value - centre) * 100 / half).clamp(-100, 100),
        None => 0,
    }
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

