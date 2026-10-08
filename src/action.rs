use crate::{koreader, vkeyboard};
use log::{debug, error, info};
use std::process::Command;
use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;
use std::thread;
use std::time::Instant;

/// Steps run in order until one lands, which is how `auto.sh` picks a reader.
#[derive(Debug, PartialEq)]
enum Step {
    Koreader(String), // HTTP Inspector event path, e.g. GotoViewRel/1
    Key(String),      // injection request, e.g. page_next or KEY_LEFT
}

/// Run a configured action. The shipped reader scripts are handled in-process —
/// a `/bin/sh` plus a `curl` per press costs more than the page turn itself.
/// Everything else goes to the shell.
pub fn run(script: &str) {
    match plan(script) {
        Some(steps) => queue(script.to_string(), steps),
        None => spawn_shell(script),
    }
}

/// Run a shell command line, without waiting for it.
pub fn spawn_shell(script: &str) {
    match Command::new("/bin/sh").args(["-c", script]).spawn() {
        Ok(mut child) => {
            // Reap elsewhere, no zombies.
            let started = Instant::now();
            let script = script.to_string();
            thread::spawn(move || {
                let _ = child.wait();
                log_duration(started, &script);
            });
        }
        Err(e) => error!("Failed to execute '{}': {}", script, e),
    }
}

fn log_duration(started: Instant, script: &str) {
    let ms = started.elapsed().as_millis();
    if ms >= 100 {
        info!("Finished in {}ms: {}", ms, script);
    } else {
        debug!("Finished in {}ms: {}", ms, script);
    }
}

/// One thread, so a held button cannot overtake itself and the event loop
/// never waits on a socket.
fn queue(script: String, steps: Vec<Step>) {
    static QUEUE: OnceLock<Sender<(String, Vec<Step>)>> = OnceLock::new();
    let tx = QUEUE.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<(String, Vec<Step>)>();
        thread::Builder::new()
            .name("actions".into())
            .spawn(move || {
                for (script, steps) in rx {
                    let started = Instant::now();
                    execute(&script, &steps);
                    log_duration(started, &script);
                }
            })
            .expect("spawn action thread");
        tx
    });
    if let Err(e) = tx.send((script, steps)) {
        error!("Action thread is gone: {}", e);
    }
}

fn execute(script: &str, steps: &[Step]) {
    for step in steps {
        match step {
            Step::Koreader(event) => {
                if koreader::send_event(event) {
                    return;
                }
            }
            Step::Key(request) => {
                if vkeyboard::inject(request) {
                    return;
                }
                // Nothing to inject into on this firmware, so hand it back to
                // the script, which taps the screen instead.
                spawn_shell(script);
                return;
            }
        }
    }
    debug!("No step of {:?} landed", steps);
}

/// Translate the shipped reader scripts into steps. Shell syntax, an unknown
/// script or anything needing lipc returns None and stays a shell call.
fn plan(script: &str) -> Option<Vec<Step>> {
    if script.contains(|c| "|&;<>()$`\\\"'*?[#~=%\n".contains(c)) {
        return None;
    }
    let mut words = script.split_whitespace();
    let path = words.next()?;
    let name = path.rsplit('/').next()?;
    let cmd = words.next()?;
    let arg = words.next();
    if words.next().is_some() {
        return None; // more than one argument is never a hot path
    }

    // `koreader.sh event <Name>` is any Dispatcher action the plugin offers.
    // Warmth steps also go directly to KOReader without spawning a shell.
    if let Some(arg) = arg {
        if name == "koreader.sh" && cmd == "warmth" {
            if let Ok(step) = arg.parse::<i32>() {
                if step != 0 {
                    let event = if step > 0 { "IncreaseFlWarmth" } else { "DecreaseFlWarmth" };
                    return Some(vec![Step::Koreader(format!("{}/{}", event, step.unsigned_abs()))]);
                }
            }
        }
        if name == "koreader.sh" && cmd == "event" && is_event_name(arg) {
            return Some(vec![Step::Koreader(arg.to_string())]);
        }
        return None;
    }

    match name {
        "koreader.sh" => Some(vec![Step::Koreader(koreader_event(cmd)?.to_string())]),
        "kindle.sh" => Some(vec![Step::Key(native_key(cmd)?.to_string())]),
        // The FIFO round trip key.sh does ends in the same inject() call, so
        // the shell only ever cost a fork. Held keys go through here.
        "key.sh" => Some(vec![Step::Key(injectable(cmd)?)]),
        "auto.sh" => Some(vec![
            Step::Koreader(koreader_event(cmd)?.to_string()),
            Step::Key(native_key(cmd)?.to_string()),
        ]),
        _ => None,
    }
}

/// A bare KOReader event name. Anything with a slash already carries its own
/// argument and is left to the shell, which url-encodes it.
fn is_event_name(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_alphabetic())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Only what the injector itself accepts, so an unknown name still reaches
/// key.sh and its evemu fallback rather than being dropped in-process.
fn injectable(cmd: &str) -> Option<String> {
    match cmd {
        "page_next" | "page_prev" => Some(cmd.to_string()),
        _ => crate::config::parse_key(cmd).map(|_| cmd.to_string()),
    }
}

