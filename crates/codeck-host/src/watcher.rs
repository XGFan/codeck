//! FSEvents watchers (notify-debouncer-full, 400 ms): one on Claude Code's `<root>/projects` and
//! `<root>/sessions`, including external project symlink targets, for the index; one on the
//! directories of the backends haa Workers run on (agy, codex, pi; ADR-0021), whose changes name the
//! Worker conversations to re-read.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use codeck_core::{agy, codex, paths, pi};
use notify::event::ModifyKind;
use notify::{EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

use crate::events::{EventSink, HostEvent};
use crate::worker::{Job, JobSender};

const DEBOUNCE: Duration = Duration::from_millis(400);
/// How often codex's recent rollouts are checked (see [`follow_workers`]).
const CODEX_POLL: Duration = Duration::from_secs(1);

/// Keeps the debouncer (and its watches) alive; dropping it stops watching.
pub struct Watcher {
    _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
    pub watched: usize,
}

/// Where relevant paths may come from and how to map them back under `<root>/projects`.
#[derive(Debug, Clone)]
pub struct PathMapper {
    /// `<root>/projects`, raw and canonical (FSEvents reports real paths).
    projects: Vec<PathBuf>,
    sessions: Vec<PathBuf>,
    /// (canonical symlink target outside `projects`, symlink path inside raw `projects`).
    links: Vec<(PathBuf, PathBuf)>,
}

impl PathMapper {
    pub fn new(root: &Path, links: Vec<(PathBuf, PathBuf)>) -> Self {
        PathMapper {
            projects: both(paths::projects_dir(root)),
            sessions: both(paths::sessions_dir(root)),
            links,
        }
    }

    /// Maps a changed path to the path the engine should look at: a path under the raw
    /// `<root>/projects` (also for symlinked targets) or a `<root>/sessions/*.json` file.
    /// Returns `None` for everything else.
    pub fn map(&self, path: &Path) -> Option<PathBuf> {
        let raw_projects = &self.projects[0];
        for dir in &self.projects {
            if let Ok(rel) = path.strip_prefix(dir) {
                return is_project_file(rel).then(|| raw_projects.join(rel));
            }
        }
        for (target, link) in &self.links {
            if let Ok(rel) = path.strip_prefix(target) {
                return is_project_file(rel).then(|| link.join(rel));
            }
        }
        for dir in &self.sessions {
            if let Ok(rel) = path.strip_prefix(dir) {
                let is_json = rel.components().count() == 1
                    && rel.extension().is_some_and(|e| e == "json")
                    && !is_hidden(rel);
                return is_json.then(|| self.sessions[0].join(rel));
            }
        }
        None
    }

    /// For a created, removed or renamed path: a project dir or an entry directly in one (such
    /// as a `<sid>/` dir moved in), mapped under the raw `<root>/projects` like [`Self::map`]. Its
    /// files may arrive without events of their own; the engine scans the whole root for such a
    /// path.
    pub fn map_tree(&self, path: &Path) -> Option<PathBuf> {
        let raw_projects = &self.projects[0];
        for dir in &self.projects {
            if let Ok(rel) = path.strip_prefix(dir) {
                return (matches!(depth(rel), Some(1 | 2))).then(|| raw_projects.join(rel));
            }
        }
        for (target, link) in &self.links {
            if let Ok(rel) = path.strip_prefix(target) {
                return (matches!(depth(rel), Some(0 | 1))).then(|| link.join(rel));
            }
        }
        None
    }
}

/// `p`, and its canonical form when that differs (FSEvents reports real paths).
fn both(p: PathBuf) -> Vec<PathBuf> {
    let mut v = vec![p.clone()];
    if let Ok(c) = p.canonicalize()
        && c != p
    {
        v.push(c);
    }
    v
}

/// The data roots of the backends haa Workers run on, whose conversations are read by id (ADR-0021).
#[derive(Debug, Clone)]
pub struct WorkerRoots {
    pub agy: PathBuf,
    pub codex: PathBuf,
    pub pi: PathBuf,
}

/// Maps a changed path under the [`WorkerRoots`] (each raw and canonical) to the Worker conversation
/// it is the file of.
#[derive(Debug, Clone)]
pub struct WorkerMapper {
    agy: Vec<PathBuf>,
    codex: Vec<PathBuf>,
    pi: Vec<PathBuf>,
}

impl WorkerMapper {
    pub fn new(roots: &WorkerRoots) -> Self {
        WorkerMapper {
            agy: both(roots.agy.clone()),
            codex: both(roots.codex.clone()),
            pi: both(roots.pi.clone()),
        }
    }

    /// The id of the Worker conversation `path` is the file of (agy's transcript, a codex rollout, a
    /// pi session file); `None` for everything else.
    pub fn worker_of(&self, path: &Path) -> Option<String> {
        self.agy
            .iter()
            .find_map(|r| agy::session_of_transcript(r, path))
            .or_else(|| {
                self.codex
                    .iter()
                    .find_map(|r| codex::thread_of_path(r, path))
            })
            .or_else(|| self.pi.iter().find_map(|r| pi::session_of_path(r, path)))
    }
}

/// Number of components of `rel`; `None` when one is hidden or not a plain name.
fn depth(rel: &Path) -> Option<usize> {
    rel.components()
        .map(|c| match c {
            Component::Normal(s) if !s.to_string_lossy().starts_with('.') => Some(()),
            _ => None,
        })
        .collect::<Option<Vec<()>>>()
        .map(|v| v.len())
}

fn is_hidden(rel: &Path) -> bool {
    rel.file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with('.'))
}

