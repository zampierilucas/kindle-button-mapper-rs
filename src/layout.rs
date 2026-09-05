use log::{info, warn};
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

const XKB_DIR: &str = "/usr/share/X11/xkb";
const US: &str = "/usr/share/X11/xkb/symbols/us";
const OVERRIDE: &str = "/var/local/kbm-us";

/// Bind-mounts a generated `us` symbols file over the read-only system one, so
/// the framework's `pc+us` re-pin on focus loads the user's layout.
pub struct LayoutOverride;

impl LayoutOverride {
    pub fn new(layout: &str) -> Option<Self> {
        let groups: Vec<&str> = layout.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
        if groups.is_empty() {
            return None;
        }
        if fs::write(OVERRIDE, symbols(&groups)).is_err() {
            warn!("layout: cannot write {OVERRIDE}");
            return None;
        }
        let _ = run("umount", &[US]);
        if !run("mount", &["--bind", OVERRIDE, US]) {
            warn!("layout '{layout}': bind-mount over us failed");
            return None;
        }
        load();
        info!("layout '{layout}' active");
        Some(LayoutOverride)
    }
}

impl Drop for LayoutOverride {
    fn drop(&mut self) {
        if run("umount", &[US]) {
            load();
        }
    }
}

pub fn set_key_repeat(delay_ms: u32, rate: u32) {
    let delay = delay_ms.to_string();
    let rate = rate.to_string();
    if run("xset", &["-display", ":0", "r", "rate", &delay, &rate]) {
        info!("key repeat {delay_ms}ms then {rate}/s");
    } else {
        warn!("key repeat {delay_ms},{rate}: xset failed");
    }
}

fn symbols(groups: &[&str]) -> String {
    let mut s = String::from("default partial alphanumeric_keys modifier_keys\nxkb_symbols \"basic\" {\n");
    for (i, g) in groups.iter().enumerate() {
        // `us` is the file we shadow, so include its `latin` base instead of recursing.
        let name = if *g == "us" { "latin" } else { g };
        if i == 0 {
            s.push_str(&format!("    include \"{name}\"\n"));
        } else {
            s.push_str(&format!("    include \"{name}:{}\"\n", i + 1));
        }
    }
    if groups.len() > 1 {
        s.push_str("    include \"group(alt_shift_toggle)\"\n");
    }
    s.push_str("};\n");
    s
}

fn load() {
    let keymap = "xkb_keymap { xkb_keycodes { include \"evdev+aliases(qwerty)\" }; \
                  xkb_types { include \"complete\" }; xkb_compat { include \"complete\" }; \
                  xkb_symbols { include \"pc+us\" }; xkb_geometry { include \"pc(pc105)\" }; };\n";
    if let Ok(mut child) = Command::new("xkbcomp")
        .args([&format!("-I{XKB_DIR}"), "-", ":0"])
        .stdin(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(keymap.as_bytes());
        }
        let _ = child.wait();
    }
}

fn run(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd)
        .args(args)
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
