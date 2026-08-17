use evdev::{AttributeSetRef, Device, Key};
use log::{debug, info, warn};
use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

const INPUT_DIR: &str = "/dev/input";
const SETTLE_ATTEMPTS: u32 = 20;
const SETTLE_INTERVAL: Duration = Duration::from_millis(100);

/// A Bluetooth node's uniq carries the address type as a suffix
/// ("E0:F6:B5:BC:1C:7F/P"), but configs hold the bare MAC in whatever case
/// the user typed. Compare only the address part, case-insensitively.
pub fn uniq_matches(node: &str, want: &str) -> bool {
    let bare = |s: &str| s.split('/').next().unwrap_or("").to_ascii_uppercase();
    !want.is_empty() && bare(node) == bare(want)
}

fn mappable_keys(keys: Option<&AttributeSetRef<Key>>) -> usize {
    let mouse_buttons = Key::BTN_LEFT.code()..=Key::BTN_TASK.code();
    keys.map_or(0, |keys| {
        keys.iter().filter(|k| !mouse_buttons.contains(&k.code())).count()
    })
}

pub struct InputHandler {
    device_name: Option<String>,
    device_uniq: Option<String>,
    grab: bool,
}

impl InputHandler {
    pub fn new(
        device_name: Option<String>,
        device_uniq: Option<String>,
        grab: bool,
    ) -> Self {
        Self {
            device_name,
            device_uniq,
            grab,
        }
    }

    pub fn open(&self) -> Result<(Device, bool), String> {
        let has_identity = self.device_uniq.as_deref().is_some_and(|u| !u.is_empty())
            || self.device_name.as_deref().is_some_and(|n| !n.is_empty());
        if !has_identity {
            return Err("No device name or uniq specified".to_string());
        }

        if let Some(dev) = self.scan_for_device()? {
            return self.finish_open(dev);
        }

        info!("Waiting for device to appear...");
        let dev = self.wait_for_matching_device()?;
        self.finish_open(dev)
    }

    fn matches_device(&self, dev: &Device) -> bool {
        if dev.name().unwrap_or("") == "kindle-button-mapper" {
            return false;
        }
        // uniq (MAC) is stable across renames and reconnects — match on it
        // alone when set, so the device name is just a label.
        if let Some(ref uniq) = self.device_uniq {
            if !uniq.is_empty() {
                return uniq_matches(dev.unique_name().unwrap_or(""), uniq);
            }
        }
        if let Some(ref name) = self.device_name {
            if !name.is_empty() {
                return dev.name().unwrap_or("") == name.as_str();
            }
        }
        true
    }

    fn scan_for_device(&self) -> Result<Option<Device>, String> {
        let entries = fs::read_dir(INPUT_DIR)
            .map_err(|e| format!("Cannot open {}: {}", INPUT_DIR, e))?;

        let mut best: Option<(usize, PathBuf, Device)> = None;

        for entry in entries.flatten() {
            let path = entry.path();
            let filename = path.file_name().and_then(OsStr::to_str).unwrap_or("");
            if !filename.starts_with("event") {
                continue;
            }
            match Device::open(&path) {
                Ok(dev) => {
                    debug!("Scanning {}: name={:?} uniq={:?}",
                        path.display(),
                        dev.name().unwrap_or(""),
                        dev.unique_name().unwrap_or(""));
                    if !self.matches_device(&dev) {
                        continue;
                    }
                    let keys = mappable_keys(dev.supported_keys());
                    debug!("{} matches, {} mappable keys", path.display(), keys);
                    if best.as_ref().is_none_or(|(most, _, _)| keys > *most) {
                        best = Some((keys, path, dev));
                    }
                }
                Err(e) => {
                    debug!("Cannot open {}: {}", path.display(), e);
                }
            }
        }
        Ok(best.map(|(_, path, dev)| {
            info!("Found device at {}", path.display());
            dev
        }))
    }

