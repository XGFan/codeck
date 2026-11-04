//! The environment for ccrpc: an app started from the Dock has none of the login shell's
//! (PATH without `toh` / `claude`, no `CCRPC_ANALYZER_API_KEY`), so it is read once from
//! `$SHELL -l -c 'env -0'`.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Variables that belong to wherever CoDeck itself was started from, not to the Sessions.
const DROP: &[&str] = &["CLAUDECODE", "TOH_HOLDER_ID", "CLAUDE_CODE_ENTRYPOINT"];

pub fn login_env() -> HashMap<String, String> {
    let mut env = read_login_env().unwrap_or_else(|| std::env::vars().collect());
    for k in DROP {
        env.remove(*k);
    }
    // Where toh / claude / ccrpc usually live, in case the shell gave nothing better.
    let home = env.get("HOME").cloned().unwrap_or_default();
    let path = env.get("PATH").cloned().unwrap_or_default();
    let mut dirs: Vec<String> = path
        .split(':')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    for extra in [
        format!("{home}/.local/bin"),
        "/opt/homebrew/bin".into(),
        "/usr/local/bin".into(),
        "/usr/bin".into(),
        "/bin".into(),
    ] {
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

/// The ccrpc to run: `CODECK_CCRPC`, else `~/.local/bin/ccrpc` (where `ccrpc plugin install` is
/// expected to have been run from), else `ccrpc` on PATH.
/// The analyzer's variables set in 「设置 › ccrpc」 override the login shell's (empty ones are left out).
pub fn with_analyzer(
    env: &HashMap<String, String>,
    a: &crate::settings::AnalyzerSettings,
) -> HashMap<String, String> {
    let mut env = env.clone();
    for (k, v) in [
        ("CCRPC_ANALYZER_API_KEY", &a.api_key),
        ("CCRPC_ANALYZER_BASE_URL", &a.base_url),
        ("CCRPC_ANALYZER_MODEL", &a.model),
    ] {
        if let Some(v) = v.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            env.insert(k.into(), v.into());
        }
    }
    env
}

pub fn ccrpc_path(env: &HashMap<String, String>) -> PathBuf {
    if let Some(p) = env.get("CODECK_CCRPC").filter(|p| !p.is_empty()) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::AnalyzerSettings;

    #[test]
    fn analyzer_settings_override_the_login_shell() {
        let shell: HashMap<String, String> = [
            ("PATH", "/bin"),
            ("CCRPC_ANALYZER_API_KEY", "from-shell"),
            ("CCRPC_ANALYZER_MODEL", "shell-model"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let a = AnalyzerSettings {
            api_key: Some(" from-settings ".into()),
            base_url: Some("https://api.anthropic.com".into()),
            model: Some("  ".into()),
        };
        let env = with_analyzer(&shell, &a);
        assert_eq!(env["CCRPC_ANALYZER_API_KEY"], "from-settings");
        assert_eq!(env["CCRPC_ANALYZER_BASE_URL"], "https://api.anthropic.com");
        // Blank in settings: the login shell's stays.
        assert_eq!(env["CCRPC_ANALYZER_MODEL"], "shell-model");
        assert_eq!(env["PATH"], "/bin");
        assert_eq!(with_analyzer(&shell, &AnalyzerSettings::default()), shell);
    }
}
