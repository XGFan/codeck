//! The ccrpc Plugin's side (`ccrpc hook`, run by Claude Code for every hook event) and ccrpc's
//! side of the hook channel (ADR-0003).
//!
//! - Only inside a clay Pane (`TOH_HOLDER_ID` set); elsewhere every hook exits at once.
//! - Never writes to stdout except a PermissionRequest decision ccrpc sent back: nothing a hook
//!   prints may reach the model's context.
//! - SessionStart checks in: `<state>/sessions/<claude pid>.json` says the Session has the plugin
//!   and where its transcript is. Other events go to the ccrpc connected to the Session over
//!   `<state>/s/<claude pid>.sock`; with nobody listening the hook just exits.
//! - A PermissionRequest waits for ccrpc's one-line reply; ccrpc going away (EOF) releases it with
//!   no output, so Claude Code's own dialog (shown meanwhile) is answered by the person.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::wake::Wake;

/// Version of the plugin (its hook events and the line protocol between `ccrpc hook` and ccrpc);
/// recorded in the check-in. 2: SubagentStart / SubagentStop.
pub const HOOK_PROTOCOL: u32 = 2;

/// ccrpc's own state directory: `CCRPC_STATE_DIR`, else `$XDG_STATE_HOME/ccrpc`, else
/// `~/.local/state/ccrpc`. Never inside Claude Code's or ToH's directories.
pub fn state_dir() -> PathBuf {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(d) = env("CCRPC_STATE_DIR") {
        return d;
    }
    if let Some(d) = env("XDG_STATE_HOME") {
        return d.join("ccrpc");
    }
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state/ccrpc")
}

pub fn checkin_path(state: &Path, claude_pid: u32) -> PathBuf {
    state.join("sessions").join(format!("{claude_pid}.json"))
}

pub fn socket_path(state: &Path, claude_pid: u32) -> PathBuf {
    state.join("s").join(format!("{claude_pid}.sock"))
}

/// Another ccrpc listens on the Session's hook socket.
pub fn socket_in_use(state: &Path, claude_pid: u32) -> bool {
    UnixStream::connect(socket_path(state, claude_pid)).is_ok()
}

/// What the plugin's SessionStart hook records for one Claude Code process.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CheckIn {
    pub hook_protocol: u32,
    pub plugin_version: String,
    pub claude_pid: u32,
    pub holder_id: String,
    pub session_id: String,
    pub transcript_path: String,
    /// `startup` / `resume` / `clear` / `compact` / `fork`.
    pub source: String,
    #[serde(default)]
    pub cwd: String,
    /// Unix seconds.
    pub at: u64,
}