pub fn sends_keys(script: &str) -> bool {
    let words: Vec<&str> = script
        .split(|c: char| c.is_whitespace() || "|&;()`\"'".contains(c))
        .filter(|w| !w.is_empty())
        .collect();
    words
        .windows(2)
        .any(|w| match w[0].rsplit('/').next().unwrap_or("") {
            "key.sh" => crate::config::parse_key(w[1]).is_some(),
            "kindle.sh" | "auto.sh" => native_key(w[1]).is_some_and(|k| !k.starts_with("page_")),
            _ => false,
        })
}

fn koreader_event(cmd: &str) -> Option<&'static str> {
    match cmd {
        "next_page" => Some("GotoViewRel/1"),
        "prev_page" => Some("GotoViewRel/-1"),
        "brightness_toggle" => Some("ToggleFrontlight"),
        "night_mode" => Some("ToggleNightMode"),
        "menu" => Some("ShowMenu"),
        "toggle_status_bar" => Some("ToggleFooterMode"),
        "rotate" => Some("IterateRotation"),
        _ => None,
    }
}

fn native_key(cmd: &str) -> Option<&'static str> {
    match cmd {
        "next_page" => Some("page_next"),
        "prev_page" => Some("page_prev"),
        "home" => Some("KEY_HOME"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_turns_are_planned() {
        assert_eq!(
            plan("/mnt/us/kindle-button-mapper/scripts/kindle.sh next_page"),
            Some(vec![Step::Key("page_next".into())])
        );
        assert_eq!(
            plan("scripts/koreader.sh prev_page"),
            Some(vec![Step::Koreader("GotoViewRel/-1".into())])
        );
        assert_eq!(
            plan("/mnt/us/kindle-button-mapper/scripts/auto.sh next_page"),
            Some(vec![
                Step::Koreader("GotoViewRel/1".into()),
                Step::Key("page_next".into()),
            ])
        );
    }

    #[test]
    fn key_injection_is_planned() {
        assert_eq!(
            plan("/mnt/us/kindle-button-mapper/scripts/key.sh KEY_LEFT"),
            Some(vec![Step::Key("KEY_LEFT".into())])
        );
        // Lowercase and bare codes are what parse_key takes, so they plan too.
        assert_eq!(
            plan("scripts/key.sh key_enter"),
            Some(vec![Step::Key("key_enter".into())])
        );
        assert_eq!(
            plan("scripts/key.sh 105"),
            Some(vec![Step::Key("105".into())])
        );
        assert_eq!(
            plan("scripts/key.sh page_next"),
            Some(vec![Step::Key("page_next".into())])
        );
        // Unknown names stay a shell call so key.sh can still try evemu.
        assert_eq!(plan("scripts/key.sh NOT_A_KEY"), None);
    }

    #[test]
    fn koreader_events_are_planned() {
        assert_eq!(
            plan("/mnt/us/kindle-button-mapper/scripts/koreader.sh event ShowMenu"),
            Some(vec![Step::Koreader("ShowMenu".into())])
        );
        assert_eq!(
            plan("scripts/koreader.sh event ToggleNightMode"),
            Some(vec![Step::Koreader("ToggleNightMode".into())])
        );
        // Already carries an argument, the shell url-encodes it.
        assert_eq!(plan("scripts/koreader.sh event GotoViewRel/1"), None);
        // Two arguments is never a hot path.
        assert_eq!(plan("scripts/koreader.sh event Foo Bar"), None);
        // The named shortcuts still work and win over the generic form.
        assert_eq!(
            plan("scripts/koreader.sh next_page"),
            Some(vec![Step::Koreader("GotoViewRel/1".into())])
        );
    }

    #[test]
    fn warmth_actions_are_planned() {
        for (step, event) in [
            ("1", "IncreaseFlWarmth/1"),
            ("-1", "DecreaseFlWarmth/1"),
            ("5", "IncreaseFlWarmth/5"),
            ("-5", "DecreaseFlWarmth/5"),
        ] {
            assert_eq!(
                plan(&format!("scripts/koreader.sh warmth {}", step)),
                Some(vec![Step::Koreader(event.into())])
            );
        }
    }

    #[test]
    fn lipc_and_user_scripts_stay_shell() {
        // auto.sh menu falls back to the native toolbar over lipc
        assert_eq!(plan("scripts/auto.sh menu"), None);
        assert_eq!(plan("scripts/kindle.sh toolbar"), None);
        assert_eq!(plan("scripts/koreader.sh brightness 2"), None);
        assert_eq!(plan("scripts/auto.sh next_page && echo hi"), None);
        assert_eq!(plan("lipc-set-prop com.lab126.powerd flIntensity 5"), None);
        assert_eq!(plan("/mnt/us/my-script.sh next_page"), None);
        assert_eq!(plan(""), None);
    }
}
