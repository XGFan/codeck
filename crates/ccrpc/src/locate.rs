//! Locating a Controlled Session: the Holder's shell (from `toh`, see [`crate::toh`]) → `claude`
//! descendant → Process File → transcript. Everything here only reads.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use crate::toh::Pane;

/// Directories ccrpc reads from.
#[derive(Debug, Clone)]
pub struct Roots {
    /// ToH state directory to hand to `toh` (`TOH_STATE_DIR` or `--toh-state-dir`); `None` leaves
    /// it to toh. ccrpc never looks inside it.
    pub toh_state: Option<PathBuf>,
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
            toh_state: env_dir("TOH_STATE_DIR"),
            claude_config: env_dir("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")),
        }
    }

    pub fn default_claude_config() -> PathBuf {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".claude")
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.claude_config.join("sessions")
    }

    pub fn projects_dir(&self) -> PathBuf {
        self.claude_config.join("projects")
    }
}

/// A Holder ID as `toh` prints it (checked before it goes into a command line).
pub fn valid_holder_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// One row of `ps`.
#[derive(Debug, Clone)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    /// Executable basename.
    pub name: String,
}

/// Snapshot of the process table.
pub fn process_table() -> Result<HashMap<u32, Proc>> {
    let out = Command::new("/bin/ps")
        .args(["-A", "-o", "pid=,ppid=,comm="])
        .output()
        .context("运行 ps 失败")?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut map = HashMap::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(pid), Some(ppid)) = (it.next(), it.next()) else {
            continue;
        };
        let comm: Vec<&str> = it.collect();
        let (Ok(pid), Ok(ppid)) = (pid.parse(), ppid.parse()) else {
            continue;
        };
        let comm = comm.join(" ");
        let name = comm.rsplit('/').next().unwrap_or(&comm).to_string();
        map.insert(pid, Proc { pid, ppid, name });
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
    panes: &'a [Pane],
    pid: u32,
) -> Option<&'a Pane> {
    let mut cur = pid;
    for _ in 0..64 {
        if let Some(h) = panes.iter().find(|p| p.shell_pid == cur) {
            return Some(h);
        }
        cur = table.get(&cur)?.ppid;
        if cur <= 1 {
            return None;
        }
    }
    None
}

/// A transcript path reported by the plugin is one of Claude Code's: a `.jsonl` file under the
/// projects directory (nothing else is ever read as a transcript).
pub fn is_transcript_path(roots: &Roots, path: &Path) -> bool {
    let projects = roots.projects_dir();
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let parent_ok = path.parent().is_some_and(|d| {
        let (d, pr) = (canon(d), canon(&projects));
        d.starts_with(&pr) && d != pr
    });
    parent_ok
        && path.extension().is_some_and(|x| x == "jsonl")
        && !path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, ppid: u32, name: &str) -> Proc {
        Proc {
            pid,
            ppid,
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
        let panes = vec![Pane {
            id: "abc".into(),
            backend: Some("clay".into()),
            hidden: Some(false),
            cwd: String::new(),
            shell_pid: 11,
            socket: PathBuf::from("/t/abc.sock"),
        }];
        assert_eq!(
            holder_of_pid(&table, &panes, 13).map(|p| p.id.as_str()),
            Some("abc")
        );
        assert!(holder_of_pid(&table, &panes, 10).is_none());
    }

    #[test]
    fn holder_id_whitelist() {
        assert!(valid_holder_id("0c3181cda759"));
        assert!(!valid_holder_id("../x"));
        assert!(!valid_holder_id(""));
    }

    #[test]
    fn transcripts_only_under_projects() {
        let roots = Roots {
            toh_state: None,
            claude_config: PathBuf::from("/c"),
        };
        assert!(is_transcript_path(
            &roots,
            Path::new("/c/projects/-w/s.jsonl")
        ));
        assert!(
            !is_transcript_path(&roots, Path::new("/c/projects/s.jsonl")),
            "inside a project dir"
        );
        assert!(!is_transcript_path(
            &roots,
            Path::new("/c/projects/-w/s.json")
        ));
        assert!(!is_transcript_path(&roots, Path::new("/etc/x.jsonl")));
        assert!(!is_transcript_path(
            &roots,
            Path::new("/c/projects/-w/../../x.jsonl")
        ));
    }
}
