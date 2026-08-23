//! Layout translation for KOReader. It reads the keyboard as raw evdev and
//! maps it through a hardcoded US table, so the XKB override in `layout.rs`
//! never reaches it. The layout itself comes from the system: `xkbcomp`
//! resolves the same keymap X is running and prints every level as text, so a
//! new language needs no data here. Characters go to KOReader as TextInput
//! events over its HTTP Inspector; keys the layout leaves alone relay as
//! keycodes, which keeps letters, arrows and shortcuts working unchanged.

use crate::keysym;
use crate::koreader;
use log::{info, warn};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

const XKB_DIR: &str = "/usr/share/X11/xkb";
const KEY_LEFTSHIFT: u16 = 42;
const KEY_RIGHTSHIFT: u16 = 54;
const KEY_RIGHTALT: u16 = 100;
const KEY_SPACE: u16 = 57;
/// X numbers keycodes 8 higher than the kernel does.
const KEYCODE_OFFSET: u16 = 8;

#[derive(Clone, PartialEq, Debug)]
enum Level {
    /// Relayed as a keycode. KOReader already types these correctly, but the
    /// character is kept: it is still what a pending dead key composes with.
    Pass(Option<char>),
    Text(String),
    /// The character it types alone, and its combining mark.
    Dead(char, char),
}

pub struct KoLayout {
    keys: HashMap<u16, [Level; 4]>,
    shift: bool,
    altgr: bool,
    pending: Option<(char, char)>,
    /// Codes whose press we took, so their release is not relayed either.
    held: HashSet<u16>,
}

impl KoLayout {
    pub fn load(layout: &str) -> Option<Self> {
        // Once per process. The bind-mount is set up before any worker starts
        // and never changes, so re-running xkbcomp per device and per config
        // reload would only reparse the whole xkb tree for the same answer.
        static DUMP: OnceLock<Option<String>> = OnceLock::new();
        let dump = DUMP.get_or_init(dump).as_deref()?;
        let parsed = Self::from_xkb(dump);
        // from_xkb only keeps a key when some level is claimed, so this is the
        // whole map. Counting level 1 alone would miss a layout that only
        // differs under Shift or AltGr and reject it outright.
        let claimed = parsed.keys.len();
        if claimed == 0 {
            warn!("layout '{layout}' resolved to nothing KOReader needs, typing there stays US");
            return None;
        }
        info!("KOReader layout '{layout}' resolved from XKB, {claimed} keys claimed");
        Some(parsed)
    }

