use crate::config::{DeviceConfig, DpadDirection, Trigger};
use evdev::Key;
use log::{debug, info};
use std::collections::HashMap;
use std::process::Command;
use std::time::{Duration, Instant};

pub struct Mapper {
    mappings: HashMap<Key, String>,
    long_press_mappings: HashMap<Key, String>,
    dpad_mappings: HashMap<DpadDirection, String>,
    dpad_longpress_mappings: HashMap<DpadDirection, String>,
    trigger_mappings: HashMap<Trigger, String>,
    trigger_longpress_mappings: HashMap<Trigger, String>,
    debounce_ms: u64,
    long_press_ms: u64,
    repeat_ms: u64,
    log_buttons: bool,
    last_press: HashMap<Key, Instant>,
    press_start: HashMap<Key, Instant>,
    long_press_fired: HashMap<Key, bool>,
    last_repeat: HashMap<Key, Instant>,
    last_dpad: HashMap<DpadDirection, Instant>,
    last_trigger: HashMap<Trigger, Instant>,
    trigger_state: HashMap<Trigger, bool>, // Track if trigger is currently pressed
    // D-pad hold tracking (handles press-release-press auto-repeat pattern)
    dpad_sequence_start: HashMap<DpadDirection, Instant>,  // First press in sequence
    dpad_last_event: HashMap<DpadDirection, Instant>,      // Last event time
    dpad_longpress_fired: HashMap<DpadDirection, bool>,
    // Trigger hold tracking (handles press-release-press auto-repeat pattern)
    trigger_sequence_start: HashMap<Trigger, Instant>,  // First press in sequence
    trigger_last_event: HashMap<Trigger, Instant>,      // Last event time
    trigger_longpress_fired: HashMap<Trigger, bool>,
}

impl Mapper {
    pub fn new(
        cfg: &DeviceConfig,
        debounce_ms: u64,
        long_press_ms: u64,
        repeat_ms: u64,
        log_buttons: bool,
    ) -> Self {
        Self {
            mappings: cfg.mappings.clone(),
            long_press_mappings: cfg.long_press_mappings.clone(),
            dpad_mappings: cfg.dpad_mappings.clone(),
            dpad_longpress_mappings: cfg.dpad_longpress_mappings.clone(),
            trigger_mappings: cfg.trigger_mappings.clone(),
            trigger_longpress_mappings: cfg.trigger_longpress_mappings.clone(),
            debounce_ms,
            long_press_ms,
            repeat_ms,
            log_buttons,
            last_press: HashMap::new(),
            press_start: HashMap::new(),
            long_press_fired: HashMap::new(),
            last_repeat: HashMap::new(),
            last_dpad: HashMap::new(),
            last_trigger: HashMap::new(),
            trigger_state: HashMap::new(),
            dpad_sequence_start: HashMap::new(),
            dpad_last_event: HashMap::new(),
            dpad_longpress_fired: HashMap::new(),
            trigger_sequence_start: HashMap::new(),
            trigger_last_event: HashMap::new(),
            trigger_longpress_fired: HashMap::new(),
        }
    }

    /// True if this key is bound to a script (normal or long-press). Unbound
    /// keys are passed through to the virtual keyboard for the layout to map.
    pub fn is_mapped(&self, key: Key) -> bool {
        self.mappings.contains_key(&key) || self.long_press_mappings.contains_key(&key)
    }

    pub fn handle_press(&mut self, key: Key) {
        // Debounce check
        if let Some(last) = self.last_press.get(&key) {
            if last.elapsed() < Duration::from_millis(self.debounce_ms) {
                debug!("Debounced key {:?}", key);
                return;
            }
        }

        self.last_press.insert(key, Instant::now());
        self.press_start.insert(key, Instant::now());
        self.long_press_fired.insert(key, false);
    }

