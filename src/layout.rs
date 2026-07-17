use log::{error, info, warn};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const XKB_DISPLAY: &str = ":0";
const XKB_BASE: &str = "/usr/share/X11/xkb";
const US_SYMBOLS: &str = "/usr/share/X11/xkb/symbols/us";
const OVERRIDE_FILE: &str = "/var/local/kbm-us-symbols";

/// Forces the user's keyboard layout by overriding the system `us` XKB symbols.
///
/// The Kindle framework re-pins the `pc+us` core keymap whenever an input
/// context gains focus, so a layout set once on the running server never sticks.
/// `/usr/share/X11/xkb` is a read-only squashfs, so `us` can't be edited in
/// place either. Instead we generate a replacement `us` symbols file and
/// bind-mount it over the original: every `pc+us` compile — including the
/// framework's re-pin — then resolves to the user's layout. Reverted by
/// unmounting (the upstart `post-stop` job and `Drop` both do this).
pub struct LayoutOverride {
    mounted: bool,
}

impl LayoutOverride {
    /// `layout` is a comma-separated list of XKB layout names: the first is the
    /// default group, any others become extra groups toggled with Alt+Shift.
    /// e.g. `"ru"`, `"us,ru"`, `"de(nodeadkeys)"`.
    pub fn new(layout: &str) -> Option<Self> {
        let groups: Vec<&str> = layout
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if groups.is_empty() {
            return None;
        }

        let path = PathBuf::from(OVERRIDE_FILE);
        if let Err(e) = fs::write(&path, build_symbols(&groups)) {
            error!("cannot write layout override {}: {e}", path.display());
            return None;
        }

        // Drop any stale override left mounted by a previous run before stacking
        // a fresh one on top of it.
        clear_mount();
        if !bind_over_us(&path) {
            warn!("layout '{layout}': bind-mount over us failed — is /usr/share/X11/xkb present?");
            return None;
        }
        if reload_core_keymap() {
            info!("layout '{layout}' active (us overridden via bind-mount)");
        } else {
            warn!("layout '{layout}' bound, but the running server did not reload it");
        }
        Some(Self { mounted: true })
    }
}

impl Drop for LayoutOverride {
    fn drop(&mut self) {
        // SIGTERM exits via _exit(), which skips Drop — the upstart post-stop
        // job unmounts in that path. This covers graceful drops (e.g. tests).
        if self.mounted && clear_mount() {
            let _ = reload_core_keymap();
            info!("layout override removed; stock us restored");
        }
    }
}

/// Builds a `us` symbols file whose default section is the requested layout(s).
fn build_symbols(groups: &[&str]) -> String {
    let mut body = String::new();
    for (i, g) in groups.iter().enumerate() {
        // `us` is the very file we shadow, so `include "us"` would recurse into
        // this override. Its Latin base lives in the separate `latin` file.
        let name = if *g == "us" { "latin" } else { *g };
        if i == 0 {
            body.push_str(&format!("    include \"{name}\"\n"));
        } else {
            body.push_str(&format!("    include \"{name}:{}\"\n", i + 1));
        }
    }
    if groups.len() > 1 {
        body.push_str("    include \"group(alt_shift_toggle)\"\n");
    }
    format!(
        "default partial alphanumeric_keys modifier_keys\n\
         xkb_symbols \"basic\" {{\n\
         {body}}};\n"
    )
}

fn bind_over_us(src: &Path) -> bool {
    let src = match src.to_str() {
        Some(s) => s,
        None => return false,
    };
    Command::new("mount")
        .args(["--bind", src, US_SYMBOLS])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn clear_mount() -> bool {
    Command::new("umount")
        .arg(US_SYMBOLS)
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Recompiles `pc+us` (now resolving through the override) and loads it into the
/// running X server, so the change takes hold immediately rather than only on
/// the framework's next re-pin.
fn reload_core_keymap() -> bool {
    let keymap = "xkb_keymap {\n\
         \x20 xkb_keycodes { include \"evdev+aliases(qwerty)\" };\n\
         \x20 xkb_types    { include \"complete\" };\n\
         \x20 xkb_compat   { include \"complete\" };\n\
         \x20 xkb_symbols  { include \"pc+us\" };\n\
         \x20 xkb_geometry { include \"pc(pc105)\" };\n\
         };\n";
    let mut child = match Command::new("xkbcomp")
        .args([&format!("-I{XKB_BASE}"), "-", XKB_DISPLAY])
        .stdin(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            error!("xkbcomp reload failed to start: {e}");
            return false;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(keymap.as_bytes()).is_err() {
            return false;
        }
    }
    matches!(child.wait(), Ok(s) if s.success())
}
