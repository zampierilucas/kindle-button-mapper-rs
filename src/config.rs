use configparser::ini::Ini;
use evdev::Key;
use log::info;
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

/// D-pad directions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DpadDirection {
    Up,
    Down,
    Left,
    Right,
}

/// Trigger buttons
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trigger {
    LT, // Left Trigger (Brake, code 10)
    RT, // Right Trigger (Gas, code 9)
}

#[derive(Debug, Clone)]
pub struct DeviceConfig {
    pub id: String,
    pub name: Option<String>,
    pub uniq: Option<String>,
    pub grab: bool,
    /// Whether the file said so. Blocks added by the pairing side never do, and
    /// those are the ones the worker is allowed to downgrade to a shared open.
    pub grab_explicit: bool,
    /// Re-emit keys with no mapping through the virtual keyboard. Only means
    /// anything alongside a grab, and it is what makes a keyboard survive one.
    pub passthrough: bool,
    /// Map this mouse's buttons. Everything unmapped is relayed, so the rest
    /// of the mouse keeps working.
    pub mouse: bool,
    pub keyboard_layout: Option<String>,
    pub key_repeat: Option<(u32, u32)>,
    pub mappings: HashMap<Key, String>,
    pub long_press_mappings: HashMap<Key, String>,
    pub dpad_mappings: HashMap<DpadDirection, String>,
    pub dpad_longpress_mappings: HashMap<DpadDirection, String>,
    pub trigger_mappings: HashMap<Trigger, String>,
    pub trigger_longpress_mappings: HashMap<Trigger, String>,
    pub stick_left_mappings: HashMap<DpadDirection, String>,
    pub stick_right_mappings: HashMap<DpadDirection, String>,
    pub gesture_mappings: HashMap<String, String>,
}

const KEY_SH: &str = "/mnt/us/kindle-button-mapper/scripts/key.sh";

impl DeviceConfig {
    /// The default gamepad layout, arrows on the D-pad, Enter/Back on A and B,
    /// page turns on the shoulders. The worker applies it when an unmapped
    /// device turns out to be a gamepad, so a pad added through the app drives
    /// KOReader before anyone maps a button. Config load can't decide this,
    /// only the opened node's capabilities say what the device is.
    pub fn apply_default_layout(&mut self) {
        for (dir, key) in [
            (DpadDirection::Up, "KEY_UP"),
            (DpadDirection::Down, "KEY_DOWN"),
            (DpadDirection::Left, "KEY_LEFT"),
            (DpadDirection::Right, "KEY_RIGHT"),
        ] {
            self.dpad_mappings.insert(dir, format!("{} {}", KEY_SH, key));
        }
        // BTN_A, BTN_B, BTN_TL, BTN_TR.
        for (code, key) in [
            (304u16, "KEY_ENTER"),
            (305, "KEY_BACK"),
            (310, "page_prev"),
            (311, "page_next"),
        ] {
            self.mappings
                .insert(Key::new(code), format!("{} {}", KEY_SH, key));
        }
        info!("[{}] no mappings configured, using the default gamepad layout", self.id);
    }

    pub fn is_unmapped(&self) -> bool {
        self.mappings.is_empty()
            && self.long_press_mappings.is_empty()
            && self.dpad_mappings.is_empty()
            && self.dpad_longpress_mappings.is_empty()
            && self.trigger_mappings.is_empty()
            && self.trigger_longpress_mappings.is_empty()
            && self.stick_left_mappings.is_empty()
            && self.stick_right_mappings.is_empty()
            && self.gesture_mappings.is_empty()
    }

    #[cfg(test)]
    pub fn for_test(id: &str) -> Self {
        Self::new(id.to_string())
    }

