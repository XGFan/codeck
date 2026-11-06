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
    /// Moved, but no background claude runs its job with a check-in for it yet (it is starting,
    /// or was stopped and may be opened again).
    Away,
    /// Runs in this background claude, whose ccrpc Plugin checked in for it.
    In {
        pf: crate::procfile::ProcessFile,
        checkin: crate::hook::CheckIn,
    },
}

/// Follows `parked_job` (the `parkedJobId` of a Pane's claude) to the background claude running
/// it. The daemon is shared by every Pane on the config dir and keeps the environment of the Pane
/// that started it, so the check-in's Holder may be another Pane's (docs/probes.md): the job id on
/// the Pane's own claude is what ties the two. Spares are not it, nor a check-in from a fresh
/// start (a spare's own, before it is claimed): the job runs as a fork or a resume.
pub fn moved(roots: &Roots, state: &Path, parked_job: Option<&str>) -> Moved {
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
    if sid != Some(checkin.session_id.as_str())
        || checkin.source == "startup"
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

/// The transcript of the Session `sid` (`<projects>/<dir>/<sid>.jsonl`): first in the directory
/// Claude Code uses for `cwd`, then in any project directory.
pub fn find_transcript(roots: &Roots, sid: &str, cwd: Option<&str>) -> Option<PathBuf> {
    if sid.is_empty() || !sid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return None;
    }
    let projects = roots.projects_dir();
    let name = format!("{sid}.jsonl");
    let encoded = cwd.map(|c| {
        c.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>()
    });
    let first = encoded.map(|e| projects.join(e).join(&name));
    let rest = std::fs::read_dir(&projects)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join(&name));
    first
        .into_iter()
        .chain(rest)
        .find(|p| p.is_file() && is_transcript_path(roots, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_transcript_by_session_id() {
        let dir = tempfile::tempdir().unwrap();
        let roots = Roots {
            toh_state: None,
            claude_config: dir.path().to_path_buf(),
        };
        let sid = "0f6c2b4e-1111-2222-3333-444455556666";
        let other = dir.path().join("projects/-elsewhere");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join(format!("{sid}.jsonl")), "{}\n").unwrap();
        // Not under the cwd's directory: found in another project directory.
        assert_eq!(
            find_transcript(&roots, sid, Some("/w/app")),
            Some(other.join(format!("{sid}.jsonl")))
        );
        // The cwd's directory comes first.
        let own = dir.path().join("projects/-w-app");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(own.join(format!("{sid}.jsonl")), "{}\n").unwrap();
        assert_eq!(
            find_transcript(&roots, sid, Some("/w/app")),
            Some(own.join(format!("{sid}.jsonl")))
        );
        assert_eq!(find_transcript(&roots, "../x", Some("/w/app")), None);
        assert_eq!(find_transcript(&roots, "missing", None), None);
    }

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
    /// `←` on the agents page (docs/probes.md): the background claude running job `new-sid`
    /// (checked in from another Pane's Holder: the daemon is shared) and a spare. All on this
    /// test's own pid (alive; no `procStart` to compare); Process Files are found by content.
    fn moved_dirs(source: &str) -> (tempfile::TempDir, tempfile::TempDir, Roots) {
        let cc = tempfile::tempdir().unwrap();
        let st = tempfile::tempdir().unwrap();
        let me = std::process::id();
        let write = |dir: &Path, name: &str, v: serde_json::Value| {
            std::fs::create_dir_all(dir.join("sessions")).unwrap();
            std::fs::write(dir.join("sessions").join(name), v.to_string()).unwrap();
        };
        write(
            cc.path(),
            &format!("{me}.json"),
            serde_json::json!({"pid": me, "sessionId": "new-sid", "kind": "bg", "jobId": "new-sid", "status": "busy"}),
        );
        write(
            cc.path(),
            "spare.json",
            serde_json::json!({"pid": me, "sessionId": "spare-sid", "kind": "bg", "jobId": "spare-si", "spare": true}),
        );
        write(
            cc.path(),
            "dead.json",
            serde_json::json!({"pid": 999_999, "sessionId": "dead-sid", "kind": "bg", "jobId": "dead-sid"}),
        );
        write(
            st.path(),
            &format!("{me}.json"),
            serde_json::to_value(crate::hook::CheckIn {
                hook_protocol: crate::hook::HOOK_PROTOCOL,
                plugin_version: "0.1.0".into(),
                claude_pid: me,
                holder_id: "first-pane".into(),
                session_id: "new-sid".into(),
                transcript_path: "/c/projects/-w/new-sid.jsonl".into(),
                source: source.into(),
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
        for source in ["fork", "resume"] {
            let (_cc, st, roots) = moved_dirs(source);
            assert_eq!(moved(&roots, st.path(), None), Moved::No);
            let Moved::In { pf, checkin } = moved(&roots, st.path(), Some("new-sid")) else {
                panic!("not followed ({source})");
            };
            assert_eq!(pf.pid, Some(std::process::id()));
            assert_eq!(checkin.session_id, "new-sid");
            assert_eq!(checkin.transcript_path, "/c/projects/-w/new-sid.jsonl");
        }
    }

    #[test]
    fn moved_conversation_not_followed_elsewhere() {
        let (_cc, st, roots) = moved_dirs("fork");
        // No background claude runs the job (starting, or stopped), only a dead one, or only a
        // spare waiting to be claimed.
        for job in ["gone", "dead-sid", "spare-si"] {
            assert_eq!(moved(&roots, st.path(), Some(job)), Moved::Away, "{job}");
        }
        // No check-in yet.
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(moved(&roots, empty.path(), Some("new-sid")), Moved::Away);
        // A fresh start's check-in (a spare's own, before it is claimed).
        let (_cc, st, roots) = moved_dirs("startup");
        assert_eq!(moved(&roots, st.path(), Some("new-sid")), Moved::Away);
    }

    #[test]
    fn job_fields_of_a_process_file() {
        let (_cc, _st, roots) = moved_dirs("fork");
        let (pf, job) = crate::procfile::read_with_job(&roots, std::process::id()).unwrap();
        assert_eq!(pf.session_id.as_deref(), Some("new-sid"));
        assert!(job.runs(&pf, "new-sid"));
        assert!(!job.runs(&pf, "other"));
        assert_eq!(job.parked_job_id, None);
        let spare = crate::procfile::Job {
            spare: Some(true),
            ..job.clone()
        };
        assert!(!spare.runs(&pf, "new-sid"));
    }

    /// The followed background claude is read by pid only while that pid still runs the job.
    #[test]
    fn followed_background_claude_by_pid() {
        let (cc, _st, roots) = moved_dirs("fork");
        let me = std::process::id();
        let at = |pid, job| crate::procfile::job_process_at(&roots, pid, job).and_then(|p| p.pid);
        assert_eq!(at(me, "new-sid"), Some(me));
        // The pid now runs another job (or is no background claude at all).
        assert_eq!(at(me, "other"), None);
        // Gone: its pid is free (or reused by a process that is not it).
        std::fs::write(
            cc.path().join("sessions/999999.json"),
            serde_json::json!({"pid": 999_999, "sessionId": "new-sid", "kind": "bg", "jobId": "new-sid"})
                .to_string(),
        )
        .unwrap();
        assert_eq!(at(999_999, "new-sid"), None);
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
