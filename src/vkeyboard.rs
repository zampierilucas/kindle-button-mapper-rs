use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
use evdev::{AttributeSet, Key};
use log::{info, warn};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const UINPUT_DEV: &str = "/dev/uinput";
const UINPUT_SYSFS: &str = "/sys/class/misc/uinput";
const TARGET_FILE: &str = "/var/run/kindle-button-mapper-key-target";

pub fn try_init() -> Option<VirtualDevice> {
    ensure_uinput_node().ok()?;

    let mut keys = AttributeSet::<Key>::new();
    for k in supported_keys() {
        keys.insert(k);
    }

    let dev = match VirtualDeviceBuilder::new()
        .and_then(|b| b.name(b"kindle-button-mapper").with_keys(&keys))
        .and_then(|b| b.build())
    {
        Ok(d) => d,
        Err(e) => {
            warn!("uinput device create failed: {} — keyboard mappings will not inject events", e);
            return None;
        }
    };

    let mut device = dev;
    if let Ok(mut paths) = device.enumerate_dev_nodes_blocking() {
        if let Some(Ok(path)) = paths.next() {
            let s = path.display().to_string();
            if let Err(e) = fs::write(TARGET_FILE, &s) {
                warn!("Cannot write {}: {}", TARGET_FILE, e);
            } else {
                info!("Virtual keyboard at {} (target written to {})", s, TARGET_FILE);
            }
        }
    }
    Some(device)
}

fn ensure_uinput_node() -> Result<(), String> {
    ensure_uinput_module();
    if Path::new(UINPUT_DEV).exists() {
        return Ok(());
    }
    // Kernel built with CONFIG_INPUT_UINPUT=y but no devtmpfs node — create it.
    let status = Command::new("mknod")
        .args([UINPUT_DEV, "c", "10", "223"])
        .status()
        .map_err(|e| format!("mknod missing: {}", e))?;
    if !status.success() {
        return Err(format!("mknod exit {}", status.code().unwrap_or(-1)));
    }
    let _ = Command::new("chmod").args(["600", UINPUT_DEV]).status();
    Ok(())
}

// Most Kindles ship CONFIG_INPUT_UINPUT built in. The Oasis 3 doesn't, so the
// mknod'd node opens with ENODEV until the driver is loaded. Best effort: if the
// driver isn't registered, insmod every bundled uinput-*.ko that matches the
// running kernel until one takes.
fn ensure_uinput_module() {
    if Path::new(UINPUT_SYSFS).exists() {
        return;
    }
    let release = kernel_release();
    let entries = match fs::read_dir(module_dir()) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !name.starts_with("uinput-") || !name.ends_with(".ko") {
            continue;
        }
        if !release.is_empty() && !name.contains(&release) {
            continue;
        }
        info!("uinput driver missing, trying module {}", name);
        let _ = Command::new("insmod").arg(&path).status();
        if Path::new(UINPUT_SYSFS).exists() {
            info!("uinput driver loaded from {}", name);
            return;
        }
    }
    warn!("uinput driver unavailable and no bundled module loaded — keyboard mappings will not inject events");
}

fn kernel_release() -> String {
    fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn module_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("modules")))
        .unwrap_or_else(|| PathBuf::from("/mnt/us/kindle-button-mapper/modules"))
}

fn supported_keys() -> impl Iterator<Item = Key> {
    // All KEY_* codes. The BTN_* ranges (0x100-0x15f mouse/gamepad,
    // 0x2c0+ trigger-happy) are skipped so the device enumerates as a
    // plain keyboard.
    (1..0x100).chain(0x160..0x2c0).map(Key::new)
}
