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

/// Version of the line protocol between `ccrpc hook` and ccrpc; recorded in the check-in.
pub const HOOK_PROTOCOL: u32 = 1;

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

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let dir = path.parent().context("路径没有上级目录")?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let tmp = dir.join(format!(
        ".{}.{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("f"),
        std::process::id()
    ));
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
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
    serde_json::to_string(out).ok()
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

/// The "always allow" updates of a permission request: Claude Code's suggestions minus mode
/// switches (the dialog's "always allow" option does not change the mode). `None`: not offered.
pub fn always_allow_rules(suggestions: &Value) -> Option<Value> {
    let rules: Vec<Value> = suggestions
        .as_array()?
        .iter()
        .filter(|s| s["type"].as_str() != Some("setMode"))
        .cloned()
        .collect();
    (!rules.is_empty()).then_some(Value::Array(rules))
}

/// Listens on the Session's hook socket for as long as it lives.
pub struct HookServer {
    path: PathBuf,
    rx: Receiver<HookMsg>,
}

impl HookServer {
    /// Binds `<state>/s/<claude pid>.sock`; fails when another ccrpc already listens there.
    pub fn bind(state: &Path, claude_pid: u32, wake: Arc<Wake>) -> Result<HookServer> {
        use std::os::unix::fs::DirBuilderExt;
        let path = socket_path(state, claude_pid);
        let dir = path.parent().context("socket 路径没有上级目录")?;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .with_context(|| format!("建不了 {}", dir.display()))?;
        if path.exists() {
            if UnixStream::connect(&path).is_ok() {
                bail!("已有 ccrpc 连着这个 Session（{}）", path.display());
            }
            let _ = std::fs::remove_file(&path);
        }
        let listener =
            UnixListener::bind(&path).with_context(|| format!("监听不了 {}", path.display()))?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut s) = conn else { continue };
                let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                let Some(input) = read_msg(&mut s) else { continue };
                let event = input["hook_event_name"].as_str().unwrap_or_default().to_string();
                let reply = (event == "PermissionRequest").then(|| {
                    let _ = s.set_read_timeout(None);
                    s
                });
                if tx.send(HookMsg { event, input, reply }).is_err() {
                    break;
                }
                wake.bump();
            }
        });
        Ok(HookServer { path, rx })
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

/// The hook input of one message (`{"v": …, "input": …}` on one line); byte by byte so nothing
/// after the line is consumed.
fn read_msg(s: &mut UnixStream) -> Option<Value> {
    let mut buf = Vec::new();
    let mut b = [0u8; 1];
    loop {
        match s.read(&mut b) {
            Ok(1) if b[0] == b'\n' => break,
            Ok(1) => {
                buf.push(b[0]);
                if buf.len() > 8 << 20 {
                    return None;
                }
            }
            _ => return None,
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
    fn always_allow_drops_mode_switches() {
        let s = json!([
            {"type": "addDirectories", "directories": ["/w"], "destination": "session"},
            {"type": "setMode", "mode": "acceptEdits", "destination": "session"}
        ]);
        assert_eq!(
            always_allow_rules(&s),
            Some(json!([{"type": "addDirectories", "directories": ["/w"], "destination": "session"}]))
        );
        assert_eq!(always_allow_rules(&json!([{"type": "setMode", "mode": "plan"}])), None);
        assert_eq!(always_allow_rules(&Value::Null), None);
    }

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
            match out {
                Some(o) => send_reply(m.reply.unwrap(), Some(o)).unwrap(),
                None => drop(m.reply),
            }
            h.join().unwrap()
        };
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
