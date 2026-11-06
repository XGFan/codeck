//! Connection to a Holder's unix socket (toh frame protocol). A reader thread keeps a VT of the
//! Pane from the Holder's Snapshots and Output at the PTY size the Holder reports (Size frames).
//! Attaching changes nothing; [`HolderConn::resize`] sets the PTY size (ADR-0001).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use toh_proto::{AttachInfo, Decoder, Frame};

use crate::screen::{Screen, Vt};
use crate::wake::Wake;

struct Shared {
    vt: Vt,
    /// Bumped on every change of the screen (Snapshot or Output).
    seen: u64,
    /// Number of Snapshots received.
    snaps: u64,
    /// The PTY size the Holder last reported (`None` before its first Size frame).
    size: Option<(u16, u16)>,
    exit: Option<i32>,
    closed: bool,
}

struct Inner {
    m: Mutex<Shared>,
    cv: Condvar,
    /// Also woken on every frame (the engine loop).
    wake: Mutex<Option<Arc<Wake>>>,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.m.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn update(&self, f: impl FnOnce(&mut Shared)) {
        f(&mut self.lock());
        self.cv.notify_all();
        if let Some(w) = self.wake.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            w.bump();
        }
    }
}

pub struct HolderConn {
    writer: Arc<Mutex<UnixStream>>,
    inner: Arc<Inner>,
}

/// Bracketed paste markers.
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