    pub fn handle_held(&mut self, key: Key) {
        // Check if we've started long press mode
        let long_press_active = self.long_press_fired.get(&key).copied().unwrap_or(false);

        if let Some(start) = self.press_start.get(&key) {
            let elapsed = start.elapsed();

            // Check for long press threshold
            if elapsed >= Duration::from_millis(self.long_press_ms) {
                // First time entering long press mode - check for long press mapping
                if !long_press_active {
                    if let Some(script) = self.long_press_mappings.get(&key) {
                        if self.log_buttons {
                            info!("Long press: {:?} (code: {}) -> {}", key, key.code(), script);
                        }
                        execute_script(script);
                        self.long_press_fired.insert(key, true);
                        self.last_repeat.insert(key, Instant::now());
                        return;
                    }
                }

                // Repeat mode: repeat the normal action at repeat_ms interval
                if self.repeat_ms > 0 {
                    let should_repeat = self
                        .last_repeat
                        .get(&key)
                        .map(|last| last.elapsed() >= Duration::from_millis(self.repeat_ms))
                        .unwrap_or(true);

                    if should_repeat {
                        if let Some(script) = self.mappings.get(&key) {
                            debug!("Repeat: {:?} (code: {}) -> {}", key, key.code(), script);
                            execute_script(script);
                            self.last_repeat.insert(key, Instant::now());
                            // Mark as fired so release doesn't trigger again
                            self.long_press_fired.insert(key, true);
                        }
                    }
                }
            }
        }
    }

    pub fn handle_release(&mut self, key: Key) {
        // A debounced press never registers in press_start, so its release
        // must not fire the action either.
        let had_press = self.press_start.remove(&key).is_some();
        let long_press_fired = self.long_press_fired.remove(&key).unwrap_or(false);
        self.last_repeat.remove(&key);

        if !had_press {
            debug!("Skipping release for {:?} (press was debounced)", key);
            return;
        }

        // If long press was already fired, don't execute normal action
        if long_press_fired {
            debug!("Skipping normal action for {:?} (long press/repeat fired)", key);
            return;
        }

        // Execute normal mapping
        if let Some(script) = self.mappings.get(&key) {
            if self.log_buttons {
                info!("Button: {:?} (code: {}) -> {}", key, key.code(), script);
            }
            execute_script(script);
        } else if self.log_buttons {
            // Log unmapped buttons for debugging
            info!("Button: {:?} (code: {}) [unmapped]", key, key.code());
        }
    }

    /// Handle D-pad axis event
    /// Hat0X: -1 = left, +1 = right, 0 = center
    /// Hat0Y: -1 = up, +1 = down, 0 = center
    ///
    /// Handles controllers that send press-release-press cycles for auto-repeat
    /// rather than holding the axis value. We detect a "sequence" by checking
    /// if events arrive within 300ms of each other.
    pub fn handle_dpad(&mut self, axis: u16, value: i32) {
        let direction = match (axis, value) {
            (16, -1) => Some(DpadDirection::Left),   // Hat0X = -1
            (16, 1) => Some(DpadDirection::Right),   // Hat0X = +1
            (17, -1) => Some(DpadDirection::Up),     // Hat0Y = -1
            (17, 1) => Some(DpadDirection::Down),    // Hat0Y = +1
            _ => None, // Center (0) = release
        };

        // Handle release (value = 0) - just update last_event time, don't clear sequence
        if value == 0 {
            let dirs: Vec<DpadDirection> = match axis {
                16 => vec![DpadDirection::Left, DpadDirection::Right],
                17 => vec![DpadDirection::Up, DpadDirection::Down],
                _ => vec![],
            };
            for dir in dirs {
                self.dpad_last_event.insert(dir, Instant::now());
            }
            return;
        }

        if let Some(dir) = direction {
            let now = Instant::now();

            // Check if this is a new sequence or continuation of existing one
            // A sequence continues if last event was within 300ms
            let is_new_sequence = self.dpad_last_event.get(&dir)
                .map(|last| last.elapsed() > Duration::from_millis(300))
                .unwrap_or(true);

            self.dpad_last_event.insert(dir, now);

            if is_new_sequence {
                // Debounce check for new sequence
                if let Some(last) = self.last_dpad.get(&dir) {
                    if last.elapsed() < Duration::from_millis(self.debounce_ms) {
                        debug!("Debounced dpad {:?}", dir);
                        return;
                    }
                }
                self.last_dpad.insert(dir, now);
                self.dpad_sequence_start.insert(dir, now);
                self.dpad_longpress_fired.insert(dir, false);

                // Execute normal mapping on first press of sequence
                if let Some(script) = self.dpad_mappings.get(&dir) {
                    if self.log_buttons {
                        info!("D-pad: {:?} -> {}", dir, script);
                    }
                    execute_script(script);
                } else if self.log_buttons {
                    info!("D-pad: {:?} [unmapped]", dir);
                }
            } else {
                // Continuation of sequence - check for long press
                self.handle_dpad_held(dir);
            }
        }
    }

