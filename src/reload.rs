//! In-process config reload.
//!
//! Restarting the daemon to pick up a config change destroys the uinput
//! keyboard, and anything holding that node open sees the device disappear.
//! KOReader reacts to that by rebuilding its UI, which throws the user out of
//! whatever menu they were editing the mapping from. So SIGHUP re-reads the
//! config in place instead, and the uinput node outlives it.

use crate::config::{Config, DeviceConfig};
use log::{info, warn};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

static REQUESTED: AtomicBool = AtomicBool::new(false);
/// Bumped once per applied reload. Workers compare against their own copy.
static GENERATION: AtomicU64 = AtomicU64::new(0);
static DEVICES: OnceLock<Mutex<Vec<DeviceConfig>>> = OnceLock::new();
/// The `[settings]` and `[gestures]` half of the file, so a reload refreshes
/// them too instead of every worker keeping its startup copy forever.
static SETTINGS: Mutex<Option<crate::WorkerSettings>> = Mutex::new(None);
/// Device ids that already have a worker. Only grows, so a device removed and
/// re-added does not end up with two threads fighting over the same node.
static WATCHED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn devices() -> &'static Mutex<Vec<DeviceConfig>> {
    DEVICES.get_or_init(|| Mutex::new(Vec::new()))
}

fn watched() -> &'static Mutex<HashSet<String>> {
    WATCHED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Claim a device id for a worker. False when someone already has it.
pub fn claim(id: &str) -> bool {
    watched()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(id.to_string())
}

/// Signal handler. Only an atomic store, which is async-signal-safe.
pub extern "C" fn handle_sighup(_: i32) {
    REQUESTED.store(true, Ordering::SeqCst);
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

pub fn publish(config: &Config) {
    *devices().lock().unwrap_or_else(|p| p.into_inner()) = config.devices.clone();
    *SETTINGS.lock().unwrap_or_else(|p| p.into_inner()) =
        Some(crate::WorkerSettings::from_config(config));
}

/// The global settings as of the last reload.
pub fn settings() -> Option<crate::WorkerSettings> {
    SETTINGS.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// The device's current config, or None if it was removed from the file.
pub fn device_config(id: &str) -> Option<DeviceConfig> {
    devices()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .find(|d| d.id == id)
        .cloned()
}

/// Re-read the config if SIGHUP asked for it, and hand back the devices that
/// nobody is watching yet so the caller can start a worker for each.
///
/// Pairing writes a new `[device.X]` block and then asks for this, so adding a
/// device no longer needs a restart. That matters beyond convenience: a restart
/// destroys the uinput keyboard, and KOReader stops delivering keys from a node
/// that disappears under it until it is itself restarted.
pub fn poll(config_path: &str) -> Vec<DeviceConfig> {
    if !REQUESTED.swap(false, Ordering::SeqCst) {
        return Vec::new();
    }
    match Config::load(config_path) {
        Ok(new) => {
            let fresh: Vec<DeviceConfig> = new
                .devices
                .iter()
                .filter(|d| claim(&d.id))
                .cloned()
                .collect();
            publish(&new);
            let gen = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
            info!("Config reloaded in place (generation {})", gen);
            fresh
        }
        Err(e) => {
            warn!("Reload failed, keeping the running config: {}", e);
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reload_refreshes_the_gestures_workers_read() {
        let mut cfg = Config::default();
        publish(&cfg);
        assert!(settings().expect("published").gesture_templates.is_empty());

        cfg.gesture_templates
            .push(("flick".into(), vec![(0.0, 0.0), (1.0, 1.0)]));
        cfg.gesture_tolerance = 0.42;
        publish(&cfg);

        let s = settings().expect("published");
        assert_eq!(s.gesture_templates.len(), 1);
        assert_eq!(s.gesture_tolerance, 0.42);
    }
}