    fn from_xkb(dump: &str) -> Self {
        // Two shapes matter, and neither collides with the other sections:
        //   <AC10> = 47;                          in xkb_keycodes
        //   key <AC10> { ... [ ccedilla, ... ] }  in xkb_symbols
        let mut codes: HashMap<&str, u16> = HashMap::new();
        let mut symbols: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut in_symbols = false;
        let mut current = "";

        for line in dump.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("xkb_") {
                in_symbols = rest.starts_with("symbols");
                continue;
            }
            if !in_symbols {
                if let Some((name, value)) = angled(line).zip(line.split('=').nth(1)) {
                    if let Ok(code) = value.trim().trim_end_matches(';').parse::<u16>() {
                        codes.insert(name, code);
                    }
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("key ") {
                current = angled(rest).unwrap_or("");
            }
            // Either the one-line form or a following symbols[Group1] line.
            // Later groups are ignored: KOReader has no group toggle.
            if !current.is_empty() && (line.starts_with("key ") || line.starts_with("symbols[Group1]")) {
                // After the last '=', so `symbols[Group1]` is not mistaken for
                // the list itself.
                let tail = line.rsplit_once('=').map_or(line, |(_, r)| r);
                if let Some(list) = between(tail, '[', ']') {
                    symbols.insert(current, list.split(',').map(str::trim).collect());
                    current = "";
                }
            }
        }

        let mut keys = HashMap::new();
        for (name, list) in symbols {
            let Some(&x_code) = codes.get(name) else { continue };
            let Some(code) = x_code.checked_sub(KEYCODE_OFFSET) else { continue };
            let mut levels = [Level::Pass(None), Level::Pass(None), Level::Pass(None), Level::Pass(None)];
            for (idx, slot) in levels.iter_mut().enumerate() {
                let Some(sym) = list.get(idx) else { break };
                *slot = resolve(code, idx, sym);
            }
            if levels.iter().any(|l| !l.is_pass()) {
                keys.insert(code, levels);
            }
        }
        Self {
            keys,
            shift: false,
            altgr: false,
            pending: None,
            held: HashSet::new(),
        }
    }

    /// True when the keystroke went to KOReader as text and must not also be
    /// relayed as a keycode. False leaves the caller's relay path untouched,
    /// which is also what happens whenever KOReader is not running, so the
    /// XKB override keeps serving X.
    pub fn consume(&mut self, code: u16, value: i32) -> bool {
        // Tracked here but still relayed: KOReader needs Shift itself for the
        // letters that go through as keycodes.
        match code {
            KEY_LEFTSHIFT | KEY_RIGHTSHIFT => {
                self.shift = value != 0;
                return false;
            }
            KEY_RIGHTALT => {
                self.altgr = value != 0;
                return false;
            }
            _ => {}
        }

        if value == 0 {
            return self.held.remove(&code);
        }

        if let Some((spacing, mark)) = self.pending.take() {
            return match self.compose(spacing, mark, code) {
                Some(text) => self.emit(code, &text),
                // Not a character key. Flush the accent on its own and let the
                // key itself relay as usual.
                None => {
                    koreader::send_text(&spacing.to_string());
                    false
                }
            };
        }

        match self.level(code) {
            Level::Text(text) => self.emit(code, &text),
            Level::Dead(spacing, mark) => {
                // Without this a dead key pressed while KOReader is down would
                // be swallowed instead of reaching X.
                if !koreader::reachable() {
                    return false;
                }
                self.pending = Some((spacing, mark));
                self.held.insert(code);
                true
            }
            Level::Pass(_) => false,
        }
    }

    fn level(&self, code: u16) -> Level {
        let idx = usize::from(self.shift) + if self.altgr { 2 } else { 0 };
        self.keys
            .get(&code)
            .map(|l| l[idx].clone())
            .unwrap_or(Level::Pass(None))
    }

    fn emit(&mut self, code: u16, text: &str) -> bool {
        if koreader::send_text(text) {
            self.held.insert(code);
            true
        } else {
            false
        }
    }

    /// What a dead key produces when `code` follows it, or None when `code`
    /// types nothing to put the accent on.
    fn compose(&self, spacing: char, mark: char, code: u16) -> Option<String> {
        if code == KEY_SPACE {
            return Some(spacing.to_string());
        }
        let base = self.level(code).char()?;
        Some(match keysym::compose(base, mark) {
            Some(made) => made.to_string(),
            // No such accented form, so both characters land, as XKB does.
            None => format!("{spacing}{base}"),
        })
    }
}

impl Level {
    fn is_pass(&self) -> bool {
        matches!(self, Level::Pass(_))
    }

