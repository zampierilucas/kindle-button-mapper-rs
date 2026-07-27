use log::{info, warn};
use nix::{ioctl_none, ioctl_read_buf, ioctl_write_int};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::mem;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;

const UINPUT_DEV: &str = "/dev/uinput";
const TARGET_FILE: &str = "/var/run/kindle-button-mapper-key-target";
const SYSFS_INPUT: &str = "/sys/devices/virtual/input";
const DEV_INPUT: &str = "/dev/input";
const DEV_NAME: &[u8] = b"kindle-button-mapper";

const UINPUT_MAX_NAME_SIZE: usize = 80;
const ABS_CNT: usize = 64;
const EV_KEY: u32 = 0x01;

ioctl_none!(ui_dev_create, b'U', 1);
ioctl_write_int!(ui_set_evbit, b'U', 100);
ioctl_write_int!(ui_set_keybit, b'U', 101);
ioctl_read_buf!(ui_get_sysname, b'U', 44, u8);

#[repr(C)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
struct UinputUserDev {
    name: [u8; UINPUT_MAX_NAME_SIZE],
    id: InputId,
    ff_effects_max: u32,
    absmax: [i32; ABS_CNT],
    absmin: [i32; ABS_CNT],
    absfuzz: [i32; ABS_CNT],
    absflat: [i32; ABS_CNT],
}

pub fn try_init() -> Option<File> {
    if let Err(e) = ensure_uinput_node() {
        warn!("{} unavailable: {} — keyboard mappings will not inject events", UINPUT_DEV, e);
        return None;
    }

    let file = match create_device() {
        Ok(f) => f,
        Err(e) => {
            warn!("uinput device create failed: {} — keyboard mappings will not inject events", e);
            return None;
        }
    };

    match dev_node(&file) {
        Ok(path) => {
            let s = path.display().to_string();
            if let Err(e) = fs::write(TARGET_FILE, &s) {
                warn!("Cannot write {}: {}", TARGET_FILE, e);
            } else {
                info!("Virtual keyboard at {} (target written to {})", s, TARGET_FILE);
            }
        }
        Err(e) => warn!("Cannot resolve virtual keyboard node: {}", e),
    }

    Some(file)
}

fn create_device() -> io::Result<File> {
    let file = OpenOptions::new().read(true).write(true).open(UINPUT_DEV)?;
    let fd = file.as_raw_fd();

    unsafe { ui_set_evbit(fd, EV_KEY as _) }?;
    for code in supported_keys() {
        unsafe { ui_set_keybit(fd, code as _) }?;
    }

    // Setup via a uinput_user_dev write rather than UI_DEV_SETUP: that ioctl
    // only exists on Linux 4.5+, and Kindles through 10th gen run 3.0/4.1
    // kernels that answer it with EINVAL. The write path works on both.
    let mut setup = UinputUserDev {
        name: [0; UINPUT_MAX_NAME_SIZE],
        id: InputId {
            bustype: 0x03, // BUS_USB
            vendor: 0x1234,
            product: 0x5678,
            version: 0x111,
        },
        ff_effects_max: 0,
        absmax: [0; ABS_CNT],
        absmin: [0; ABS_CNT],
        absfuzz: [0; ABS_CNT],
        absflat: [0; ABS_CNT],
    };
    setup.name[..DEV_NAME.len()].copy_from_slice(DEV_NAME);

    let bytes = unsafe {
        std::slice::from_raw_parts(
            (&setup as *const UinputUserDev).cast::<u8>(),
            mem::size_of::<UinputUserDev>(),
        )
    };
    (&file).write_all(bytes)?;

    unsafe { ui_dev_create(fd) }?;
    Ok(file)
}

fn dev_node(file: &File) -> io::Result<PathBuf> {
    let mut buf = [0u8; 64];
    let len = unsafe { ui_get_sysname(file.as_raw_fd(), &mut buf) }? as usize;
    let bytes = &buf[..len.min(buf.len())];
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let sysname = std::str::from_utf8(&bytes[..end])
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let sysdir = Path::new(SYSFS_INPUT).join(sysname);
    for entry in fs::read_dir(&sysdir)? {
        let name = entry?.file_name();
        let name = name.to_string_lossy().into_owned();
        if !name.starts_with("event") {
            continue;
        }
        let path = Path::new(DEV_INPUT).join(&name);
        ensure_event_node(&path, &sysdir.join(&name))?;
        return Ok(path);
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("no event node under {}", sysdir.display()),
    ))
}

fn ensure_uinput_node() -> Result<(), String> {
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

fn ensure_event_node(path: &Path, sysdir: &Path) -> io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    let dev = fs::read_to_string(sysdir.join("dev"))?;
    let (major, minor) = dev
        .trim()
        .split_once(':')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("bad dev {:?}", dev)))?;
    let status = Command::new("mknod")
        .args([&path.display().to_string(), "c", major, minor])
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "mknod {} exit {}",
            path.display(),
            status.code().unwrap_or(-1)
        )));
    }
    let _ = Command::new("chmod").args(["600", &path.display().to_string()]).status();
    Ok(())
}

fn supported_keys() -> impl Iterator<Item = u32> {
    // All KEY_* codes. The BTN_* ranges (0x100-0x15f mouse/gamepad,
    // 0x2c0+ trigger-happy) are skipped so the device enumerates as a
    // plain keyboard.
    (1..0x100).chain(0x160..0x2c0)
}