/// `rel` is relative to `projects/` (or a project symlink target, in which case it is relative to
/// the project dir and one level shallower).
/// Relevant: `*.jsonl` (main, `agent-*`, `journal.jsonl`), `*.meta.json`, `workflows/*.json`,
/// and anything in `tool-results/`.
fn is_project_file(rel: &Path) -> bool {
    if rel.components().count() < 1 || is_hidden(rel) {
        return false;
    }
    let comps: Vec<_> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    if comps.iter().any(|c| c.starts_with('.')) {
        return false;
    }
    let name = comps.last().copied().unwrap_or_default();
    if comps.contains(&"tool-results") && comps.last() != Some(&"tool-results") {
        return true;
    }
    if name.ends_with(".jsonl") || name.ends_with(".meta.json") {
        return true;
    }
    name.ends_with(".json") && comps.len() >= 2 && comps[comps.len() - 2] == "workflows"
}

/// Project dirs under `<root>/projects` that are symlinks whose target lies outside it:
/// `(canonical target, symlink path)`.
pub fn external_links(root: &Path) -> Vec<(PathBuf, PathBuf)> {
    let projects = paths::projects_dir(root);
    let canonical_projects = projects.canonicalize().unwrap_or_else(|_| projects.clone());
    let Ok(rd) = std::fs::read_dir(&projects) else {
        return Vec::new();
    };
    rd.flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_symlink()))
        .filter_map(|e| {
            let target = e.path().canonicalize().ok()?;
            (target.is_dir() && !target.starts_with(&canonical_projects))
                .then(|| (target, e.path()))
        })
        .collect()
}

/// Starts watching; debounced batches become `Job::Fs`. Directories that cannot be watched
/// (e.g. a missing root) are logged and skipped.
pub fn start(root: &Path, jobs: JobSender) -> Option<Watcher> {
    let links = external_links(root);
    let mapper = PathMapper::new(root, links.clone());
    let mut debouncer =
        match new_debouncer(DEBOUNCE, None, move |res: DebounceEventResult| match res {
            Ok(events) => {
                let mapper = &mapper;
                let mut out: Vec<PathBuf> = events
                    .iter()
                    .filter(|e| !matches!(e.kind, EventKind::Access(_)))
                    .flat_map(|e| {
                        let tree = matches!(
                            e.kind,
                            EventKind::Create(_)
                                | EventKind::Remove(_)
                                | EventKind::Modify(ModifyKind::Name(_))
                        );
                        e.paths.iter().filter_map(move |p| {
                            mapper
                                .map(p)
                                .or_else(|| tree.then(|| mapper.map_tree(p)).flatten())
                        })
                    })
                    .collect();
                out.sort();
                out.dedup();
                if !out.is_empty() {
                    jobs.send(Job::Fs(out));
                }
            }
            Err(errors) => {
                for e in errors {
                    tracing::warn!("watcher error: {e}");
                }
                // Events may have been lost: diff the whole root now, not at the next periodic
                // Rescan.
                jobs.send(Job::Rescan);
            }
        }) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("cannot create watcher: {e}");
                return None;
            }
        };
    let mut watched = 0;
    let mut dirs: Vec<(PathBuf, RecursiveMode)> = vec![
        (paths::projects_dir(root), RecursiveMode::Recursive),
        (paths::sessions_dir(root), RecursiveMode::Recursive),
    ];
    dirs.extend(
        links
            .into_iter()
            .map(|(target, _)| (target, RecursiveMode::Recursive)),
    );
    for (dir, mode) in dirs {
        match debouncer.watch(&dir, mode) {
            Ok(()) => watched += 1,
            Err(e) => tracing::warn!("cannot watch {}: {e}", dir.display()),
        }
    }
    Some(Watcher {
        _debouncer: debouncer,
        watched,
    })
}