    fn char(&self) -> Option<char> {
        match self {
            Level::Pass(c) => *c,
            Level::Text(t) => t.chars().next(),
            Level::Dead(..) => None,
        }
    }
}

/// What a US keyboard types at this keycode, which is exactly what KOReader's
/// hardcoded table types. Comparing against the position rather than against
/// "is it a letter" is what keeps AZERTY and QWERTZ right, where the letters
/// sit somewhere else and relaying them would type the US one.
const US_ROWS: &[(u16, &str)] = &[
    (2, "1234567890"),
    (16, "qwertyuiop"),
    (30, "asdfghjkl"),
    (44, "zxcvbnm"),
];

fn us_char(code: u16) -> Option<char> {
    US_ROWS
        .iter()
        .find_map(|(first, row)| row.chars().nth(usize::from(code.checked_sub(*first)?)))
}

/// A level KOReader already types correctly is relayed rather than sent, so
/// its own shortcuts keep working. That is the letters where the layout agrees
/// with US, the digits likewise, and space; everything else it either gets
/// wrong or cannot type at all.
fn resolve(code: u16, idx: usize, sym: &str) -> Level {
    if let Some(dead) = keysym::dead(sym) {
        return Level::Dead(dead.0, dead.1);
    }
    let Some(ch) = keysym::to_char(sym) else {
        return Level::Pass(None);
    };
    let known = match idx {
        0 => us_char(code) == Some(ch),
        1 => us_char(code).is_some_and(|u| u.is_ascii_lowercase() && ch == u.to_ascii_uppercase()),
        _ => false,
    };
    // Backspace, Tab, Return, Escape and Delete all carry a U+ codepoint in
    // keysymdef, so without this Delete would type U+007F instead of deleting.
    if known || ch <= ' ' || ch.is_control() {
        Level::Pass(Some(ch))
    } else {
        Level::Text(ch.to_string())
    }
}

/// The keymap X itself is running. `us` is the file `layout.rs` bind-mounts
/// over, so this resolves to whatever the user asked for, and to plain US when
/// the bind-mount did not take, which is what X would be showing then too.
fn dump() -> Option<String> {
    let keymap = "xkb_keymap { xkb_keycodes { include \"evdev+aliases(qwerty)\" }; \
                  xkb_types { include \"complete\" }; xkb_compat { include \"complete\" }; \
                  xkb_symbols { include \"pc+us\" }; xkb_geometry { include \"pc(pc105)\" }; };\n";
    let mut child = Command::new("xkbcomp")
        .args([&format!("-I{XKB_DIR}"), "-xkb", "-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| warn!("xkbcomp did not run, KOReader keeps the US layout: {e}"))
        .ok()?;
    child.stdin.take()?.write_all(keymap.as_bytes()).ok()?;
    let out = child.wait_with_output().ok()?;
    String::from_utf8(out.stdout).ok()
}

fn angled(line: &str) -> Option<&str> {
    between(line, '<', '>')
}

fn between(line: &str, open: char, close: char) -> Option<&str> {
    let start = line.find(open)? + open.len_utf8();
    let end = line[start..].find(close)? + start;
    Some(&line[start..end])
}

#[cfg(test)]
mod tests {
    use super::{KoLayout, Level};

    // Trimmed from `xkbcomp -I/usr/share/X11/xkb -xkb - -` fed pc+br(abnt2).
    const BR: &str = r#"
xkb_keymap {
xkb_keycodes "evdev" {
    <AE02> = 11;
    <AD01> = 24;
    <AD11> = 34;
    <AC01> = 38;
    <AC10> = 47;
    <AB10> = 61;
    <SPCE> = 65;
    <DELE> = 119;
    alias <MENU> = <COMP>;
};
xkb_symbols "pc+br(abnt2)" {
    key <AE02> {
        type= "FOUR_LEVEL",
        symbols[Group1]= [               2,              at,     twosuperior,         onehalf ]
    };
    key <AD01> {
        type= "FOUR_LEVEL_SEMIALPHABETIC",
        symbols[Group1]= [               q,               Q,           slash,           slash ]
    };
    key <AD11> {
        type= "FOUR_LEVEL",
        symbols[Group1]= [      dead_acute,      dead_grave,           acute,           grave ]
    };
    key <AC01> {
        type= "FOUR_LEVEL_SEMIALPHABETIC",
        symbols[Group1]= [               a,               A,              ae,              AE ]
    };
    key <AC10> {
        type= "FOUR_LEVEL_SEMIALPHABETIC",
        symbols[Group1]= [        ccedilla,        Ccedilla,      dead_acute, dead_doubleacute ]
    };
    key <AB10> {
        type= "FOUR_LEVEL",
        symbols[Group1]= [       semicolon,           colon,   dead_belowdot,   dead_abovedot ]
    };
    key  <SPCE> {         [           space ] };
    key <DELE> {         [          Delete ] };
};
xkb_geometry "pc(pc105)" {
    key <AC10> { ignored };
};
};
"#;

    fn br() -> KoLayout {
        KoLayout::from_xkb(BR)
    }

    #[test]
    fn keycodes_come_back_as_kernel_codes_not_x_ones() {
        // <AC10> = 47 in X is evdev 39.
        assert_eq!(br().level(39), Level::Text("ç".into()));
    }

    #[test]
    fn shift_and_altgr_pick_the_right_level() {
        let mut l = br();
        l.shift = true;
        assert_eq!(l.level(39), Level::Text("Ç".into()));
        l.shift = false;
        l.altgr = true;
        assert_eq!(l.level(3), Level::Text("²".into()));
        l.shift = true;
        assert_eq!(l.level(3), Level::Text("½".into()));
    }

    #[test]
    fn letters_and_plain_digits_relay_so_shortcuts_keep_working() {
        let mut l = br();
        assert_eq!(l.level(30), Level::Pass(Some('a')));
        assert_eq!(l.level(3), Level::Pass(Some('2')));
        assert!(!l.consume(30, 1));
        // Shifted digits are not letters, and KOReader cannot type them at all.
        l.shift = true;
        assert_eq!(l.level(3), Level::Text("@".into()));
        assert_eq!(l.level(30), Level::Pass(Some('A')));
    }

    #[test]
    fn space_relays_rather_than_being_typed_as_text() {
        let mut l = br();
        assert!(!l.consume(57, 1));
    }

    #[test]
    fn delete_deletes_instead_of_typing_its_control_character() {
        // XK_Delete is annotated U+007F in keysymdef, so it resolves to a
        // character and would be typed as text without the control guard.
        let mut l = br();
        assert!(!l.keys.contains_key(&111));
        assert!(!l.consume(111, 1));
    }

    #[test]
    fn dead_keys_resolve_and_compose_with_whatever_the_next_key_types() {
        let mut l = br();
        let Level::Dead(spacing, mark) = l.level(26) else {
            panic!("AD11 should be dead_acute")
        };
        assert_eq!(spacing, '´');
        assert_eq!(l.compose(spacing, mark, 30), Some("á".into()));
        // Space gives the accent alone, as XKB does.
        assert_eq!(l.compose(spacing, mark, 57), Some("´".into()));
        // No accented form: both characters land.
        assert_eq!(l.compose(spacing, mark, 16), Some("´q".into()));
        // A key that types nothing at all.
        assert_eq!(l.compose(spacing, mark, 103), None);
        // Uppercase follows the level, so it composes uppercase too.
        l.shift = true;
        assert_eq!(l.compose(spacing, mark, 30), Some("Á".into()));
    }

    #[test]
    fn an_unmapped_dead_key_is_relayed_rather_than_swallowing_the_next_press() {
        // dead_belowdot has no combining mark here, so AB10 AltGr must relay.
        let mut l = br();
        l.altgr = true;
        assert_eq!(l.level(53), Level::Pass(None));
    }

    // AZERTY moves the letters. Relaying them would type the US one.
    const FR: &str = r#"
xkb_keycodes "evdev" {
    <AD01> = 24;
    <AD02> = 25;
    <AC01> = 38;
};
xkb_symbols "pc+fr" {
    key <AD01> { [ a, A, ae, AE ] };
    key <AD02> { [ z, Z, guillemotleft, less ] };
    key <AC01> { [ q, Q, at, Greek_OMEGA ] };
};
"#;

    #[test]
    fn a_moved_letter_is_sent_as_text_not_relayed_as_the_us_one() {
        let mut l = KoLayout::from_xkb(FR);
        // evdev 16 is the US q position; AZERTY types a there.
        assert_eq!(l.level(16), Level::Text("a".into()));
        assert_eq!(l.level(17), Level::Text("z".into()));
        assert_eq!(l.level(30), Level::Text("q".into()));
        l.shift = true;
        assert_eq!(l.level(16), Level::Text("A".into()));
    }

    #[test]
    fn a_letter_that_did_not_move_still_relays() {
        // Same fixture, but br leaves q where US has it.
        let l = br();
        assert_eq!(l.level(16), Level::Pass(Some('q')));
    }

    #[test]
    fn modifiers_are_tracked_but_still_relayed() {
        let mut l = br();
        assert!(!l.consume(42, 1));
        assert!(l.shift);
        assert!(!l.consume(42, 0));
        assert!(!l.shift);
    }

    #[test]
    fn geometry_and_aliases_do_not_leak_into_the_map() {
        let l = br();
        // <MENU> alias has no numeric value, and geometry's key line has no list.
        // AE02 AD01 AD11 AC01 AC10 AB10. SPCE is every-level-relayed so it is
        // dropped, and the <MENU> alias has no keycode of its own.
        assert_eq!(l.keys.len(), 6);
        assert!(!l.keys.contains_key(&57));
    }
}