    fn new(id: String) -> Self {
        Self {
            id,
            name: None,
            uniq: None,
            grab: true,
            grab_explicit: false,
            passthrough: false,
            mouse: false,
            keyboard_layout: None,
            key_repeat: None,
            mappings: HashMap::new(),
            long_press_mappings: HashMap::new(),
            dpad_mappings: HashMap::new(),
            dpad_longpress_mappings: HashMap::new(),
            trigger_mappings: HashMap::new(),
            trigger_longpress_mappings: HashMap::new(),
            stick_left_mappings: HashMap::new(),
            stick_right_mappings: HashMap::new(),
            gesture_mappings: HashMap::new(),
        }
    }
}

#[derive(Debug)]
pub struct Config {
    pub devices: Vec<DeviceConfig>,
    pub debounce_ms: u64,
    pub long_press_ms: u64,
    pub repeat_ms: u64,
    pub log_buttons: bool,
    pub keep_awake: bool,
    pub stick_deadzone: i32,
    pub gesture_min_percent: i32,
    pub gesture_tolerance: f32,
    pub gesture_templates: Vec<(String, Vec<crate::gesture::Point>)>,
    pub on_connect: Option<String>,
    pub on_disconnect: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            devices: Vec::new(),
            debounce_ms: 200,
            long_press_ms: 500,
            repeat_ms: 100,
            log_buttons: false,
            keep_awake: true,
            stick_deadzone: 50,
            gesture_min_percent: 15,
            gesture_tolerance: 0.30,
            gesture_templates: Vec::new(),
            on_connect: None,
            on_disconnect: None,
        }
    }
}

const DEFAULT_CONFIG: &str = "# Kindle Button Mapper - default config
# Edit via the Button Mapper WAF app, or by hand.

[settings]
debounce_ms = 0
log_buttons = true
long_press_ms = 500
repeat_ms = 100
keep_awake = true
";

const OLD_SECTIONS: [&str; 7] = [
    "device", "buttons", "longpress", "dpad", "dpad_longpress", "triggers", "triggers_longpress",
];

impl Config {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let p = path.as_ref();
        if !p.exists() {
            std::fs::write(p, DEFAULT_CONFIG)
                .map_err(|e| format!("Cannot create {}: {}", p.display(), e))?;
        }

        let mut ini = Ini::new();
        ini.load(p).map_err(|e| format!("Failed to load config: {}", e))?;

        let mut config = Config::default();
        let map = ini.get_map_ref();

        if map.keys().any(|s| OLD_SECTIONS.contains(&s.as_str())) {
            return Err(format!(
                "{} uses the old config format. Delete it and re-add your device via the Button Mapper app.",
                p.display()
            ));
        }

        if let Some(s) = map.get("settings") {
            if let Some(v) = get(s, "debounce_ms") { config.debounce_ms = v.parse().unwrap_or(config.debounce_ms); }
            if let Some(v) = get(s, "long_press_ms") { config.long_press_ms = v.parse().unwrap_or(config.long_press_ms); }
            if let Some(v) = get(s, "repeat_ms") { config.repeat_ms = v.parse().unwrap_or(config.repeat_ms); }
            if let Some(v) = get(s, "log_buttons") { config.log_buttons = parse_bool(v); }
            if let Some(v) = get(s, "keep_awake") { config.keep_awake = parse_bool(v); }
            if let Some(v) = get(s, "stick_deadzone") { config.stick_deadzone = v.parse().unwrap_or(config.stick_deadzone); }
            if let Some(v) = get(s, "gesture_min_percent") { config.gesture_min_percent = v.parse().unwrap_or(config.gesture_min_percent); }
            if let Some(v) = get(s, "gesture_tolerance") { config.gesture_tolerance = v.parse().unwrap_or(config.gesture_tolerance); }
            config.on_connect = get(s, "on_connect").filter(|v| !v.is_empty()).map(String::from);
            config.on_disconnect = get(s, "on_disconnect").filter(|v| !v.is_empty()).map(String::from);
        }

        if let Some(sec) = map.get("gestures") {
            for (name, points) in sec {
                if let Some(points) = points {
                    let parsed = crate::gesture::from_string(points);
                    if !parsed.is_empty() {
                        config.gesture_templates.push((name.clone(), parsed));
                    }
                }
            }
        }

