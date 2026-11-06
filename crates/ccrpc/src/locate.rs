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

/// Where the conversation of a Pane's claude runs once it moved to the background (the agents
/// page): the Pane's claude stays as the front end and shows the background claude's screen.
#[derive(Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum Moved {
    /// Not moved: the Pane's claude runs it.
    No,
    /// Moved, but no background claude of its job runs with a check-in from this Holder yet (it
    /// is starting, or was stopped and may be opened again).
    Away,
    /// Runs in this background claude, whose ccrpc Plugin checked in from this Holder.
    In {
        pf: crate::procfile::ProcessFile,
        checkin: crate::hook::CheckIn,
    },
}

/// Follows `parked_job` (the `parkedJobId` of the Pane's claude, in Holder `holder`) to the
/// background claude running it. Spare background claude processes check in from the same Holder
/// too: only the job id tells which one is this conversation.
pub fn moved(roots: &Roots, state: &Path, holder: &str, parked_job: Option<&str>) -> Moved {
    let Some(job) = parked_job else {
        return Moved::No;
    };
    let Some(pf) = crate::procfile::job_process(roots, job) else {
        return Moved::Away;
    };
    let Some(checkin) = pf.pid.and_then(|pid| crate::hook::read_checkin(state, pid)) else {
        return Moved::Away;
    };
    let sid = pf.session_id.as_deref();
    if checkin.holder_id != holder
        || sid != Some(checkin.session_id.as_str())
        || crate::hook::checkin_problem(Some(&checkin), sid).is_some()
    {
        return Moved::Away;
    }
    Moved::In { pf, checkin }
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

    /// A Claude config dir and a ccrpc state dir with the Process Files and check-ins seen after
    /// `←` on the agents page (docs/probes.md): the Pane's claude, the background claude running
    /// its job, and a spare. All on this test's own pid (alive; no `procStart` to compare).
    fn moved_dirs(bg_holder: &str) -> (tempfile::TempDir, tempfile::TempDir, Roots) {
        let cc = tempfile::tempdir().unwrap();
        let st = tempfile::tempdir().unwrap();
        let me = std::process::id();
        let write = |dir: &Path, name: &str, v: serde_json::Value| {
            std::fs::create_dir_all(dir.join("sessions")).unwrap();
            std::fs::write(dir.join("sessions").join(name), v.to_string()).unwrap();
        };
        // The background claude (pid = ours) and a spare on a dead pid.
        write(
            cc.path(),
            &format!("{me}.json"),
            serde_json::json!({"pid": me, "sessionId": "new-sid", "kind": "bg", "jobId": "new-sid", "status": "busy"}),
        );
        write(
            cc.path(),
            "999999.json",
            serde_json::json!({"pid": 999_999, "sessionId": "spare-sid", "kind": "bg", "jobId": "spare-si", "spare": true}),
        );
        write(
            st.path(),
            &format!("{me}.json"),
            serde_json::to_value(crate::hook::CheckIn {
                hook_protocol: crate::hook::HOOK_PROTOCOL,
                plugin_version: "0.1.0".into(),
                claude_pid: me,
                holder_id: bg_holder.into(),
                session_id: "new-sid".into(),
                transcript_path: "/c/projects/-w/new-sid.jsonl".into(),
                source: "fork".into(),
                cwd: "/w".into(),
                at: 0,
            })
            .unwrap(),
        );
        let roots = Roots {
            toh_state: None,
            claude_config: cc.path().to_path_buf(),
        };
        (cc, st, roots)
    }

    #[test]
    fn moved_conversation_runs_in_the_job_s_background_claude() {
        let (_cc, st, roots) = moved_dirs("h1");
        assert_eq!(moved(&roots, st.path(), "h1", None), Moved::No);
        let Moved::In { pf, checkin } = moved(&roots, st.path(), "h1", Some("new-sid")) else {
            panic!("not followed");
        };
        assert_eq!(pf.pid, Some(std::process::id()));
        assert_eq!(checkin.session_id, "new-sid");
        assert_eq!(checkin.transcript_path, "/c/projects/-w/new-sid.jsonl");
    }

    #[test]
    fn moved_conversation_not_followed_elsewhere() {
        let (_cc, st, roots) = moved_dirs("h1");
        // No background claude runs the job (starting, or stopped), or only a dead one does.
        assert_eq!(moved(&roots, st.path(), "h1", Some("gone")), Moved::Away);
        assert_eq!(
            moved(&roots, st.path(), "h1", Some("spare-si")),
            Moved::Away
        );
        // Checked in from another Holder.
        assert_eq!(moved(&roots, st.path(), "h2", Some("new-sid")), Moved::Away);
        // No check-in yet.
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            moved(&roots, empty.path(), "h1", Some("new-sid")),
            Moved::Away
        );
    }

    #[test]
    fn job_fields_of_a_process_file() {
        let (_cc, _st, roots) = moved_dirs("h1");
        let (pf, job) = crate::procfile::read_with_job(&roots, std::process::id()).unwrap();
        assert_eq!(pf.session_id.as_deref(), Some("new-sid"));
        assert_eq!(job.kind.as_deref(), Some("bg"));
        assert_eq!(job.job_id.as_deref(), Some("new-sid"));
        assert_eq!(job.parked_job_id, None);
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