/// The check-in of `claude_pid`, if its plugin checked in.
pub fn read_checkin(state: &Path, claude_pid: u32) -> Option<CheckIn> {
    let bytes = std::fs::read(checkin_path(state, claude_pid)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Why a Session cannot be controlled through its check-in, or `None` when it can.
pub fn checkin_problem(c: Option<&CheckIn>, process_session: Option<&str>) -> Option<String> {
    let Some(c) = c else {
        return Some(
            "这个 Claude Code 没有加载 ccrpc 插件：先运行 ccrpc plugin install，再重启 claude（如 claude --resume <sessionId>）".into(),
        );
    };
    if c.hook_protocol != HOOK_PROTOCOL {
        return Some(format!(
            "ccrpc 插件的 hook 协议是 {}，本 ccrpc 是 {HOOK_PROTOCOL}：重新运行 ccrpc plugin install 并重启 claude",
            c.hook_protocol
        ));
    }
    // A pid reused by another claude that never checked in still has the old record.
    if let Some(s) = process_session
        && s != c.session_id
    {
        return Some(format!(
            "进程文件的 sessionId（{s}）与 ccrpc 插件的报到（{}）不一致：这个 claude 可能没有加载插件，重启 claude 后再试",
            c.session_id
        ));
    }
    None
}

/// `dir` is a real directory of this user that nobody else can touch (0700): only then are files
/// in it trusted (the hook socket decides approvals).
pub fn dir_is_private(dir: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(md) = std::fs::symlink_metadata(dir) else {
        return false;
    };
    // SAFETY: geteuid has no preconditions.
    let me = unsafe { libc::geteuid() };
    md.is_dir() && md.uid() == me && md.mode() & 0o077 == 0
}

/// Creates `dir` (0700) if missing and checks it is private.
fn private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("建不了 {}", dir.display()))?;
    if !dir_is_private(dir) {
        bail!("{} 不是只属于当前用户的目录（须为 0700）", dir.display());
    }
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path.parent().context("路径没有上级目录")?;
    private_dir(dir)?;
    let tmp = dir.join(format!(
        ".{}.{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("f"),
        std::process::id()
    ));
    let _ = std::fs::remove_file(&tmp);
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(libc::O_NOFOLLOW)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

// ---- the hook side ---------------------------------------------------------------------------

/// `ccrpc hook`: one hook event on stdin. Always exits 0; prints only a decision ccrpc sent.
pub fn run_hook() {
    // Outside a clay Pane there is nothing for ccrpc to do.
    let Some(holder_id) = std::env::var("TOH_HOLDER_ID").ok().filter(|v| !v.is_empty()) else {
        return;
    };
    let mut raw = String::new();
    if std::io::stdin().read_to_string(&mut raw).is_err() {
        return;
    }
    let Ok(input) = serde_json::from_str::<Value>(&raw) else {
        return;
    };
    // Claude Code runs the hook command as its own child.
    let claude_pid = std::os::unix::process::parent_id();
    let state = state_dir();
    let event = input["hook_event_name"].as_str().unwrap_or_default().to_string();
    if event == "SessionStart" {
        let c = CheckIn {
            hook_protocol: HOOK_PROTOCOL,
            plugin_version: env!("CARGO_PKG_VERSION").into(),
            claude_pid,
            holder_id,
            session_id: input["session_id"].as_str().unwrap_or_default().into(),
            transcript_path: input["transcript_path"].as_str().unwrap_or_default().into(),
            source: input["source"].as_str().unwrap_or_default().into(),
            cwd: input["cwd"].as_str().unwrap_or_default().into(),
            at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };
        if let Ok(b) = serde_json::to_vec(&c) {
            let _ = write_private(&checkin_path(&state, claude_pid), &b);
        }
    }
    if let Some(out) = forward(&socket_path(&state, claude_pid), &event, input) {
        let mut so = std::io::stdout();
        let _ = so.write_all(out.as_bytes());
        let _ = so.flush();
    }
}

/// Sends the event to the listening ccrpc; for a PermissionRequest waits for its reply and
/// returns the hook output to print (none when ccrpc passes or goes away).
fn forward(sock: &Path, event: &str, input: Value) -> Option<String> {
    // Only a socket in this user's private directory speaks for ccrpc.
    if !dir_is_private(sock.parent()?) {
        return None;
    }
    let mut s = UnixStream::connect(sock).ok()?;
    let line = serde_json::to_string(&json!({"v": HOOK_PROTOCOL, "input": input})).ok()?;
    s.write_all(line.as_bytes()).ok()?;
    s.write_all(b"\n").ok()?;
    if event != "PermissionRequest" {
        return None;
    }
    let mut reply = String::new();
    BufReader::new(s).read_line(&mut reply).ok()?;
    let v: Value = serde_json::from_str(reply.trim()).ok()?;
    let out = v.get("output").filter(|o| !o.is_null())?;
    serde_json::to_string(&checked_decision(out)?).ok()
}

/// The PermissionRequest output rebuilt from only the fields ccrpc ever sends: allow (with an
/// updated input, permission updates other than mode switches) or deny (message, interrupt).
/// Anything else is no decision.
fn checked_decision(out: &Value) -> Option<Value> {
    let d = out.get("hookSpecificOutput")?.get("decision")?;
    match d.get("behavior")?.as_str()? {
        "allow" => {
            let perms = d.get("updatedPermissions").map(|p| {
                Value::Array(
                    p.as_array()
                        .into_iter()
                        .flatten()
                        .filter(|r| r["type"].as_str().is_some_and(|t| t != "setMode"))
                        .cloned()
                        .collect(),
                )
            });
            let input = d.get("updatedInput").filter(|i| i.is_object()).cloned();
            Some(allow_output(perms, input))
        }
        "deny" => {
            let msg = d.get("message").and_then(Value::as_str);
            let interrupt = d.get("interrupt").and_then(Value::as_bool).unwrap_or(false);
            let mut o = deny_output(msg);
            if msg.is_some() && interrupt {
                o["hookSpecificOutput"]["decision"]["interrupt"] = json!(true);
            }
            Some(o)
        }
        _ => None,
    }
}

// ---- ccrpc's side ----------------------------------------------------------------------------

/// One hook event received from the Session's plugin.
#[derive(Debug)]
pub struct HookMsg {
    pub event: String,
    pub input: Value,
    /// A PermissionRequest's connection: the hook waits on it for the reply.
    pub reply: Option<UnixStream>,
}

/// The waiting hook on the other end of `s` is gone (killed by Claude Code: answered in the
/// dialog, Escape, its timeout).
pub fn peer_gone(s: &UnixStream) -> bool {
    use std::os::fd::AsRawFd;
    let mut b = [0u8; 1];
    // SAFETY: a one-byte non-blocking peek into a local buffer on an fd `s` owns.
    let n = unsafe {
        libc::recv(s.as_raw_fd(), b.as_mut_ptr().cast(), 1, libc::MSG_PEEK | libc::MSG_DONTWAIT)
    };
    // 0: EOF. -1 with EAGAIN: still connected, nothing to read (the hook never writes again).
    n == 0 || (n < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EAGAIN))
}