    fn handle_dpad_held(&mut self, dir: DpadDirection) {
        let long_press_fired = self.dpad_longpress_fired.get(&dir).copied().unwrap_or(false);

        if let Some(start) = self.dpad_sequence_start.get(&dir) {
            let elapsed = start.elapsed();

            // Check for long press threshold
            if elapsed >= Duration::from_millis(self.long_press_ms) {
                // Use long press mapping if available, otherwise fall back to normal
                let script = self.dpad_longpress_mappings.get(&dir)
                    .or_else(|| self.dpad_mappings.get(&dir));

                if let Some(script) = script {
                    // Log first long press differently
                    if !long_press_fired {
                        if self.log_buttons {
                            info!("D-pad long press: {:?} -> {}", dir, script);
                        }
                        self.dpad_longpress_fired.insert(dir, true);
                    } else {
                        debug!("D-pad repeat: {:?} -> {}", dir, script);
                    }
                    execute_script(script);
                }
            }
        }
    }

    /// Handle trigger axis event
    /// Gas (code 9): RT - value 0 (released) to 1023 (fully pressed)
    /// Brake (code 10): LT - value 0 (released) to 1023 (fully pressed)
    ///
    /// Handles controllers that send press-release-press cycles for auto-repeat.
    pub fn handle_trigger(&mut self, axis: u16, value: i32) {
        let trigger = match axis {
            9 => Trigger::RT,   // Gas = Right Trigger
            10 => Trigger::LT,  // Brake = Left Trigger
            _ => return,
        };

        // Treat as pressed if value > 512 (halfway), released otherwise
        let pressed = value > 512;
        let was_pressed = self.trigger_state.get(&trigger).copied().unwrap_or(false);
        let now = Instant::now();

        // Handle release - just update last_event time
        if !pressed && was_pressed {
            self.trigger_last_event.insert(trigger, now);
            self.trigger_state.insert(trigger, false);
            return;
        }

        // Handle press (new or repeat)
        if pressed && !was_pressed {
            // Check if this is a new sequence or continuation
            let is_new_sequence = self.trigger_last_event.get(&trigger)
                .map(|last| last.elapsed() > Duration::from_millis(300))
                .unwrap_or(true);

            self.trigger_last_event.insert(trigger, now);
            self.trigger_state.insert(trigger, true);

            if is_new_sequence {
                // Debounce check for new sequence
                if let Some(last) = self.last_trigger.get(&trigger) {
                    if last.elapsed() < Duration::from_millis(self.debounce_ms) {
                        debug!("Debounced trigger {:?}", trigger);
                        return;
                    }
                }
                self.last_trigger.insert(trigger, now);
                self.trigger_sequence_start.insert(trigger, now);
                self.trigger_longpress_fired.insert(trigger, false);

                // Execute mapping on first press of sequence
                if let Some(script) = self.trigger_mappings.get(&trigger) {
                    if self.log_buttons {
                        info!("Trigger: {:?} -> {}", trigger, script);
                    }
                    execute_script(script);
                } else if self.log_buttons {
                    info!("Trigger: {:?} [unmapped]", trigger);
                }
            } else {
                // Continuation of sequence - check for long press
                self.handle_trigger_held(trigger);
            }
        }
        // Handle held (already pressed, still pressed - continuous axis value)
        else if pressed && was_pressed {
            self.trigger_last_event.insert(trigger, now);
            self.handle_trigger_held(trigger);
        }
    }

    fn handle_trigger_held(&mut self, trigger: Trigger) {
        let long_press_fired = self.trigger_longpress_fired.get(&trigger).copied().unwrap_or(false);

        if let Some(start) = self.trigger_sequence_start.get(&trigger) {
            let elapsed = start.elapsed();

            // Check for long press threshold
            if elapsed >= Duration::from_millis(self.long_press_ms) {
                // Use long press mapping if available, otherwise fall back to normal
                let script = self.trigger_longpress_mappings.get(&trigger)
                    .or_else(|| self.trigger_mappings.get(&trigger));

                if let Some(script) = script {
                    // Log first long press differently
                    if !long_press_fired {
                        if self.log_buttons {
                            info!("Trigger long press: {:?} -> {}", trigger, script);
                        }
                        self.trigger_longpress_fired.insert(trigger, true);
                    } else {
                        debug!("Trigger repeat: {:?} -> {}", trigger, script);
                    }
                    execute_script(script);
                }
            }
        }
    }
}

fn execute_script(script: &str) {
    // Always use shell to handle arguments properly
    match Command::new("/bin/sh").args(["-c", script]).spawn() {
        Ok(mut child) => {
            // Spawn thread to wait for child to avoid zombies
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => {
            log::error!("Failed to execute '{}': {}", script, e);
        }
    }
}