/// Starts watching the Worker roots' conversation directories (agy's `brain/`, codex's `sessions/`
/// and `archived_sessions/`, pi's `sessions/`; missing ones are skipped): each debounced batch that
/// names Worker conversations becomes a `HostEvent::WorkerFilesChanged`, straight to `sink` (the
/// index is not involved). `None` when there is nothing to watch.
pub fn start_workers(roots: &WorkerRoots, sink: EventSink) -> Option<Watcher> {
    let dirs: Vec<PathBuf> = [
        agy::brain_dir(&roots.agy),
        roots.codex.join("sessions"),
        roots.codex.join("archived_sessions"),
        roots.pi.join("sessions"),
    ]
    .into_iter()
    .filter(|d| d.is_dir())
    .collect();
    if dirs.is_empty() {
        return None;
    }
    let mapper = WorkerMapper::new(roots);
    let mut debouncer =
        match new_debouncer(DEBOUNCE, None, move |res: DebounceEventResult| match res {
            Ok(events) => {
                let mut ids: Vec<String> = events
                    .iter()
                    .filter(|e| !matches!(e.kind, EventKind::Access(_)))
                    .flat_map(|e| e.paths.iter().filter_map(|p| mapper.worker_of(p)))
                    .collect();
                ids.sort();
                ids.dedup();
                if !ids.is_empty() {
                    sink(HostEvent::WorkerFilesChanged { session_ids: ids });
                }
            }
            // A lost event only delays a panel's next read until the file changes again.
            Err(errors) => {
                for e in errors {
                    tracing::warn!("Worker file watcher error: {e}");
                }
            }
        }) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("cannot create the Worker file watcher: {e}");
                return None;
            }
        };
    let mut watched = 0;
    for dir in dirs {
        match debouncer.watch(&dir, RecursiveMode::Recursive) {
            Ok(()) => watched += 1,
            Err(e) => tracing::warn!("cannot watch {}: {e}", dir.display()),
        }
    }
    Some(Watcher {
        _debouncer: debouncer,
        watched,
    })
}

/// Follows the Worker conversations for the Host's lifetime, on a thread of its own: watches the Worker
/// roots ([`start_workers`]), and, since codex keeps a running thread's rollout open and FSEvents
/// reports a write to an open file only once it is closed, checks the rollouts of codex's two newest
/// day directories by size and mtime every second. Returns at once when there is nothing to follow.
pub fn follow_workers(roots: &WorkerRoots, sink: EventSink) {
    let watcher = start_workers(roots, sink.clone());
    if let Some(w) = &watcher {
        tracing::info!("watching {} haa Worker dirs", w.watched);
    }
    if !roots.codex.join("sessions").is_dir() {
        if watcher.is_some() {
            // Dropping it stops the watch.
            loop {
                std::thread::park();
            }
        }
        return;
    }
    let mut seen = None;
    loop {
        std::thread::sleep(CODEX_POLL);
        let ids = codex_changes(&roots.codex, &mut seen);
        if !ids.is_empty() {
            sink(HostEvent::WorkerFilesChanged { session_ids: ids });
        }
    }
}

/// `(size, mtime)` of each recent codex rollout, as [`codex_changes`] last saw them.
type Stamps = HashMap<PathBuf, (u64, SystemTime)>;

