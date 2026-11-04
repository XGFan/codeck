//! The environment for ccrpc: an app started from the Dock has none of the login shell's
//! (PATH without `toh` / `claude`, no `CCRPC_ANALYZER_API_KEY`), so it is read once from
//! `$SHELL -l -c 'env -0'`.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Variables that belong to wherever cc-gui itself was started from, not to the Sessions.
const DROP: &[&str] = &["CLAUDECODE", "TOH_HOLDER_ID", "CLAUDE_CODE_ENTRYPOINT"];

pub fn login_env() -> HashMap<String, String> {
    let mut env = read_login_env().unwrap_or_else(|| std::env::vars().collect());
    for k in DROP {
        env.remove(*k);
    }
    // Where toh / claude / ccrpc usually live, in case the shell gave nothing better.
    let home = env.get("HOME").cloned().unwrap_or_default();
    let path = env.get("PATH").cloned().unwrap_or_default();
    let mut dirs: Vec<String> = path.split(':').filter(|s| !s.is_empty()).map(String::from).collect();
    for extra in [format!("{home}/.local/bin"), "/opt/homebrew/bin".into(), "/usr/local/bin".into(), "/usr/bin".into(), "/bin".into()] {
        if !dirs.contains(&extra) {
            dirs.push(extra);
        }
    }
    env.insert("PATH".into(), dirs.join(":"));
    env
}

fn read_login_env() -> Option<HashMap<String, String>> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let mut child = Command::new(shell)
        .args(["-l", "-c", "/usr/bin/env -0"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    // Bounded as a whole: something the shell config backgrounds may keep stdout open.
    let deadline = Instant::now() + Duration::from_secs(10);
    let buf = rx.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    let _ = child.kill();
    let _ = child.wait();
    let buf = buf.ok()?;
    let env: HashMap<String, String> = buf
        .split(|b| *b == 0)
        .filter_map(|kv| {
            let s = std::str::from_utf8(kv).ok()?;
            let (k, v) = s.split_once('=')?;
            (!k.is_empty()).then(|| (k.to_string(), v.to_string()))
        })
        .collect();
    (env.contains_key("PATH")).then_some(env)
}

/// The ccrpc to run: `CC_GUI_CCRPC`, else `~/.local/bin/ccrpc` (where `ccrpc plugin install` is
/// expected to have been run from), else `ccrpc` on PATH.
pub fn ccrpc_path(env: &HashMap<String, String>) -> PathBuf {
    if let Some(p) = env.get("CC_GUI_CCRPC").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    let home = env.get("HOME").cloned().unwrap_or_default();
    let local = PathBuf::from(&home).join(".local/bin/ccrpc");
    if local.is_file() {
        return local;
    }
    for d in env.get("PATH").map(String::as_str).unwrap_or("").split(':') {
        let p = PathBuf::from(d).join("ccrpc");
        if p.is_file() {
            return p;
        }
    }
    local
}