    fn wait_for_matching_device(&self) -> Result<Device, String> {
        let inotify = Inotify::init(InitFlags::empty())
            .map_err(|e| format!("inotify_init failed: {}", e))?;
        inotify.add_watch(Path::new(INPUT_DIR), AddWatchFlags::IN_CREATE)
            .map_err(|e| format!("inotify_add_watch failed: {}", e))?;

        // A device that appeared between the caller's scan and the watch
        // being added would never produce an event — scan once more.
        if let Some(dev) = self.scan_for_device()? {
            return Ok(dev);
        }

        loop {
            let events = inotify.read_events()
                .map_err(|e| format!("inotify read failed: {}", e))?;

            let new_node = events.iter().any(|e| {
                e.name.as_ref().is_some_and(|n| n.to_string_lossy().starts_with("event"))
            });
            if !new_node {
                continue;
            }

            // A node is not usable the instant it appears: udev still has to
            // apply permissions, and a uhid node's uniq is filled in after
            // the node shows up. Probing once right after the event loses
            // that race often enough to matter, and since the event is
            // consumed the device would then be ignored until it connects
            // again, which is why it took a power cycle to come good. Rescan
            // over a short window instead of trusting a single probe.
            for _ in 0..SETTLE_ATTEMPTS {
                thread::sleep(SETTLE_INTERVAL);
                if let Some(dev) = self.scan_for_device()? {
                    return Ok(dev);
                }
            }
            debug!("New input node did not match within the settle window");
        }
    }

    /// The device, and whether the exclusive grab was actually taken. A failed
    /// grab is not fatal, but the caller has to know: whoever holds it still
    /// gets the events, so anything that assumes exclusivity would double up.
    fn finish_open(&self, mut device: Device) -> Result<(Device, bool), String> {
        if device.name().unwrap_or("") == "kindle-button-mapper" {
            return Err("Refusing to read our own virtual keyboard".to_string());
        }
        let mut grabbed = false;
        if self.grab {
            match device.grab() {
                Ok(()) => {
                    grabbed = true;
                    info!("Grabbed device exclusively");
                }
                Err(e) => warn!("Cannot grab device: {}, continuing without exclusive access", e),
            }
        } else {
            info!("Exclusive grab disabled, sharing device");
        }
        info!("Reading events from {} (uniq={:?})",
            device.name().unwrap_or("?"),
            device.unique_name().unwrap_or(""));
        Ok((device, grabbed))
    }
}

#[cfg(test)]
mod tests {
    use super::{mappable_keys, uniq_matches};
    use evdev::{AttributeSet, Key};

    #[test]
    fn a_mouse_node_loses_to_the_keyboard_node() {
        let mouse = AttributeSet::from_iter([Key::BTN_LEFT, Key::BTN_RIGHT, Key::BTN_MIDDLE]);
        let keyboard = AttributeSet::from_iter([Key::KEY_A, Key::KEY_PAGEUP, Key::KEY_PAGEDOWN]);
        let gamepad = AttributeSet::from_iter([Key::BTN_SOUTH, Key::BTN_EAST]);

        assert_eq!(mappable_keys(Some(&mouse)), 0);
        assert_eq!(mappable_keys(Some(&keyboard)), 3);
        assert_eq!(mappable_keys(Some(&gamepad)), 2);
        assert_eq!(mappable_keys(None), 0);
    }

    #[test]
    fn uniq_ignores_suffix_and_case() {
        assert!(uniq_matches("E0:F6:B5:BC:1C:7F/P", "E0:F6:B5:BC:1C:7F"));
        assert!(uniq_matches("E0:F6:B5:BC:1C:7F/P", "e0:f6:b5:bc:1c:7f"));
        assert!(uniq_matches("E0:F6:B5:BC:1C:7F", "E0:F6:B5:BC:1C:7F/P"));
        assert!(!uniq_matches("E0:F6:B5:BC:1C:7F/P", "AA:BB:CC:DD:EE:FF"));
        assert!(!uniq_matches("", "AA:BB:CC:DD:EE:FF"));
        assert!(!uniq_matches("E0:F6:B5:BC:1C:7F/P", ""));
    }
}