/// Drops check-ins of claude processes that are gone.
pub fn prune_checkins(state: &Path) {
    let Ok(rd) = std::fs::read_dir(state.join("sessions")) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let pid = p
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u32>().ok());
        if let Some(pid) = pid
            && p.extension().is_some_and(|x| x == "json")
            && !crate::procs::pid_exists(pid)
        {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// The reply a waiting PermissionRequest hook prints (`None`: no decision, the person answers).
pub fn send_reply(mut s: UnixStream, output: Option<Value>) -> std::io::Result<()> {
    let line = json!({"output": output}).to_string();
    s.write_all(line.as_bytes())?;
    s.write_all(b"\n")?;
    s.flush()
}

/// PermissionRequest output allowing the call, optionally with permission updates or a changed
/// input (AskUserQuestion's answers).
pub fn allow_output(updated_permissions: Option<Value>, updated_input: Option<Value>) -> Value {
    let mut d = json!({"behavior": "allow"});
    if let Some(p) = updated_permissions {
        d["updatedPermissions"] = p;
    }
    if let Some(i) = updated_input {
        d["updatedInput"] = i;
    }
    json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": d}})
}

/// PermissionRequest output denying the call. Without a message it interrupts the turn, as the
/// dialog's No does; with one the model gets it as feedback and goes on.
pub fn deny_output(message: Option<&str>) -> Value {
    let d = match message {
        Some(m) => json!({"behavior": "deny", "message": m}),
        None => json!({"behavior": "deny", "interrupt": true}),
    };
    json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": d}})
}

/// Listens on the Session's hook socket for as long as it lives.
pub struct HookServer {
    path: PathBuf,
    rx: Receiver<HookMsg>,
    /// `<pid>.lock`, flocked while this ccrpc owns the socket.
    _lock: std::fs::File,
}

impl HookServer {
    /// Binds `<state>/s/<claude pid>.sock`; fails when another ccrpc already listens there.
    pub fn bind(state: &Path, claude_pid: u32, wake: Arc<Wake>) -> Result<HookServer> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        let path = socket_path(state, claude_pid);
        let dir = path.parent().context("socket 路径没有上级目录")?;
        private_dir(dir)?;
        // The lock, not the socket, says who owns the Session: two ccrpc starting at once cannot
        // both take a stale socket for theirs.
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .custom_flags(libc::O_NOFOLLOW)
            .mode(0o600)
            .open(path.with_extension("lock"))
            .context("打不开 hook socket 的锁文件")?;
        // SAFETY: flock on an fd this function owns.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            bail!("已有 ccrpc 连着这个 Session（{}）", path.display());
        }
        let _ = std::fs::remove_file(&path);
        prune_checkins(state);
        let listener =
            UnixListener::bind(&path).with_context(|| format!("监听不了 {}", path.display()))?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut s) = conn else { continue };
                let (tx, wake) = (tx.clone(), wake.clone());
                // Each connection on its own: a slow one cannot hold up the others.
                std::thread::spawn(move || {
                    let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                    let Some(input) = read_msg(&mut s) else { return };
                    let event = input["hook_event_name"].as_str().unwrap_or_default().to_string();
                    let reply = (event == "PermissionRequest").then(|| {
                        let _ = s.set_read_timeout(None);
                        s
                    });
                    if tx.send(HookMsg { event, input, reply }).is_ok() {
                        wake.bump();
                    }
                });
            }
        });
        Ok(HookServer { path, rx, _lock: lock })
    }

    pub fn try_recv(&self) -> Option<HookMsg> {
        self.rx.try_recv().ok()
    }
}

