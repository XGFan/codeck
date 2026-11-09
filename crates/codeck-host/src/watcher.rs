//! FSEvents watcher (notify-debouncer-full, 400 ms) on `<root>/projects` and `<root>/sessions`,
//! including external symlink targets, mapping paths back to Sessions.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use codeck_core::paths;
use notify::event::ModifyKind;
use notify::{EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

use crate::worker::{Job, JobSender};

const DEBOUNCE: Duration = Duration::from_millis(400);

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
        let both = |p: PathBuf| {
            let mut v = vec![p.clone()];
            if let Ok(c) = p.canonicalize()
                && c != p
            {
                v.push(c);
            }
            v
        };
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
    let dirs = [paths::projects_dir(root), paths::sessions_dir(root)]
        .into_iter()
        .chain(links.into_iter().map(|(target, _)| target));
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
