use log::{debug, warn};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::time::Duration;

const PORT: u16 = 8080;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);
const WRITE_TIMEOUT: Duration = Duration::from_millis(300);
const READ_TIMEOUT: Duration = Duration::from_millis(300);

fn connect() -> Option<TcpStream> {
    let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, PORT));
    match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
        Ok(s) => {
            let _ = s.set_nodelay(true);
            let _ = s.set_write_timeout(Some(WRITE_TIMEOUT));
            Some(s)
        }
        Err(e) => {
            debug!("KOReader not reachable on port {}: {}", PORT, e);
            None
        }
    }
}

fn get(stream: &mut TcpStream, path: &str) -> bool {
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        path, PORT
    );
    stream.write_all(req.as_bytes()).is_ok()
}

/// Whether KOReader is up. Used to decide whether a keystroke goes to it as
/// text or to the virtual keyboard as a keycode.
pub fn reachable() -> bool {
    connect().is_some()
}

/// Send an event to KOReader's HTTP Inspector, e.g. `GotoViewRel/1`. False
/// means KOReader did not take it, so the caller can try the native reader.
/// The inspector closes the connection after every response, so there is
/// nothing to keep alive between calls.
pub fn send_event(event: &str) -> bool {
    let mut stream = match connect() {
        Some(s) => s,
        None => return false,
    };
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));

    if !get(&mut stream, &format!("/koreader/event/{}", event)) {
        debug!("KOReader event '{}' not sent", event);
        return false;
    }

    // KOReader only polls the socket on its 50ms UI tick, so the reply lags.
    // The request is delivered either way, so a timeout still counts as taken.
    let mut buf = [0u8; 64];
    match stream.read(&mut buf) {
        Ok(0) => debug!("KOReader closed without a reply to '{}'", event),
        Ok(n) => match status_code(&buf[..n]) {
            Some(code) if (200..300).contains(&code) => debug!("KOReader {} -> {}", event, code),
            Some(code) => warn!("KOReader rejected event '{}' with HTTP {}", event, code),
            None => warn!("KOReader sent no HTTP status for event '{}'", event),
        },
        Err(e) => debug!("No reply from KOReader for '{}': {}", event, e),
    }
    true
}

/// Insert `text` at the cursor of whichever KOReader input field has focus.
/// Unlike `send_event` this does not wait for the reply: the inspector only
/// answers on the 50ms UI tick, and at typing speed that would stall the
/// event loop for a tick per character.
pub fn send_text(text: &str) -> bool {
    let mut stream = match connect() {
        Some(s) => s,
        None => return false,
    };
    get(&mut stream, &text_path(text))
}

/// The inspector runs unquoted args through `tonumber`, so a digit would
/// arrive as a number and never reach `addChars`. Quoting keeps it a string.
/// Its parser has no escape inside quotes, so the quote character is picked
/// to be one the text does not contain.
fn text_path(text: &str) -> String {
    let quote = if text.contains('"') { '\'' } else { '"' };
    format!("/koreader/event/TextInput/{}{}{}", quote, encode(text), quote)
}

fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn status_code(response: &[u8]) -> Option<u16> {
    let head = std::str::from_utf8(response).ok()?;
    head.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::text_path;

    #[test]
    fn text_is_quoted_and_percent_encoded() {
        assert_eq!(text_path("ç"), "/koreader/event/TextInput/\"%C3%A7\"");
        // A digit unquoted would reach Event:new() as a number, not a string.
        assert_eq!(text_path("2"), "/koreader/event/TextInput/\"2\"");
        // Nothing escapes a quote inside a quoted arg, so the other one is used.
        assert_eq!(text_path("\""), "/koreader/event/TextInput/'%22'");
        assert_eq!(text_path("'"), "/koreader/event/TextInput/\"%27\"");
    }
}