impl HolderConn {
    /// Attaches, asking for the PTY size (the Holder sends it before its first Snapshot), and waits
    /// for that Snapshot. A Holder that sends no size is older than ccrpc: refused.
    pub fn connect(socket: &Path) -> Result<HolderConn> {
        let stream = UnixStream::connect(socket)
            .with_context(|| format!("连不上 Holder {}", socket.display()))?;
        let mut reader = stream.try_clone()?;
        // The size in Attach is only the client's viewport; attaching never changes the PTY.
        let attach = Frame::Attach(AttachInfo {
            client: "ccrpc".into(),
            cols: 80,
            rows: 24,
            size_events: true,
            ..Default::default()
        });
        (&stream).write_all(&attach.encode())?;
        let inner = Arc::new(Inner {
            m: Mutex::new(Shared {
                vt: Vt::new(80, 24),
                seen: 0,
                snaps: 0,
                size: None,
                exit: None,
                closed: false,
            }),
            cv: Condvar::new(),
            wake: Mutex::new(None),
        });
        let writer = Arc::new(Mutex::new(stream));
        let shared = inner.clone();
        let snapshot_req = writer.clone();
        std::thread::spawn(move || {
            let mut dec = Decoder::new();
            let mut buf = vec![0u8; 64 * 1024];
            'read: loop {
                let n = match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                dec.push(&buf[..n]);
                loop {
                    match dec.next_frame() {
                        Ok(Some(Frame::Snapshot(b))) => shared.update(|s| {
                            s.vt.reset_to(&b);
                            s.seen += 1;
                            s.snaps += 1;
                        }),
                        Ok(Some(Frame::Output(b))) => shared.update(|s| {
                            s.vt.feed(&b);
                            s.seen += 1;
                        }),
                        Ok(Some(Frame::Size { cols, rows })) => {
                            let mut changed = false;
                            shared.update(|s| {
                                changed = s.size.is_some();
                                s.vt.resize(cols, rows);
                                s.size = Some((cols, rows));
                                s.seen += 1;
                            });
                            // Claude Code redraws for the new size; start the VT over from a
                            // Snapshot rather than reflowing what it had (the first size comes
                            // before the attach Snapshot anyway).
                            if changed && let Ok(mut w) = snapshot_req.lock() {
                                let _ = w.write_all(&Frame::SnapshotReq.encode());
                            }
                        }
                        Ok(Some(Frame::Exit { code })) => shared.update(|s| s.exit = Some(code)),
                        Ok(Some(_)) => {}
                        Ok(None) => break,
                        Err(_) => break 'read,
                    }
                }
            }
            shared.update(|s| s.closed = true);
        });
        let conn = HolderConn { writer, inner };
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut s = conn.inner.lock();
        while s.snaps == 0 {
            Self::check_open(&s)?;
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!("等 Holder 快照超时");
            }
            s = conn
                .inner
                .cv
                .wait_timeout(s, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        let old = s.size.is_none();
        drop(s);
        if old {
            conn.detach();
            bail!(
                "这个 Pane 的 Holder 版本太旧，不报告终端尺寸：关掉这个 Pane 重开（claude --resume 可以接着原来的 Session）"
            );
        }
        Ok(conn)
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

    fn check_open(s: &Shared) -> Result<()> {
        if let Some(c) = s.exit {
            bail!("Session 已结束（shell 退出码 {c}）");
        }
        if s.closed {
            bail!("Holder 连接已关闭");
        }
        Ok(())
    }

    /// Current screen as kept from the stream.
    pub fn screen(&self) -> Screen {
        self.inner.lock().vt.screen()
    }

    /// Screen generation: changes whenever the screen may have changed.
    pub fn generation(&self) -> u64 {
        self.inner.lock().seen
    }

    /// Blocks until the screen generation differs from `seen`, the connection ends, or `timeout`
    /// passes; returns whether it changed.
    pub fn wait_change(&self, seen: u64, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut s = self.inner.lock();
        loop {
            if s.seen != seen {
                return true;
            }
            if s.closed || s.exit.is_some() {
                return false;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            s = self
                .inner
                .cv
                .wait_timeout(s, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Asks for a fresh Snapshot and waits for it (the VT starts over from it).
    pub fn resync(&self, timeout: Duration) -> Result<Screen> {
        let before = {
            let s = self.inner.lock();
            Self::check_open(&s)?;
            s.snaps
        };
        self.send(Frame::SnapshotReq)?;
        let deadline = Instant::now() + timeout;
        let mut s = self.inner.lock();
        loop {
            Self::check_open(&s)?;
            if s.snaps > before {
                return Ok(s.vt.screen());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!("等 Holder 快照超时");
            }
            s = self
                .inner
                .cv
                .wait_timeout(s, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Bumps `wake` on every frame from now on.
    pub fn set_wake(&self, wake: Arc<Wake>) {
        *self.inner.wake.lock().unwrap_or_else(|e| e.into_inner()) = Some(wake);
    }

    /// The PTY size the Holder last reported.
    pub fn size(&self) -> (u16, u16) {
        self.inner.lock().size.unwrap_or((80, 24))
    }

    /// Sets the PTY size (ccrpc becomes its owner until a person's Client takes it); the VT
    /// follows once the Holder reports the new size.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.send(Frame::Resize { cols, rows })
    }

    /// True once the Holder has closed the connection or reported Exit.
    pub fn poll_closed(&self) -> bool {
        let s = self.inner.lock();
        s.closed || s.exit.is_some()
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

/// The keys ccrpc presses itself (legacy xterm encodings, as Claude Code reads them from a plain
/// PTY); the same bytes as their names in [`key_bytes`].
pub mod key {
    pub const ENTER: &[u8] = b"\r";
    pub const ESC: &[u8] = b"\x1b";
    pub const SHIFT_TAB: &[u8] = b"\x1b[Z";
    pub const BACKSPACE: &[u8] = b"\x7f";
    pub const UP: &[u8] = b"\x1b[A";
    pub const DOWN: &[u8] = b"\x1b[B";
    pub const RIGHT: &[u8] = b"\x1b[C";
    pub const LEFT: &[u8] = b"\x1b[D";
    pub const CTRL_U: &[u8] = b"\x15";
    /// `ctrl+enter` (kitty keyboard protocol): send what waits in Claude Code's queue now. Not
    /// offered to `send_keys` by name.
    pub const CTRL_ENTER: &[u8] = b"\x1b[13;5u";
}

/// Named key → bytes (legacy xterm encodings, as Claude Code reads them from a plain PTY).
pub fn key_bytes(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "enter" | "return" => key::ENTER,
        "esc" | "escape" => key::ESC,
        "tab" => b"\t",
        "shift+tab" | "backtab" => key::SHIFT_TAB,
        "backspace" => key::BACKSPACE,
        "space" => b" ",
        "up" => key::UP,
        "down" => key::DOWN,
        "right" => key::RIGHT,
        "left" => b"\x1b[D",
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "pageup" => b"\x1b[5~",
        "pagedown" => b"\x1b[6~",
        "delete" => b"\x1b[3~",
        "ctrl+c" => b"\x03",
        "ctrl+d" => b"\x04",
        "ctrl+u" => key::CTRL_U,
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