/// The codex threads whose rollout (in the two newest day directories) changed or appeared since the
/// previous call; none on the first, which only takes stock.
fn codex_changes(codex_root: &Path, seen: &mut Option<Stamps>) -> Vec<String> {
    let now: Stamps = codex::recent_rollouts(codex_root, 2)
        .into_iter()
        .filter_map(|p| {
            let md = std::fs::metadata(&p).ok()?;
            Some((p, (md.len(), md.modified().ok()?)))
        })
        .collect();
    let mut ids: Vec<String> = match seen.as_ref() {
        Some(before) => now
            .iter()
            .filter(|(p, stamp)| before.get(*p) != Some(*stamp))
            .filter_map(|(p, _)| codex::thread_of_path(codex_root, p))
            .collect(),
        None => Vec::new(),
    };
    *seen = Some(now);
    ids.sort();
    ids.dedup();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapper() -> PathMapper {
        PathMapper {
            projects: vec!["/r/projects".into()],
            sessions: vec!["/r/sessions".into()],
            links: vec![("/ext/proj-a".into(), "/r/projects/-Users-a".into())],
        }
    }

    fn workers() -> WorkerMapper {
        WorkerMapper {
            agy: vec!["/agy".into(), "/private/agy".into()],
            codex: vec!["/codex".into()],
            pi: vec!["/pi".into()],
        }
    }

    #[test]
    fn codex_rollouts_are_checked_by_size_and_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let old = root.join("sessions/2026/11/26");
        let day = root.join("sessions/2026/11/28");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&day).unwrap();
        std::fs::create_dir_all(root.join("sessions/2026/11/27")).unwrap();
        let (a, b) = ("01a2-aaaa", "01a2-bbbb");
        let file = |d: &Path, id: &str| d.join(format!("rollout-2026-11-28T10-00-00-{id}.jsonl"));
        std::fs::write(file(&day, a), "1\n").unwrap();
        std::fs::write(file(&old, b), "1\n").unwrap();
        let mut seen = None;
        assert!(
            codex_changes(root, &mut seen).is_empty(),
            "the first call takes stock"
        );
        assert!(codex_changes(root, &mut seen).is_empty());
        std::fs::write(file(&day, a), "1\n2\n").unwrap();
        // Past the two newest day directories: not checked.
        std::fs::write(file(&old, b), "1\n2\n").unwrap();
        assert_eq!(codex_changes(root, &mut seen), vec![a.to_owned()]);
        std::fs::write(file(&day, b), "1\n").unwrap();
        assert_eq!(codex_changes(root, &mut seen), vec![b.to_owned()]);
        assert!(codex_changes(root, &mut seen).is_empty());
    }

    #[test]
    fn worker_files_name_their_conversation() {
        let w = |p: &str| workers().worker_of(Path::new(p));
        let s = |v: &str| Some(v.to_owned());
        assert_eq!(
            w("/agy/brain/c1/.system_generated/logs/transcript_full.jsonl"),
            s("c1")
        );
        assert_eq!(
            w("/private/agy/brain/c1/.system_generated/logs/transcript_full.jsonl"),
            s("c1")
        );
        let thread = "01a21df5-5b33-7d90-b103-519c3a8e60b2";
        assert_eq!(
            w(&format!(
                "/codex/sessions/2026/11/28/rollout-2026-11-28T01-02-28-{thread}.jsonl"
            )),
            s(thread)
        );
        assert_eq!(
            w(&format!(
                "/codex/archived_sessions/rollout-2026-11-20T09-00-00-{thread}.jsonl.zst"
            )),
            s(thread)
        );
        assert_eq!(
            w("/pi/sessions/--Users-dev-p--/2026-11-24T08-04-52-446Z_haa-c84b9c69.jsonl"),
            s("haa-c84b9c69")
        );
        // Other files of the same backends, and Claude Code's, name no Worker.
        assert_eq!(w("/agy/brain/c1/notes.jsonl"), None);
        assert_eq!(w("/agy/conversations/c1.db"), None);
        assert_eq!(w("/codex/sessions/2026/11/28/notes.jsonl"), None);
        assert_eq!(w("/codex/history.jsonl"), None);
        assert_eq!(w("/pi/sessions/2026-11-24T08-04-52-446Z_x.jsonl"), None);
        assert_eq!(w("/r/projects/-Users-a/s1.jsonl"), None);
    }

    #[test]
    fn writing_a_worker_file_reports_its_conversation() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = WorkerRoots {
            agy: tmp.path().join("agy"),
            codex: tmp.path().join("codex"),
            pi: tmp.path().join("pi"),
        };
        assert!(start_workers(&roots, std::sync::Arc::new(|_| {})).is_none());
        let dir = roots.pi.join("sessions/--Users-dev-p--");
        std::fs::create_dir_all(&dir).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let sink: EventSink = std::sync::Arc::new(move |e| {
            if let HostEvent::WorkerFilesChanged { session_ids } = e {
                let _ = tx.send(session_ids);
            }
        });
        let _watcher = start_workers(&roots, sink).expect("watcher");
        // FSEvents starts reporting a moment after the stream is created.
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(dir.join("2026-11-28T00-00-00-000Z_haa-w1.jsonl"), "{}\n").unwrap();
        let ids = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("WorkerFilesChanged");
        assert_eq!(ids, vec!["haa-w1".to_owned()]);
    }

    fn m(p: &str) -> Option<String> {
        mapper().map(Path::new(p)).map(|p| p.display().to_string())
    }

    #[test]
    fn main_and_agent_transcripts_map() {
        assert_eq!(
            m("/r/projects/-Users-a/s1.jsonl").as_deref(),
            Some("/r/projects/-Users-a/s1.jsonl")
        );
        assert!(m("/r/projects/-Users-a/s1/subagents/agent-ab.jsonl").is_some());
        assert!(m("/r/projects/-Users-a/s1/subagents/workflows/w1/journal.jsonl").is_some());
    }

    #[test]
    fn sidecar_files_map() {
        assert!(m("/r/projects/-Users-a/s1/subagents/agent-ab.meta.json").is_some());
        assert!(m("/r/projects/-Users-a/s1/workflows/run1.json").is_some());
        assert!(m("/r/projects/-Users-a/s1/tool-results/abc.txt").is_some());
    }

    #[test]
    fn irrelevant_files_are_ignored() {
        assert_eq!(m("/r/projects/-Users-a/.DS_Store"), None);
        assert_eq!(m("/r/projects/-Users-a/s1/tool-results"), None);
        assert_eq!(m("/r/projects/-Users-a/notes.txt"), None);
        assert_eq!(m("/r/projects/-Users-a/s1/config.json"), None);
        assert_eq!(m("/r/projects/-Users-a/.hidden/s1.jsonl"), None);
        assert_eq!(m("/elsewhere/s1.jsonl"), None);
    }

    #[test]
    fn project_dirs_and_their_entries_map_as_tree_changes() {
        let t = |p: &str| {
            mapper()
                .map_tree(Path::new(p))
                .map(|p| p.display().to_string())
        };
        assert_eq!(
            t("/r/projects/-Users-b").as_deref(),
            Some("/r/projects/-Users-b")
        );
        assert_eq!(
            t("/r/projects/-Users-b/s1").as_deref(),
            Some("/r/projects/-Users-b/s1")
        );
        assert_eq!(
            t("/ext/proj-a/s1").as_deref(),
            Some("/r/projects/-Users-a/s1")
        );
        assert_eq!(t("/r/projects"), None);
        assert_eq!(t("/r/projects/-Users-b/s1/subagents"), None);
        assert_eq!(t("/r/projects/-Users-b/.tmp"), None);
    }

    #[test]
    fn symlink_target_maps_back_under_projects() {
        assert_eq!(
            m("/ext/proj-a/s9.jsonl").as_deref(),
            Some("/r/projects/-Users-a/s9.jsonl")
        );
        assert_eq!(
            m("/ext/proj-a/s9/subagents/agent-x.meta.json").as_deref(),
            Some("/r/projects/-Users-a/s9/subagents/agent-x.meta.json")
        );
        assert_eq!(m("/ext/proj-a/.DS_Store"), None);
    }

    #[test]
    fn sessions_only_json_files() {
        assert_eq!(
            m("/r/sessions/123.json").as_deref(),
            Some("/r/sessions/123.json")
        );
        assert_eq!(m("/r/sessions/123.abcdef.key"), None);
        assert_eq!(m("/r/sessions/.tmp.json"), None);
    }

    #[test]
    fn external_links_finds_outside_symlinks_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let ext = tmp.path().join("ext");
        std::fs::create_dir_all(root.join("projects/inner")).unwrap();
        std::fs::create_dir_all(ext.join("p")).unwrap();
        std::os::unix::fs::symlink(ext.join("p"), root.join("projects/outer")).unwrap();
        std::os::unix::fs::symlink(root.join("projects/inner"), root.join("projects/alias"))
            .unwrap();
        let links = external_links(&root);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].0, ext.join("p").canonicalize().unwrap());
        assert_eq!(links[0].1, root.join("projects/outer"));
    }
}
