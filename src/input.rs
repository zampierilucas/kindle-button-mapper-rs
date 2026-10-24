use evdev::{AbsoluteAxisType, AttributeSetRef, Device, Key, PropType};
use log::{debug, info, warn};
use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
use std::os::fd::AsFd;
use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

const INPUT_DIR: &str = "/dev/input";
const SETTLE_ATTEMPTS: u32 = 20;
const SETTLE_INTERVAL: Duration = Duration::from_millis(100);
const RESCAN_TIMEOUT_MS: u16 = 3000;

/// A Bluetooth node's uniq carries the address type as a suffix
/// ("E0:F6:B5:BC:1C:7F/P"), but configs hold the bare MAC in whatever case
/// the user typed. Compare only the address part, case-insensitively.
pub fn bare_addr(s: &str) -> String {
    s.split('/').next().unwrap_or("").to_ascii_uppercase()
}

pub fn uniq_matches(node: &str, want: &str) -> bool {
    !want.is_empty() && bare_addr(node) == bare_addr(want)
}

pub fn mappable_keys(keys: Option<&AttributeSetRef<Key>>, mouse: bool) -> usize {
    let mouse_buttons = Key::BTN_LEFT.code()..=Key::BTN_TASK.code();
    keys.map_or(0, |keys| {
        keys.iter().filter(|k| mouse_buttons.contains(&k.code()) == mouse).count()
    })
}

pub fn reports_position(axes: Option<&AttributeSetRef<AbsoluteAxisType>>) -> bool {
    axes.is_some_and(|axes| {
        (axes.contains(AbsoluteAxisType::ABS_X) && axes.contains(AbsoluteAxisType::ABS_Y))
            || axes.contains(AbsoluteAxisType::ABS_MT_POSITION_X)
    })
}

pub fn is_touchscreen(device: &Device) -> bool {
    device.properties().contains(PropType::DIRECT)
        || device
            .supported_absolute_axes()
            .is_some_and(|a| a.contains(AbsoluteAxisType::ABS_MT_POSITION_X))
}

/// A kernel that splits a device per HID application gives every node the
/// same uniq, and stick directions and gestures only come from a node that
/// reports a position. Fields compare in order, so serving the section's
/// position bindings outranks any number of keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct NodeRank {
    serves_position_bindings: bool,
    mappable_keys: usize,
}