        // First pass: collect device IDs from [device.NAME] sections, ordered by appearance.
        let mut devices: HashMap<String, DeviceConfig> = HashMap::new();
        let mut order: Vec<String> = Vec::new();

        for section in map.keys() {
            let parts: Vec<&str> = section.split('.').collect();
            if parts.first() != Some(&"device") || parts.len() < 2 {
                continue;
            }
            let id = parts[1].to_string();
            if !devices.contains_key(&id) {
                devices.insert(id.clone(), DeviceConfig::new(id.clone()));
                order.push(id);
            }
        }

        // Second pass: fill in each device's fields.
        for section in map.keys() {
            let parts: Vec<&str> = section.split('.').collect();
            if parts.first() != Some(&"device") {
                continue;
            }
            let id = match parts.get(1) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let dev = match devices.get_mut(&id) {
                Some(d) => d,
                None => continue,
            };
            let entries = match map.get(section) {
                Some(e) => e,
                None => continue,
            };

            match parts.get(2).copied() {
                None => {
                    if let Some(v) = get(entries, "name") { dev.name = Some(v.to_string()); }
                    if let Some(v) = get(entries, "uniq") { dev.uniq = Some(v.to_string()); }
                    if let Some(v) = get(entries, "grab") {
                        dev.grab = parse_bool(v);
                        dev.grab_explicit = true;
                    }
                    if let Some(v) = get(entries, "passthrough") { dev.passthrough = parse_bool(v); }
                    if let Some(v) = get(entries, "type") {
                        dev.mouse = v.trim().eq_ignore_ascii_case("mouse");
                    }
                    dev.keyboard_layout = get(entries, "keyboard_layout")
                        .map(str::trim)
                        .filter(|v| !v.is_empty())
                        .map(String::from);
                    dev.key_repeat = get(entries, "key_repeat").and_then(parse_key_repeat);
                }
                Some("buttons") => fill_key_map(entries, &mut dev.mappings),
                Some("longpress") => fill_key_map(entries, &mut dev.long_press_mappings),
                Some("dpad") => fill_dpad_map(entries, &mut dev.dpad_mappings),
                Some("dpad_longpress") => fill_dpad_map(entries, &mut dev.dpad_longpress_mappings),
                Some("triggers") => fill_trigger_map(entries, &mut dev.trigger_mappings),
                Some("triggers_longpress") => {
                    fill_trigger_map(entries, &mut dev.trigger_longpress_mappings)
                }
                Some("stick_left") => fill_dpad_map(entries, &mut dev.stick_left_mappings),
                Some("stick_right") => fill_dpad_map(entries, &mut dev.stick_right_mappings),
                Some("gestures") => {
                    for (k, v) in entries {
                        if let Some(v) = v {
                            dev.gesture_mappings.insert(k.clone(), v.clone());
                        }
                    }
                }
                _ => {}
            }
        }

        for id in order {
            if let Some(dev) = devices.remove(&id) {
                if dev.name.is_some() || dev.uniq.is_some() {
                    config.devices.push(dev);
                }
            }
        }

        Ok(config)
    }
}

fn get<'a>(section: &'a HashMap<String, Option<String>>, key: &str) -> Option<&'a str> {
    section.get(key).and_then(|v| v.as_deref())
}

fn parse_bool(v: &str) -> bool {
    matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "yes" | "1" | "on")
}

fn parse_key_repeat(v: &str) -> Option<(u32, u32)> {
    let (delay, rate) = v.split_once(',')?;
    let delay: u32 = delay.trim().parse().ok()?;
    let rate: u32 = rate.trim().parse().ok()?;
    (delay > 0 && rate > 0).then_some((delay, rate))
}

fn fill_key_map(
    entries: &HashMap<String, Option<String>>,
    out: &mut HashMap<Key, String>,
) {
    for (k, v) in entries {
        if let (Some(key), Some(script)) = (parse_key(k), v) {
            out.insert(key, script.clone());
        }
    }
}

