//! Connection to a Holder's unix socket as an ordinary attached client (toh frame protocol).
//! Never sends Resize, so the PTY size stays with whoever owns it.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use toh_proto::{AttachInfo, Decoder, Frame};

/// What the reader thread forwards (Output is dropped: the screen is rebuilt from Snapshots).
#[derive(Debug)]
pub enum HolderEvent {
    Snapshot(Vec<u8>),
    Exit(i32),
    Closed,
}

pub struct HolderConn {
    writer: Arc<Mutex<UnixStream>>,
    events: Receiver<HolderEvent>,
}

/// Bracketed paste markers.
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

impl HolderConn {
    pub fn connect(socket: &Path) -> Result<HolderConn> {
        let stream = UnixStream::connect(socket)
            .with_context(|| format!("连不上 Holder {}", socket.display()))?;
        let mut reader = stream.try_clone()?;
        let attach = Frame::Attach(AttachInfo {
            client: "ccrpc".into(),
            cols: 80,
            rows: 24,
            ..Default::default()
        });
        (&stream).write_all(&attach.encode())?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut dec = Decoder::new();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                dec.push(&buf[..n]);
                loop {
                    match dec.next_frame() {
                        Ok(Some(Frame::Snapshot(b))) => {
                            if tx.send(HolderEvent::Snapshot(b)).is_err() {
                                return;
                            }
                        }
                        Ok(Some(Frame::Exit { code })) => {
                            let _ = tx.send(HolderEvent::Exit(code));
                        }
                        Ok(Some(_)) => {}
                        Ok(None) => break,
                        Err(_) => {
                            let _ = tx.send(HolderEvent::Closed);
                            return;
                        }
                    }
                }
            }
            let _ = tx.send(HolderEvent::Closed);
        });
        Ok(HolderConn {
            writer: Arc::new(Mutex::new(stream)),
            events: rx,
        })
    }

    fn send(&self, frame: Frame) -> Result<()> {
        let mut w = self
            .writer
            .lock()
            .map_err(|_| anyhow!("Holder 连接已损坏"))?;
        w.write_all(&frame.encode()).context("写 Holder 失败")
    }

    /// Writes raw bytes into the PTY.
    pub fn input(&self, bytes: &[u8]) -> Result<()> {
        self.send(Frame::Input(bytes.to_vec()))
    }

    /// Writes `text` as one bracketed paste (control characters other than `\n` / `\t` removed).
    pub fn paste(&self, text: &str) -> Result<()> {
        let mut b = Vec::with_capacity(text.len() + 12);
        b.extend_from_slice(PASTE_START);
        b.extend_from_slice(sanitize(text).as_bytes());
        b.extend_from_slice(PASTE_END);
        self.input(&b)
    }

    /// Requests and waits for a fresh Snapshot; returns `Err` if the Holder exits or closes.
    pub fn snapshot(&self, timeout: Duration) -> Result<Vec<u8>> {
        // Drop stale Snapshots (the one sent on attach, or answers to earlier requests).
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                HolderEvent::Snapshot(_) => {}
                HolderEvent::Exit(c) => bail!("Session 已结束（shell 退出码 {c}）"),
                HolderEvent::Closed => bail!("Holder 连接已关闭"),
            }
        }
        self.send(Frame::SnapshotReq)?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left) {
                Ok(HolderEvent::Snapshot(b)) => return Ok(b),
                Ok(HolderEvent::Exit(c)) => bail!("Session 已结束（shell 退出码 {c}）"),
                Ok(HolderEvent::Closed) | Err(RecvTimeoutError::Disconnected) => {
                    bail!("Holder 连接已关闭")
                }
                Err(RecvTimeoutError::Timeout) => bail!("等 Holder 快照超时"),
            }
        }
    }

    /// True once the Holder has closed the connection or reported Exit (non-blocking).
    pub fn poll_closed(&self) -> bool {
        loop {
            match self.events.try_recv() {
                Ok(HolderEvent::Snapshot(_)) => continue,
                Ok(HolderEvent::Exit(_)) | Ok(HolderEvent::Closed) => return true,
                Err(mpsc::TryRecvError::Empty) => return false,
                Err(mpsc::TryRecvError::Disconnected) => return true,
            }
        }
    }

    pub fn detach(&self) {
        let _ = self.send(Frame::Detach);
    }
}

/// Removes control characters except `\n` and `\t`; `\r\n` / `\r` become `\n`.
pub fn sanitize(text: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

/// Named key → bytes (legacy xterm encodings, as Claude Code reads them from a plain PTY).
pub fn key_bytes(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "enter" | "return" => b"\r",
        "esc" | "escape" => b"\x1b",
        "tab" => b"\t",
        "shift+tab" | "backtab" => b"\x1b[Z",
        "backspace" => b"\x7f",
        "space" => b" ",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "pageup" => b"\x1b[5~",
        "pagedown" => b"\x1b[6~",
        "delete" => b"\x1b[3~",
        "ctrl+c" => b"\x03",
        "ctrl+d" => b"\x04",
        "ctrl+u" => b"\x15",
        "ctrl+l" => b"\x0c",
        "ctrl+o" => b"\x0f",
        "ctrl+r" => b"\x12",
        "ctrl+t" => b"\x14",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_newlines_and_drops_escapes() {
        assert_eq!(sanitize("a\r\nb\x1b[31mc\td\x07"), "a\nb[31mc\td");
    }

    #[test]
    fn named_keys() {
        assert_eq!(key_bytes("shift+tab"), Some(&b"\x1b[Z"[..]));
        assert_eq!(key_bytes("nope"), None);
    }
}