impl NodeRank {
    fn new(binds_position: bool, position: bool, mappable_keys: usize) -> Self {
        Self {
            serves_position_bindings: binds_position && position,
            mappable_keys,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct InputHandler {
    device_name: Option<String>,
    device_uniq: Option<String>,
    grab: bool,
    mouse: bool,
    binds_position: bool,
}

impl InputHandler {
    pub fn new(
        device_name: Option<String>,
        device_uniq: Option<String>,
        grab: bool,
        mouse: bool,
    ) -> Self {
        Self {
            device_name,
            device_uniq,
            grab,
            mouse,
            binds_position: false,
        }
    }

    pub fn with_position(self, binds_position: bool) -> Self {
        Self {
            binds_position,
            ..self
        }
    }

    pub fn open(&self) -> Result<(Device, bool), String> {
        self.require_identity()?;

        if let Some(dev) = self.scan_for_device()? {
            return self.finish_open(dev);
        }

        info!("Waiting for device to appear...");
        let dev = self.wait_for_matching_device()?;
        self.finish_open(dev)
    }

    /// Every node of the device, for a frontend that has to hear a press
    /// whichever node it comes from.
    pub fn node_paths(&self) -> Result<Vec<PathBuf>, String> {
        self.require_identity()?;
        let entries =
            fs::read_dir(INPUT_DIR).map_err(|e| format!("Cannot open {INPUT_DIR}: {e}"))?;
        Ok(entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(OsStr::to_str)
                    .is_some_and(|name| name.starts_with("event"))
            })
            .filter(|path| Device::open(path).is_ok_and(|dev| self.matches_device(&dev)))
            .collect())
    }

    fn require_identity(&self) -> Result<(), String> {
        let has_identity = self.device_uniq.as_deref().is_some_and(|u| !u.is_empty())
            || self.device_name.as_deref().is_some_and(|n| !n.is_empty());
        if !has_identity {
            return Err("No device name or uniq specified".to_string());
        }
        Ok(())
    }

    fn matches_device(&self, dev: &Device) -> bool {
        if dev.name().unwrap_or("").starts_with("kindle-button-mapper") {
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

        let mut best: Option<(NodeRank, PathBuf, Device)> = None;

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
                    let keys = mappable_keys(dev.supported_keys(), self.mouse);
                    let position = reports_position(dev.supported_absolute_axes());
                    debug!(
                        "{path} matches, {keys} mappable keys, position: {position}",
                        path = path.display()
                    );
                    let rank = NodeRank::new(self.binds_position, position, keys);
                    if best.as_ref().is_none_or(|(top, _, _)| rank > *top) {
                        best = Some((rank, path, dev));
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
        let inotify = Inotify::init(InitFlags::IN_NONBLOCK)
            .map_err(|e| format!("inotify_init failed: {}", e))?;
        inotify.add_watch(Path::new(INPUT_DIR), AddWatchFlags::IN_CREATE)
            .map_err(|e| format!("inotify_add_watch failed: {}", e))?;

        // A device that appeared between the caller's scan and the watch
        // being added would never produce an event — scan once more.
        if let Some(dev) = self.scan_for_device()? {
            return Ok(dev);
        }

        loop {
            let mut fds = [PollFd::new(inotify.as_fd(), PollFlags::POLLIN)];
            let ready = poll(&mut fds, PollTimeout::from(RESCAN_TIMEOUT_MS))
                .map_err(|e| format!("poll on inotify failed: {}", e))?;

            if ready == 0 {
                // The settle window below can still miss: a uhid node's uniq
                // may be filled in later than that, and a node number reused
                // for a different device produces no second event. Without
                // this timeout rescan the worker would then wait for a
                // creation event that never comes, until the device
                // reconnected.
                if let Some(dev) = self.scan_for_device()? {
                    return Ok(dev);
                }
                continue;
            }

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
            // that race often enough to matter. Rescan over a short window
            // instead of trusting a single probe; the poll timeout above
            // covers whatever this window still misses.
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
        if device.name().unwrap_or("").starts_with("kindle-button-mapper") {
            return Err("Refusing to read our own virtual device".to_string());
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
    use super::{mappable_keys, reports_position, uniq_matches, NodeRank};
    use evdev::{AbsoluteAxisType, AttributeSet, Key};

    #[test]
    fn a_device_that_binds_a_stick_reads_the_node_that_reports_a_position() {
        // An F3 ring on 5.15: one node per HID application, both with its address.
        let xy = AttributeSet::from_iter([AbsoluteAxisType::ABS_X, AbsoluteAxisType::ABS_Y]);
        let touchscreen = AttributeSet::from_iter([Key::BTN_TOUCH, Key::BTN_TOOL_PEN, Key::BTN_0]);
        let consumer = AttributeSet::from_iter([
            Key::KEY_VOLUMEUP,
            Key::KEY_VOLUMEDOWN,
            Key::KEY_MUTE,
            Key::KEY_PLAYPAUSE,
        ]);
        let touchscreen_picked = |binds_position| {
            NodeRank::new(
                binds_position,
                reports_position(Some(&xy)),
                mappable_keys(Some(&touchscreen), false),
            ) > NodeRank::new(
                binds_position,
                reports_position(None),
                mappable_keys(Some(&consumer), false),
            )
        };

        assert!(touchscreen_picked(true));
        assert!(!touchscreen_picked(false));
    }

    #[test]
    fn a_panel_reports_a_position_and_a_lone_axis_does_not() {
        let multitouch = AttributeSet::from_iter([AbsoluteAxisType::ABS_MT_POSITION_X]);
        let wheel = AttributeSet::from_iter([AbsoluteAxisType::ABS_X]);
        let triggers =
            AttributeSet::from_iter([AbsoluteAxisType::ABS_GAS, AbsoluteAxisType::ABS_BRAKE]);

        assert!(reports_position(Some(&multitouch)));
        assert!(!reports_position(Some(&wheel)));
        assert!(!reports_position(Some(&triggers)));
        assert!(!reports_position(None));
    }

    #[test]
    fn a_mouse_node_loses_to_the_keyboard_node() {
        let mouse = AttributeSet::from_iter([Key::BTN_LEFT, Key::BTN_RIGHT, Key::BTN_MIDDLE]);
        let keyboard = AttributeSet::from_iter([Key::KEY_A, Key::KEY_PAGEUP, Key::KEY_PAGEDOWN]);
        let gamepad = AttributeSet::from_iter([Key::BTN_SOUTH, Key::BTN_EAST]);

        assert_eq!(mappable_keys(Some(&mouse), false), 0);
        assert_eq!(mappable_keys(Some(&keyboard), false), 3);
        assert_eq!(mappable_keys(Some(&gamepad), false), 2);
        assert_eq!(mappable_keys(None, false), 0);
    }

    #[test]
    fn a_mouse_device_picks_the_node_with_the_buttons() {
        let buttons = AttributeSet::from_iter([Key::BTN_LEFT, Key::BTN_RIGHT, Key::BTN_MIDDLE]);
        let consumer = AttributeSet::from_iter([Key::KEY_PLAYPAUSE, Key::KEY_VOLUMEUP]);

        assert!(mappable_keys(Some(&buttons), true) > mappable_keys(Some(&consumer), true));
        assert!(mappable_keys(Some(&buttons), false) < mappable_keys(Some(&consumer), false));
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
