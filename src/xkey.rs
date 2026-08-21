use log::debug;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::process::Command;
use std::time::Duration;

const SOCKET: &str = "/tmp/.X11-unix/X0";
const TIMEOUT: Duration = Duration::from_millis(500);

const KEYCODE_OFFSET: u8 = 8;
const ATOM_WM_NAME: u32 = 39;
const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;
const MAX_NAME: u32 = 128;

pub fn send_page(evdev_code: u16) -> bool {
    let Some(title) = active_window_title() else {
        debug!("X page turn: winmgr does not say what is in front");
        return false;
    };
    let Some((prefix, id)) = title_parts(&title) else {
        debug!("X page turn: {:?} is not an app window", title);
        return false;
    };
    match turn(prefix, id, (evdev_code as u8).wrapping_add(KEYCODE_OFFSET)) {
        Ok(()) => {
            debug!("X page turn sent to {}", id);
            true
        }
        Err(e) => {
            debug!("X page turn to {}: {}", id, e);
            false
        }
    }
}

fn turn(prefix: &str, id: &str, keycode: u8) -> io::Result<()> {
    let mut x = X::connect()?;
    let window = x
        .find(prefix, id)?
        .ok_or_else(|| io::Error::other(format!("no window named {}ID:{}", prefix, id)))?;
    x.send_key(window, keycode, KEY_PRESS)?;
    x.send_key(window, keycode, KEY_RELEASE)?;
    x.request(&[43, 0, 1, 0])?;
    x.reply()?;
    Ok(())
}

fn active_window_title() -> Option<String> {
    let out = Command::new("lipc-get-prop")
        .args(["com.lab126.winmgr", "getActiveAppTitle"])
        .output()
        .ok()?;
    let title = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!title.is_empty()).then_some(title)
}

fn title_parts(title: &str) -> Option<(&str, &str)> {
    let (prefix, rest) = title.split_once("ID:")?;
    let id = rest.split('_').next()?;
    (!prefix.is_empty() && !id.is_empty()).then_some((prefix, id))
}

fn is_match(name: &str, prefix: &str, id: &str) -> bool {
    match name
        .strip_prefix(prefix)
        .and_then(|r| r.strip_prefix("ID:"))
    {
        Some(rest) => rest.split('_').next() == Some(id),
        None => false,
    }
}

struct X {
    sock: UnixStream,
    root: u32,
}

impl X {
    fn connect() -> io::Result<Self> {
        let sock = UnixStream::connect(SOCKET)?;
        sock.set_read_timeout(Some(TIMEOUT))?;
        sock.set_write_timeout(Some(TIMEOUT))?;
        let mut x = X { sock, root: 0 };

        let mut req = [0u8; 12];
        req[0] = b'l';
        req[2] = 11;
        x.sock.write_all(&req)?;

        let mut head = [0u8; 8];
        x.sock.read_exact(&mut head)?;
        if head[0] != 1 {
            return Err(io::Error::other(format!("X refused us: {}", head[0])));
        }
        let mut setup = vec![0u8; read_u16(&head, 6)? as usize * 4];
        x.sock.read_exact(&mut setup)?;

        let vendor = read_u16(&setup, 16)? as usize;
        let padded = vendor + (4 - vendor % 4) % 4;
        let formats = *setup.get(21).ok_or_else(short)? as usize;
        x.root = read_u32(&setup, 32 + padded + formats * 8)?;
        Ok(x)
    }

    fn request(&mut self, req: &[u8]) -> io::Result<()> {
        self.sock.write_all(req)
    }

    fn reply(&mut self) -> io::Result<(Vec<u8>, Vec<u8>)> {
        let mut head = vec![0u8; 32];
        self.sock.read_exact(&mut head)?;
        if head[0] == 0 {
            return Err(io::Error::other(format!("X error {}", head[1])));
        }
        let mut data = vec![0u8; read_u32(&head, 4)? as usize * 4];
        self.sock.read_exact(&mut data)?;
        Ok((head, data))
    }

    fn query_tree(&mut self, window: u32) -> io::Result<Vec<u32>> {
        let mut req = [0u8; 8];
        req[0] = 15;
        req[2] = 2;
        req[4..].copy_from_slice(&window.to_le_bytes());
        self.request(&req)?;
        let (head, data) = self.reply()?;
        (0..read_u16(&head, 16)? as usize)
            .map(|i| read_u32(&data, i * 4))
            .collect()
    }

    fn name(&mut self, window: u32) -> io::Result<String> {
        let mut req = [0u8; 24];
        req[0] = 20;
        req[2] = 6;
        req[4..8].copy_from_slice(&window.to_le_bytes());
        req[8..12].copy_from_slice(&ATOM_WM_NAME.to_le_bytes());
        req[20..24].copy_from_slice(&MAX_NAME.to_le_bytes());
        self.request(&req)?;
        let (head, data) = self.reply()?;
        let len = (read_u32(&head, 16)? as usize).min(data.len());
        Ok(String::from_utf8_lossy(&data[..len]).into_owned())
    }

    fn find(&mut self, prefix: &str, id: &str) -> io::Result<Option<u32>> {
        let mut found = None;
        let mut queue = vec![self.root];
        while let Some(window) = queue.pop() {
            for child in self.query_tree(window)? {
                if is_match(&self.name(child)?, prefix, id) {
                    found = Some(child);
                }
                queue.push(child);
            }
        }
        Ok(found)
    }

    fn send_key(&mut self, window: u32, keycode: u8, kind: u8) -> io::Result<()> {
        let mut req = [0u8; 44];
        req[0] = 25;
        req[2] = 11;
        req[4..8].copy_from_slice(&window.to_le_bytes());
        let ev = &mut req[12..];
        ev[0] = kind;
        ev[1] = keycode;
        ev[8..12].copy_from_slice(&self.root.to_le_bytes());
        ev[12..16].copy_from_slice(&window.to_le_bytes());
        ev[30] = 1;
        self.request(&req)
    }
}

fn short() -> io::Error {
    io::Error::other("short X reply")
}

fn read_u16(buf: &[u8], at: usize) -> io::Result<u16> {
    buf.get(at..at + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(short)
}

fn read_u32(buf: &[u8], at: usize) -> io::Result<u32> {
    buf.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(short)
}

#[cfg(test)]
mod tests {
    use super::*;

    const READER: &str = "L:A_N:application_ID:com.lab126.booklet.reader_M:false_WT:true_S:-2";

    #[test]
    fn the_window_in_front_is_matched() {
        let (prefix, id) = title_parts(READER).unwrap();
        assert_eq!(prefix, "L:A_N:application_");
        assert_eq!(id, "com.lab126.booklet.reader");
        assert!(is_match(READER, prefix, id));
        assert!(is_match(&format!("{}_DM:N", READER), prefix, id));
        assert!(!is_match(
            "L:C_N:footerBar_ID:com.lab126.booklet.reader_M:false",
            prefix,
            id
        ));
        assert!(!is_match(
            "L:C_N:appToolBar_owner:com.lab126.booklet.reader_ID:com.lab126.KPPMainApp",
            prefix,
            id
        ));
        assert!(!is_match(
            "L:A_N:application_ID:com.lab126.booklet.home_M:false",
            prefix,
            id
        ));
        assert!(!is_match(
            "L:A_N:application_ID:com.lab126.booklet.reader2_M:false",
            prefix,
            id
        ));
    }

    #[test]
    fn titles_without_an_id_are_not_ours() {
        assert_eq!(title_parts("mesquite"), None);
        assert_eq!(title_parts("ID:com.lab126.krpp"), None);
    }
}