fn fill_dpad_map(
    entries: &HashMap<String, Option<String>>,
    out: &mut HashMap<DpadDirection, String>,
) {
    for (k, v) in entries {
        if let (Some(dir), Some(script)) = (parse_dpad_direction(k), v) {
            out.insert(dir, script.clone());
        }
    }
}

fn fill_trigger_map(
    entries: &HashMap<String, Option<String>>,
    out: &mut HashMap<Trigger, String>,
) {
    for (k, v) in entries {
        if let (Some(t), Some(script)) = (parse_trigger(k), v) {
            out.insert(t, script.clone());
        }
    }
}

pub fn parse_key(s: &str) -> Option<Key> {
    // Try parsing as decimal
    if let Ok(code) = s.parse::<u16>() {
        return Some(Key::new(code));
    }

    // Try parsing as hex (0x prefix)
    if let Some(hex) = s.strip_prefix("0x") {
        if let Ok(code) = u16::from_str_radix(hex, 16) {
            return Some(Key::new(code));
        }
    }

    // Try parsing as named key (evdev knows every KEY_* name)
    Key::from_str(&s.to_uppercase()).ok()
}

fn parse_dpad_direction(s: &str) -> Option<DpadDirection> {
    match s.to_lowercase().as_str() {
        "up" => Some(DpadDirection::Up),
        "down" => Some(DpadDirection::Down),
        "left" => Some(DpadDirection::Left),
        "right" => Some(DpadDirection::Right),
        _ => None,
    }
}

