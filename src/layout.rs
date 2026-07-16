use log::{error, info, warn};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const XKB_DISPLAY: &str = ":0";
const XKB_BASE: &str = "/usr/share/X11/xkb";
const REASSERT_THROTTLE: Duration = Duration::from_millis(120);

/// Precompiles a layout to a .xkm once, then re-asserts it cheaply on demand.
///
/// The Kindle framework re-pins the `us` core keymap whenever an input context
/// gains focus, clobbering a one-shot layout set at device connect. So we hold
/// a precompiled keymap and reload it into the core map at typing-burst start.
pub struct LayoutAsserter {
    xkm: PathBuf,
    last_assert: Option<Instant>,
}

impl LayoutAsserter {
    /// Compile `pc+<layout>` to a cached .xkm. `layout` may be a combo like
    /// "ru+kz:2+group(alt_shift_toggle)". Returns None if it fails to compile
    /// (e.g. the symbols file is missing from /usr/share/X11/xkb/symbols).
    pub fn new(id: &str, layout: &str) -> Option<Self> {
        let keymap = format!(
            "xkb_keymap {{\n\
             \x20 xkb_keycodes {{ include \"evdev+aliases(qwerty)\" }};\n\
             \x20 xkb_types    {{ include \"complete\" }};\n\
             \x20 xkb_compat   {{ include \"complete\" }};\n\
             \x20 xkb_symbols  {{ include \"pc+{layout}\" }};\n\
             \x20 xkb_geometry {{ include \"pc(pc105)\" }};\n\
             }};\n"
        );
        let xkm = PathBuf::from(format!("/tmp/kbm-layout-{id}.xkm"));

        let mut child = Command::new("xkbcomp")
            .args([
                &format!("-I{XKB_BASE}"),
                "-xkm",
                "-",
                xkm.to_str()?,
            ])
            .stdin(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| error!("[{id}] xkbcomp precompile failed to start: {e}"))
            .ok()?;
        child.stdin.take()?.write_all(keymap.as_bytes()).ok()?;
        if !matches!(child.wait(), Ok(s) if s.success()) {
            warn!("[{id}] layout '{layout}' failed to compile (missing symbols file?)");
            return None;
        }
        info!("[{id}] layout '{layout}' precompiled to {}", xkm.display());
        Some(Self {
            xkm,
            last_assert: None,
        })
    }

    /// Reload the precompiled .xkm into the core keymap. Cheap: no symbol
    /// compilation, just a keymap load. Throttled so an auto-repeat can't spawn
    /// a storm of xkbcomp processes.
    pub fn assert(&mut self) {
        if let Some(t) = self.last_assert {
            if t.elapsed() < REASSERT_THROTTLE {
                return;
            }
        }
        let ok = Command::new("xkbcomp")
            .args([self.xkm.to_str().unwrap_or(""), XKB_DISPLAY])
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            warn!("layout re-assert failed");
        }
        self.last_assert = Some(Instant::now());
    }
}