impl Drop for HookServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The hook input of one message (`{"v": …, "input": …}` on one line; the hook sends nothing
/// after it), within a few seconds.
fn read_msg(s: &mut UnixStream) -> Option<Value> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut r = BufReader::new(Read::take(&mut *s, 8 << 20));
    let mut buf = Vec::new();
    loop {
        if std::time::Instant::now() > deadline {
            return None;
        }
        match r.read_until(b'\n', &mut buf) {
            Ok(0) => return None,
            Ok(_) if buf.ends_with(b"\n") => break,
            Ok(_) => return None,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => return None,
        }
    }
    let v: Value = serde_json::from_slice(&buf).ok()?;
    if v["v"].as_u64() != Some(HOOK_PROTOCOL as u64) {
        return None;
    }
    Some(v["input"].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_interrupts_unless_there_is_feedback() {
        assert_eq!(
            deny_output(None)["hookSpecificOutput"]["decision"],
            json!({"behavior": "deny", "interrupt": true})
        );
        assert_eq!(
            deny_output(Some("用 cargo"))["hookSpecificOutput"]["decision"],
            json!({"behavior": "deny", "message": "用 cargo"})
        );
    }

    #[test]
    fn checkin_problems() {
        let c = CheckIn {
            hook_protocol: HOOK_PROTOCOL,
            plugin_version: "0.1.0".into(),
            claude_pid: 1,
            holder_id: "m1".into(),
            session_id: "s1".into(),
            transcript_path: "/t.jsonl".into(),
            source: "startup".into(),
            cwd: "/w".into(),
            at: 0,
        };
        assert!(checkin_problem(None, None).is_some());
        assert!(checkin_problem(Some(&c), Some("s1")).is_none());
        assert!(checkin_problem(Some(&c), None).is_none());
        assert!(checkin_problem(Some(&c), Some("s2")).is_some());
        let old = CheckIn { hook_protocol: 0, ..c };
        assert!(checkin_problem(Some(&old), Some("s1")).is_some());
    }

    #[test]
    fn only_decisions_ccrpc_sends_are_printed() {
        let allow = allow_output(Some(json!([{"type": "addDirectories", "directories": ["/w"]},
            {"type": "setMode", "mode": "bypassPermissions"}])), Some(json!({"answers": {}})));
        let d = checked_decision(&allow).unwrap();
        assert_eq!(
            d["hookSpecificOutput"]["decision"]["updatedPermissions"],
            json!([{"type": "addDirectories", "directories": ["/w"]}]),
            "mode switches never get through"
        );
        assert_eq!(d["hookSpecificOutput"]["decision"]["updatedInput"], json!({"answers": {}}));
        assert_eq!(checked_decision(&deny_output(None)).unwrap(), deny_output(None));
        assert_eq!(checked_decision(&deny_output(Some("no"))).unwrap(), deny_output(Some("no")));
        let extra = json!({"hookSpecificOutput": {"decision": {"behavior": "allow", "extra": 1}},
            "systemMessage": "x"});
        assert_eq!(checked_decision(&extra).unwrap(), allow_output(None, None));
        assert!(checked_decision(&json!({"hookSpecificOutput": {"decision": {"behavior": "ask"}}})).is_none());
        assert!(checked_decision(&json!({"continue": false})).is_none());
    }

    #[test]
    fn hook_trusts_only_a_private_directory() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let s = d.path().join("s");
        private_dir(&s).unwrap();
        assert!(dir_is_private(&s));
        std::fs::set_permissions(&s, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!dir_is_private(&s));
        assert!(private_dir(&s).is_err());
        let link = d.path().join("link");
        std::os::unix::fs::symlink(d.path(), &link).unwrap();
        assert!(!dir_is_private(&link), "a symlink is not the directory");
    }

    /// A hook event reaches the server; a PermissionRequest gets the server's reply, and its
    /// connection closing without a reply means no decision.
    #[test]
    fn hook_channel_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let wake = Wake::new();
        let server = HookServer::bind(dir.path(), 4242, wake.clone()).unwrap();
        assert!(HookServer::bind(dir.path(), 4242, wake.clone()).is_err(), "one ccrpc per Session");
        let sock = socket_path(dir.path(), 4242);

        assert_eq!(forward(&sock, "Stop", json!({"hook_event_name": "Stop", "prompt_id": "p"})), None);
        let m = recv(&server);
        assert_eq!((m.event.as_str(), m.input["prompt_id"].as_str()), ("Stop", Some("p")));
        assert!(m.reply.is_none());

        let ask = |server: &HookServer, out: Option<Value>| {
            let sock = sock.clone();
            let h = std::thread::spawn(move || {
                forward(&sock, "PermissionRequest", json!({"hook_event_name": "PermissionRequest"}))
            });
            let m = recv(server);
            assert!(!peer_gone(m.reply.as_ref().unwrap()), "the hook still waits");
            match out {
                Some(o) => send_reply(m.reply.unwrap(), Some(o)).unwrap(),
                None => drop(m.reply),
            }
            h.join().unwrap()
        };
        // A hook that went away while waiting.
        let h = {
            let sock = sock.clone();
            std::thread::spawn(move || {
                let mut c = UnixStream::connect(&sock).unwrap();
                let line = json!({"v": HOOK_PROTOCOL, "input": {"hook_event_name": "PermissionRequest"}});
                c.write_all(format!("{line}\n").as_bytes()).unwrap();
            })
        };
        let m = recv(&server);
        h.join().unwrap();
        let r = m.reply.unwrap();
        for _ in 0..100 {
            if peer_gone(&r) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(peer_gone(&r));
        let printed = ask(&server, Some(allow_output(None, None))).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&printed).unwrap(), allow_output(None, None));
        assert_eq!(ask(&server, None), None, "EOF releases the hook with no output");

        drop(server);
        assert!(!sock.exists());
        assert_eq!(forward(&sock, "Stop", json!({})), None, "nobody listening");
    }

    fn recv(server: &HookServer) -> HookMsg {
        for _ in 0..200 {
            if let Some(m) = server.try_recv() {
                return m;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("no hook message");
    }
}