fn parse_trigger(s: &str) -> Option<Trigger> {
    match s.to_lowercase().as_str() {
        "lt" | "left" => Some(Trigger::LT),
        "rt" | "right" => Some(Trigger::RT),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn load_str(name: &str, body: &str) -> Config {
        let path = std::env::temp_dir().join(name);
        let mut f = std::fs::File::create(&path).expect("write test config");
        f.write_all(body.as_bytes()).expect("write test config");
        Config::load(&path).expect("load test config")
    }

    #[test]
    fn load_leaves_an_unmapped_device_unmapped() {
        // The worker decides on defaults from the opened node's capabilities;
        // load must not guess.
        let cfg = load_str(
            "kbm-defaults.ini",
            "[device.pad]\nuniq = AA:BB:CC:DD:EE:FF\n",
        );
        assert!(cfg.devices[0].is_unmapped());
    }

    #[test]
    fn default_layout_covers_dpad_and_face_buttons() {
        let mut dev = DeviceConfig::new("pad".into());
        dev.apply_default_layout();
        assert!(!dev.is_unmapped());
        assert_eq!(dev.dpad_mappings.len(), 4);
        assert!(dev.dpad_mappings[&DpadDirection::Left].ends_with("key.sh KEY_LEFT"));
        assert!(dev.mappings[&Key::new(304)].ends_with("key.sh KEY_ENTER"));
        assert!(dev.mappings[&Key::new(311)].ends_with("key.sh page_next"));
    }

    #[test]
    fn grab_is_only_explicit_when_the_file_says_so() {
        // The worker downgrades an implicit grab on anything that isn't a
        // gamepad, so the difference has to survive the parse.
        let cfg = load_str(
            "kbm-grab.ini",
            "[device.pad]\nuniq = AA:BB:CC:DD:EE:FF\n\n[device.kbd]\nuniq = 11:22:33:44:55:66\ngrab = true\n",
        );
        let pad = cfg.devices.iter().find(|d| d.id == "pad").expect("pad");
        assert!(pad.grab && !pad.grab_explicit);
        let kbd = cfg.devices.iter().find(|d| d.id == "kbd").expect("kbd");
        assert!(kbd.grab && kbd.grab_explicit);
    }

    #[test]
    fn key_repeat_takes_a_delay_and_a_rate() {
        assert_eq!(parse_key_repeat("250,20"), Some((250, 20)));
        assert_eq!(parse_key_repeat(" 400 , 25 "), Some((400, 25)));
        assert_eq!(parse_key_repeat("250"), None);
        assert_eq!(parse_key_repeat("0,20"), None);
        assert_eq!(parse_key_repeat("250,0"), None);
        assert_eq!(parse_key_repeat("fast,20"), None);

        let cfg = load_str(
            "kbm-key-repeat.ini",
            "[device.kbd]\nuniq = 11:22:33:44:55:66\nkey_repeat = 250,20\n",
        );
        assert_eq!(cfg.devices[0].key_repeat, Some((250, 20)));
    }

    #[test]
    fn passthrough_is_off_unless_asked_for() {
        let cfg = load_str(
            "kbm-passthrough.ini",
            "[device.pad]\nuniq = AA:BB:CC:DD:EE:FF\n\n[device.kbd]\nuniq = 11:22:33:44:55:66\ngrab = true\npassthrough = true\n",
        );
        let pad = cfg.devices.iter().find(|d| d.id == "pad").expect("pad");
        assert!(!pad.passthrough);
        let kbd = cfg.devices.iter().find(|d| d.id == "kbd").expect("kbd");
        assert!(kbd.passthrough && kbd.grab && kbd.grab_explicit);
    }

    #[test]
    fn type_mouse_is_the_only_type_that_changes_anything() {
        let cfg = load_str(
            "kbm-type.ini",
            "[device.mouse]\nuniq = AA:BB:CC:DD:EE:FF\ntype = Mouse\n\n[device.pad]\nuniq = 11:22:33:44:55:66\n\n[device.kbd]\nuniq = 22:33:44:55:66:77\ntype = keyboard\n",
        );
        let by = |id: &str| cfg.devices.iter().find(|d| d.id == id).expect("device");
        assert!(by("mouse").mouse);
        assert!(!by("pad").mouse);
        assert!(!by("kbd").mouse);
    }

    #[test]
    fn one_hand_mapped_button_counts_as_mapped() {
        let cfg = load_str(
            "kbm-nodefaults.ini",
            "[device.pad]\nuniq = AA:BB:CC:DD:EE:FF\n\n[device.pad.buttons]\n304 = /mnt/us/mine.sh\n",
        );
        let dev = &cfg.devices[0];
        assert!(!dev.is_unmapped());
        assert!(dev.dpad_mappings.is_empty());
        assert_eq!(dev.mappings[&Key::new(304)], "/mnt/us/mine.sh");
    }
}

#[cfg(test)]
mod gesture_config_tests {
    use super::*;

    #[test]
    fn a_gesture_survives_a_round_trip_through_the_ini_parser() {
        let pts: Vec<crate::gesture::Point> =
            (0..32).map(|i| (i as f32 / 31.0, 0.5)).collect();
        // A name the user typed, not a generated one.
        let body = format!(
            "[settings]\nlog_buttons = true\n\n[gestures]\nPage Next = {}\n\n\
             [device.pad]\nname = Pad\nuniq = AA:BB\n\n\
             [device.pad.gestures]\nPage Next = /bin/true\n",
            crate::gesture::to_string(&pts)
        );
        let path = std::env::temp_dir().join("kbm_gesture_roundtrip.ini");
        std::fs::write(&path, body).unwrap();

        let cfg = Config::load(&path).expect("config loads");
        let (name, loaded) = cfg.gesture_templates.first().expect("template present");
        assert_eq!(loaded.len(), pts.len(), "every point must survive the parser");
        assert!(crate::gesture::distance(loaded, &pts) < 0.01);

        // Whatever the parser does to the spelling, both halves must agree or
        // the binding never fires.
        let dev = cfg.devices.iter().find(|d| d.id == "pad").unwrap();
        assert!(dev.gesture_mappings.contains_key(name), "template {:?} has no binding", name);
        let _ = std::fs::remove_file(&path);
    }
}
