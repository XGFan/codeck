//! Connection to a Holder's unix socket (toh frame protocol). A reader thread keeps a VT of the
//! Pane from the Holder's Snapshots and Output, and the Holder's reports of who owns the PTY size.
//! Attaching changes nothing; [`HolderConn::claim_size`] makes ccrpc the size owner (ADR-0001).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use toh_proto::{AttachInfo, Decoder, Frame};

use crate::screen::{Screen, Vt};
use crate::wake::Wake;

/// Who owns the PTY size, as the Holder reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    /// No report yet (an older Holder never sends one).
    Unknown,
    Nobody,
    Me,
    /// Another attached client, by its client name.
    Other(String),
}

struct Shared {
    vt: Vt,
    /// Bumped on every change of the screen (Snapshot or Output).
    seen: u64,
    /// Number of Snapshots received.
    snaps: u64,
    owner: Owner,
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
    /// Attaches (asking for size-owner reports) with a VT of `cols` × `rows`, the PTY's size.
    pub fn connect(socket: &Path, cols: u16, rows: u16) -> Result<HolderConn> {
        let stream = UnixStream::connect(socket)
            .with_context(|| format!("连不上 Holder {}", socket.display()))?;
        let mut reader = stream.try_clone()?;
        let attach = Frame::Attach(AttachInfo {
            client: "ccrpc".into(),
            cols,
            rows,
            owner_events: true,
            ..Default::default()
        });
        (&stream).write_all(&attach.encode())?;
        let inner = Arc::new(Inner {
            m: Mutex::new(Shared {
                vt: Vt::new(cols, rows),
                seen: 0,
                snaps: 0,
                owner: Owner::Unknown,
                exit: None,
                closed: false,
            }),
            cv: Condvar::new(),
            wake: Mutex::new(None),
        });
        let shared = inner.clone();
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
                        Ok(Some(Frame::Owner(o))) => shared.update(|s| {
                            s.owner = match (o.you, o.client) {
                                (true, _) => Owner::Me,
                                (false, Some(c)) => Owner::Other(c),
                                (false, None) => Owner::Nobody,
                            };
                        }),
                        Ok(Some(Frame::Exit { code })) => shared.update(|s| s.exit = Some(code)),
                        Ok(Some(_)) => {}
                        Ok(None) => break,
                        Err(_) => break 'read,
                    }
                }
            }
            shared.update(|s| s.closed = true);
        });
        Ok(HolderConn {
            writer: Arc::new(Mutex::new(stream)),
            inner,
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

    /// Waits until the attach Snapshot has arrived and, when the Holder reports size owners, its
    /// first report (sent right after the Snapshot); returns the owner (`Unknown` for an older
    /// Holder).
    pub fn initial_owner(&self, timeout: Duration) -> Result<Owner> {
        let deadline = Instant::now() + timeout;
        let mut snap_at: Option<Instant> = None;
        let mut s = self.inner.lock();
        loop {
            Self::check_open(&s)?;
            if s.owner != Owner::Unknown {
                return Ok(s.owner.clone());
            }
            let now = Instant::now();
            if s.snaps > 0 && snap_at.is_none() {
                snap_at = Some(now);
            }
            // The report follows the Snapshot in the same burst; an older Holder never sends it.
            if snap_at.is_some_and(|t| now >= t + Duration::from_millis(500)) || now >= deadline {
                if s.snaps == 0 {
                    bail!("等 Holder 快照超时");
                }
                return Ok(Owner::Unknown);
            }
            s = self
                .inner
                .cv
                .wait_timeout(s, Duration::from_millis(50))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Bumps `wake` on every frame from now on.
    pub fn set_wake(&self, wake: Arc<Wake>) {
        *self.inner.wake.lock().unwrap_or_else(|e| e.into_inner()) = Some(wake);
    }

    pub fn owner(&self) -> Owner {
        self.inner.lock().owner.clone()
    }

    /// Size of the VT.
    pub fn vt_size(&self) -> (u16, u16) {
        self.inner.lock().vt.size()
    }

    /// Makes ccrpc the PTY size owner at `cols` × `rows`, then resyncs the screen.
    pub fn claim_size(&self, cols: u16, rows: u16) -> Result<()> {
        self.inner.update(|s| s.vt.resize(cols, rows));
        self.send(Frame::Resize { cols, rows })?;
        self.resync(Duration::from_secs(3)).map(|_| ())
    }

    /// Follows a PTY size set by someone else: resizes the VT and resyncs.
    pub fn follow_size(&self, cols: u16, rows: u16) -> Result<()> {
        self.inner.update(|s| s.vt.resize(cols, rows));
        self.resync(Duration::from_secs(3)).map(|_| ())
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
