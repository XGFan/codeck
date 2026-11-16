//! The Holder side: where a Pane's Holder listens (`toh ls --json`), and a connection to it that hands
//! what it sends on as tagged messages for xterm.js. Attaching never changes the PTY size and no
//! `Resize` is ever sent: the size belongs to the person's ToH Client, or to ccrpc when it cannot
//! read the screen (ccrpc ADR-0001).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use codeck_host::ccrpc::relay::output_within;
use serde::Deserialize;
use toh_proto::{AttachInfo, Decoder, Frame};

/// Message tags, the first byte of each message.
pub const SIZE: u8 = b'S';
pub const HISTORY: u8 = b'B';
pub const SNAPSHOT: u8 = b'P';
pub const OUTPUT: u8 = b'O';
pub const EXIT: u8 = b'X';
pub const CLOSED: u8 = b'C';

#[derive(Deserialize)]
struct Pane {
    id: String,
    socket: PathBuf,
}

/// The socket of Holder `id` in `toh ls --json`'s output.
pub fn socket_in(list: &[u8], id: &str) -> Result<PathBuf, String> {
    let panes: Vec<Pane> =
        serde_json::from_slice(list).map_err(|_| "toh ls --json 的输出不是 JSON".to_string())?;
    panes
        .into_iter()
        .find(|p| p.id == id)
        .map(|p| p.socket)
        .ok_or_else(|| format!("找不到 Pane {id}"))
}

/// Runs `toh ls --json` in `env` (the login shell's: an app started from the Dock has no `toh` on PATH),
/// without the variables of a terminal CoDeck may have been started from (as ccrpc runs `toh`).
pub fn locate(env: &HashMap<String, String>, id: &str) -> Result<PathBuf, String> {
    let mut cmd = Command::new("toh");
    cmd.env_clear().envs(env).args(["ls", "--json"]);
    for k in [
        "TOH_CONFIG",
        "TOH_HOLDER_ID",
        "KITTY_WINDOW_ID",
        "KITTY_LISTEN_ON",
        "WEZTERM_PANE",
        "WEZTERM_UNIX_SOCKET",
        "KAKU_UNIX_SOCKET",
        "ITERM_SESSION_ID",
    ] {
        cmd.env_remove(k);
    }
    // Through the Host's spawn lock: CoDeck forks ccrpc on other threads (see `output_within`).
    let out =
        output_within(cmd, Duration::from_secs(10)).map_err(|e| format!("运行 toh 失败：{e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "toh ls 失败：{}",
            err.trim().lines().next().unwrap_or_default()
        ));
    }
    socket_in(&out.stdout, id)
}

/// One attached connection to a Holder. Everything written to it goes through one writer thread, in
/// order (typed bytes and the reader's snapshot requests alike), so a Holder that stops reading never
/// blocks the caller.
pub struct Link {
    frames: Sender<Frame>,
}

impl Link {
    /// Attaches to the Holder at `socket`; a reader thread passes every message to `sink` until the
    /// connection ends (then a final [`CLOSED`]) or `sink` returns false.
    pub fn attach(
        socket: &Path,
        mut sink: impl FnMut(Vec<u8>) -> bool + Send + 'static,
    ) -> Result<Link, String> {
        let stream = UnixStream::connect(socket).map_err(|e| format!("连不上 Pane：{e}"))?;
        let attach = Frame::Attach(AttachInfo {
            client: "codeck-term".into(),
            // Only this client's viewport: attaching never changes the PTY.
            cols: 80,
            rows: 24,
            size_events: true,
            ..Default::default()
        });
        (&stream)
            .write_all(&attach.encode())
            .map_err(|e| format!("连不上 Pane：{e}"))?;
        let mut reader = stream.try_clone().map_err(|e| e.to_string())?;
        let (frames, queue) = mpsc::channel::<Frame>();
        let mut writer = stream;
        std::thread::spawn(move || {
            for f in queue {
                let last = f == Frame::Detach;
                if writer.write_all(&f.encode()).is_err() || last {
                    break;
                }
            }
            // Ends the reader too.
            let _ = writer.shutdown(Shutdown::Both);
        });
        let requests = frames.clone();
        std::thread::spawn(move || {
            let mut dec = Decoder::new();
            let mut buf = vec![0u8; 64 * 1024];
            let mut sized = false;
            'read: loop {
                let n = match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                dec.push(&buf[..n]);
                loop {
                    let frame = match dec.next_frame() {
                        Ok(Some(f)) => f,
                        Ok(None) => break,
                        Err(_) => break 'read,
                    };
                    let msg = match frame {
                        Frame::Size { cols, rows } => {
                            // Claude Code redraws for the new size: start over from a Snapshot rather than
                            // reflowing (the first size comes before the attach Snapshot anyway).
                            if sized {
                                let _ = requests.send(Frame::SnapshotReq);
                            }
                            sized = true;
                            let mut m = vec![SIZE];
                            m.extend_from_slice(&cols.to_le_bytes());
                            m.extend_from_slice(&rows.to_le_bytes());
                            m
                        }
                        Frame::Scrollback { data, .. } => tagged(HISTORY, &data),
                        Frame::Snapshot(b) => tagged(SNAPSHOT, &b),
                        Frame::Output(b) => tagged(OUTPUT, &b),
                        Frame::Exit { code } => tagged(EXIT, &code.to_le_bytes()),
                        _ => continue,
                    };
                    if !sink(msg) {
                        let _ = reader.shutdown(Shutdown::Both);
                        return;
                    }
                }
            }
            // Also after a frame this toh-proto cannot read: no half-open client left on the Holder.
            let _ = reader.shutdown(Shutdown::Both);
            sink(vec![CLOSED]);
        });
        Ok(Link { frames })
    }

    /// Queues typed bytes for the PTY.
    pub fn input(&self, bytes: Vec<u8>) -> Result<(), String> {
        self.frames
            .send(Frame::Input(bytes))
            .map_err(|_| "与 Pane 的连接已断开".to_string())
    }
}

impl Drop for Link {
    /// Detaches (after what was queued before); the Session keeps running.
    fn drop(&mut self) {
        let _ = self.frames.send(Frame::Detach);
    }
}

fn tagged(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(body.len() + 1);
    m.push(tag);
    m.extend_from_slice(body);
    m
}
