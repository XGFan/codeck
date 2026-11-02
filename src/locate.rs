//! Locating a Controlled Session: Holder metadata → shell pid → `claude` descendant → Process File
//! → transcript. Everything here only reads.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

/// Directories ccrpc reads from.
#[derive(Debug, Clone)]
pub struct Roots {
    /// ToH state directory (`TOH_STATE_DIR`, default `~/.local/state/toh`).
    pub toh_state: PathBuf,
    /// Claude Code config directory (`CLAUDE_CONFIG_DIR`, default `~/.claude`).
    pub claude_config: PathBuf,
}

impl Roots {
    pub fn from_env() -> Roots {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let env_dir = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Roots {
            toh_state: env_dir("TOH_STATE_DIR").unwrap_or_else(|| home.join(".local/state/toh")),
            claude_config: env_dir("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")),
        }
    }

    pub fn holders_dir(&self) -> PathBuf {
        self.toh_state.join("holders")
    }

    pub fn holder_socket(&self, id: &str) -> PathBuf {
        self.holders_dir().join(format!("{id}.sock"))
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.claude_config.join("sessions")
    }

    pub fn projects_dir(&self) -> PathBuf {
        self.claude_config.join("projects")
    }
}

/// The fields of `<toh>/holders/<id>.json` ccrpc needs.
#[derive(Debug, Clone, Deserialize)]
pub struct HolderMeta {
    pub holder_pid: u32,
    /// The Pane's shell.
    pub child_pid: u32,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub backend: Option<String>,
}

pub fn valid_holder_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub fn read_holder(roots: &Roots, id: &str) -> Result<HolderMeta> {
    if !valid_holder_id(id) {
        bail!("Holder ID 不合法：{id}");
    }
    let p = roots.holders_dir().join(format!("{id}.json"));
    let bytes =
        std::fs::read(&p).with_context(|| format!("读不到 Holder 元数据 {}", p.display()))?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// Every live Holder: `(id, meta)` for each `<id>.json` that has a `<id>.sock` next to it.
pub fn list_holders(roots: &Roots) -> Vec<(String, HolderMeta)> {
    let Ok(rd) = std::fs::read_dir(roots.holders_dir()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(id) = p.file_stem().and_then(|s| s.to_str()).map(str::to_string) else {
            continue;
        };
        if !roots.holder_socket(&id).exists() {
            continue;
        }
        if let Ok(m) = read_holder(roots, &id)
            && crate::procs::pid_exists(m.holder_pid)
        {
            out.push((id, m));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// One row of `ps`.
#[derive(Debug, Clone)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    /// Controlling terminal name without `/dev/` (`ttys012`), `None` for `??`.
    pub tty: Option<String>,
    /// Executable basename.
    pub name: String,
}

/// Snapshot of the process table.
pub fn process_table() -> Result<HashMap<u32, Proc>> {
    let out = Command::new("/bin/ps")
        .args(["-A", "-o", "pid=,ppid=,tty=,comm="])
        .output()
        .context("运行 ps 失败")?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut map = HashMap::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(pid), Some(ppid), Some(tty)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let comm: Vec<&str> = it.collect();
        let (Ok(pid), Ok(ppid)) = (pid.parse(), ppid.parse()) else {
            continue;
        };
        let comm = comm.join(" ");
        let name = comm.rsplit('/').next().unwrap_or(&comm).to_string();
        let tty = (tty != "??").then(|| tty.to_string());
        map.insert(
            pid,
            Proc {
                pid,
                ppid,
                tty,
                name,
            },
        );
    }
    Ok(map)
}

/// The `claude` process among the descendants of `root` (closest to `root` first).
pub fn find_claude_under(table: &HashMap<u32, Proc>, root: u32) -> Option<Proc> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for p in table.values() {
        children.entry(p.ppid).or_default().push(p.pid);
    }
    let mut queue = std::collections::VecDeque::from([root]);
    while let Some(pid) = queue.pop_front() {
        let mut kids = children.get(&pid).cloned().unwrap_or_default();
        kids.sort_unstable();
        for k in kids {
            if let Some(p) = table.get(&k)
                && p.name == "claude"
            {
                return Some(p.clone());
            }
            queue.push_back(k);
        }
    }
    None
}

/// The Holder whose shell is an ancestor of `pid`.
pub fn holder_of_pid<'a>(
    table: &HashMap<u32, Proc>,
    holders: &'a [(String, HolderMeta)],
    pid: u32,
) -> Option<&'a (String, HolderMeta)> {
    let mut cur = pid;
    for _ in 0..64 {
        if let Some(h) = holders.iter().find(|(_, m)| m.child_pid == cur) {
            return Some(h);
        }
        cur = table.get(&cur)?.ppid;
        if cur <= 1 {
            return None;
        }
    }
    None
}

/// `(cols, rows)` of a terminal device, read with `TIOCGWINSZ` on a read-only, non-blocking open
/// (nothing is ever read from or written to the device).
pub fn tty_size(tty: &str) -> Result<(u16, u16)> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    if !tty.bytes().all(|b| b.is_ascii_alphanumeric()) {
        bail!("tty 名不合法：{tty}");
    }
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
        .open(Path::new("/dev").join(tty))
        .with_context(|| format!("打不开 /dev/{tty}"))?;
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    // SAFETY: TIOCGWINSZ fills a winsize; the fd is valid for the duration of the call.
    let rc = unsafe { libc::ioctl(f.as_raw_fd(), libc::TIOCGWINSZ, &mut ws) };
    if rc != 0 {
        return Err(anyhow!(std::io::Error::last_os_error()).context("TIOCGWINSZ"));
    }
    Ok((ws.ws_col, ws.ws_row))
}

/// Transcript path of `session_id`: `<projects>/<encoded cwd>/<id>.jsonl`, falling back to a
/// search over every project directory.
pub fn transcript_path(roots: &Roots, cwd: &str, session_id: &str) -> Option<PathBuf> {
    let file = format!("{session_id}.jsonl");
    let encoded: String = cwd
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let direct = roots.projects_dir().join(&encoded).join(&file);
    if direct.exists() {
        return Some(direct);
    }
    let rd = std::fs::read_dir(roots.projects_dir()).ok()?;
    rd.flatten()
        .map(|e| e.path().join(&file))
        .find(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, ppid: u32, name: &str) -> Proc {
        Proc {
            pid,
            ppid,
            tty: Some("ttys001".into()),
            name: name.into(),
        }
    }

    #[test]
    fn finds_claude_below_shell_and_holder_above_it() {
        let table: HashMap<u32, Proc> = [
            proc(10, 1, "toh"),
            proc(11, 10, "fish"),
            proc(12, 11, "env"),
            proc(13, 12, "claude"),
            proc(14, 13, "zsh"),
        ]
        .into_iter()
        .map(|p| (p.pid, p))
        .collect();
        assert_eq!(find_claude_under(&table, 11).map(|p| p.pid), Some(13));
        assert_eq!(find_claude_under(&table, 14).map(|p| p.pid), None);
        let holders = vec![(
            "abc".to_string(),
            HolderMeta {
                holder_pid: 10,
                child_pid: 11,
                cwd: String::new(),
                backend: None,
            },
        )];
        assert_eq!(
            holder_of_pid(&table, &holders, 13).map(|h| h.0.as_str()),
            Some("abc")
        );
        assert!(holder_of_pid(&table, &holders, 10).is_none());
    }

    #[test]
    fn holder_id_whitelist() {
        assert!(valid_holder_id("0c3181cda759"));
        assert!(!valid_holder_id("../x"));
        assert!(!valid_holder_id(""));
    }
}
