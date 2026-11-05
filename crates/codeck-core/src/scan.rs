//! Filesystem discovery under `<root>/projects`: symlink / (dev, ino) dedupe and
//! path → (session, role, agent) mapping. Only `agent-*.jsonl` count as agent transcripts.
//!
//! Layout (per project dir, i.e. each child of `<root>/projects`, symlinks followed): `<sid>.jsonl`
//! is the main file; `<sid>/` holds the agent and sidecar files of [`transcript::layout`].
//!
//! Everything else (`tool-results/`, `memory/`, …) is ignored. Directories and files are visited
//! once per realpath and (dev, ino), so a symlinked project dir or a hard link adds nothing.

use std::collections::{BTreeSet, HashSet};
use std::fs::{self, Metadata};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::model::FileRole;
use crate::paths::projects_dir;
use transcript::layout::{self, AgentFile};

/// A transcript file (main or agent), identified by realpath.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScannedFile {
    /// Canonical path (realpath).
    pub path: PathBuf,
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    pub mtime_ns: i64,
    pub role: FileRole,
    pub session_id: String,
    /// File stem without `agent-`; `None` for main files.
    pub agent_id: Option<String>,
    /// `<runId>` for workflow agents.
    pub workflow_run_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidecarKind {
    /// `agent-<id>.meta.json`.
    AgentMeta {
        agent_id: String,
        role: FileRole,
        workflow_run_id: Option<String>,
    },
    /// `<sid>/workflows/<runId>.json`.
    WorkflowJson { run_id: String },
    /// `<sid>/subagents/workflows/<runId>/journal.jsonl`.
    Journal { run_id: String },
}

/// A non-transcript file that assembly reads (tracked for change detection and revisions).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidecarFile {
    pub path: PathBuf,
    pub session_id: String,
    pub kind: SidecarKind,
    pub size: u64,
    pub mtime_ns: i64,
}

#[derive(Clone, Debug, Default)]
pub struct ScanResult {
    pub files: Vec<ScannedFile>,
    pub sidecars: Vec<SidecarFile>,
}

/// Files of one session dir (`<project dir>/<sid>/`).
#[derive(Clone, Debug, Default)]
pub struct SessionDirListing {
    pub agent_files: Vec<ScannedFile>,
    pub sidecars: Vec<SidecarFile>,
}

/// `st_mtime` in nanoseconds.
pub fn mtime_ns(md: &Metadata) -> i64 {
    md.mtime()
        .saturating_mul(1_000_000_000)
        .saturating_add(md.mtime_nsec())
}

/// Walks `<root>/projects`. Missing or unreadable entries are skipped.
pub fn scan_root(root: &Path) -> ScanResult {
    let mut out = ScanResult::default();
    let mut seen_files: HashSet<(u64, u64)> = HashSet::new();
    for dir in project_dirs(root) {
        scan_project_dir(&dir, &mut seen_files, &mut out);
    }
    out
}

/// [`scan_root`] limited to the sessions `sids`: their main files and session dirs in every
/// project dir (a session's files may sit in more than one).
///
/// The (dev, ino) dedupe sees only these sessions' files: a file hard-linked into another
/// session, which [`scan_root`] keeps only once (for whichever comes first), is kept here.
pub fn scan_sessions(root: &Path, sids: &BTreeSet<String>) -> ScanResult {
    let mut out = ScanResult::default();
    let mut seen_files: HashSet<(u64, u64)> = HashSet::new();
    for dir in project_dirs(root) {
        // The order of `scan_project_dir` (sorted names: `<sid>/` before `<sid>.jsonl`).
        for sid in sids {
            let session_dir = dir.join(sid);
            if fs::metadata(&session_dir).is_ok_and(|m| m.is_dir()) {
                let listing = list_session_dir(&session_dir, sid, &mut seen_files);
                out.files.extend(listing.agent_files);
                out.sidecars.extend(listing.sidecars);
            }
            let main = dir.join(format!("{sid}.jsonl"));
            let Ok(md) = fs::metadata(&main) else {
                continue;
            };
            if md.is_file() && seen_files.insert((md.dev(), md.ino())) {
                let real = fs::canonicalize(&main).unwrap_or(main);
                out.files
                    .push(scanned(real, &md, FileRole::Main, sid, None, None));
            }
        }
    }
    out
}

/// Realpaths of the project dirs under `<root>/projects`, each directory once.
fn project_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = read_dir_sorted(&projects_dir(root)) else {
        return Vec::new();
    };
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    entries
        .into_iter()
        .filter_map(|entry| {
            let real = fs::canonicalize(&entry).ok()?;
            let md = fs::metadata(&real).ok()?;
            (md.is_dir() && seen.insert((md.dev(), md.ino()))).then_some(real)
        })
        .collect()
}

fn scan_project_dir(dir: &Path, seen: &mut HashSet<(u64, u64)>, out: &mut ScanResult) {
    let Ok(entries) = read_dir_sorted(dir) else {
        return;
    };
    for path in entries {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Ok(md) = fs::metadata(&path) else {
            continue;
        };
        if md.is_file() {
            let Some(sid) = name.strip_suffix(".jsonl") else {
                continue;
            };
            if !seen.insert((md.dev(), md.ino())) {
                continue;
            }
            let real = fs::canonicalize(&path).unwrap_or(path.clone());
            out.files
                .push(scanned(real, &md, FileRole::Main, sid, None, None));
        } else if md.is_dir() {
            let listing = list_session_dir(&path, name, seen);
            out.files.extend(listing.agent_files);
            out.sidecars.extend(listing.sidecars);
        }
    }
}

/// Lists the agent transcripts and sidecars of `<project dir>/<session_id>/`, deduplicating
/// files through `seen` (dev, ino).
pub fn list_session_dir(
    session_dir: &Path,
    session_id: &str,
    seen: &mut HashSet<(u64, u64)>,
) -> SessionDirListing {
    let mut out = SessionDirListing::default();
    collect_agents(
        &layout::subagents_dir(session_dir),
        session_id,
        FileRole::Subagent,
        None,
        seen,
        &mut out,
    );
    if let Ok(runs) = read_dir_sorted(&layout::workflow_agents_dir(session_dir)) {
        for run_dir in runs {
            if !run_dir.is_dir() {
                continue;
            }
            let Some(run_id) = run_dir.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let run_id = run_id.to_owned();
            collect_agents(
                &run_dir,
                session_id,
                FileRole::WorkflowSubagent,
                Some(&run_id),
                seen,
                &mut out,
            );
            let journal = run_dir.join(layout::JOURNAL);
            if let Some(sc) = sidecar(&journal, session_id, SidecarKind::Journal { run_id }, seen) {
                out.sidecars.push(sc);
            }
        }
    }
    if let Ok(wfs) = read_dir_sorted(&layout::workflows_dir(session_dir)) {
        for p in wfs {
            let Some(run_id) = p
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(layout::workflow_json_run)
            else {
                continue;
            };
            let kind = SidecarKind::WorkflowJson {
                run_id: run_id.to_owned(),
            };
            if let Some(sc) = sidecar(&p, session_id, kind, seen) {
                out.sidecars.push(sc);
            }
        }
    }
    out
}

fn collect_agents(
    dir: &Path,
    session_id: &str,
    role: FileRole,
    run_id: Option<&str>,
    seen: &mut HashSet<(u64, u64)>,
    out: &mut SessionDirListing,
) {
    let Ok(entries) = read_dir_sorted(dir) else {
        return;
    };
    for p in entries {
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(file) = layout::agent_file(name) else {
            continue;
        };
        if let AgentFile::Meta(agent_id) = file {
            let kind = SidecarKind::AgentMeta {
                agent_id: agent_id.to_owned(),
                role,
                workflow_run_id: run_id.map(str::to_owned),
            };
            if let Some(sc) = sidecar(&p, session_id, kind, seen) {
                out.sidecars.push(sc);
            }
        } else if let AgentFile::Transcript(agent_id) = file {
            let Ok(md) = fs::metadata(&p) else {
                continue;
            };
            if !md.is_file() || !seen.insert((md.dev(), md.ino())) {
                continue;
            }
            let real = fs::canonicalize(&p).unwrap_or(p.clone());
            out.agent_files.push(scanned(
                real,
                &md,
                role,
                session_id,
                Some(agent_id.to_owned()),
                run_id.map(str::to_owned),
            ));
        }
    }
}

fn sidecar(
    path: &Path,
    session_id: &str,
    kind: SidecarKind,
    seen: &mut HashSet<(u64, u64)>,
) -> Option<SidecarFile> {
    let md = fs::metadata(path).ok()?;
    if !md.is_file() || !seen.insert((md.dev(), md.ino())) {
        return None;
    }
    Some(SidecarFile {
        path: fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
        session_id: session_id.to_owned(),
        kind,
        size: md.size(),
        mtime_ns: mtime_ns(&md),
    })
}

fn scanned(
    path: PathBuf,
    md: &Metadata,
    role: FileRole,
    session_id: &str,
    agent_id: Option<String>,
    workflow_run_id: Option<String>,
) -> ScannedFile {
    ScannedFile {
        path,
        dev: md.dev(),
        ino: md.ino(),
        size: md.size(),
        mtime_ns: mtime_ns(md),
        role,
        session_id: session_id.to_owned(),
        agent_id,
        workflow_run_id,
    }
}

fn read_dir_sorted(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    v.sort();
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_layout_and_dedupes_symlinked_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let proj = root.join("projects/-p");
        let sid = "s1";
        fs::create_dir_all(proj.join(sid).join("subagents/workflows/wf1")).unwrap();
        fs::create_dir_all(proj.join(sid).join("workflows")).unwrap();
        fs::create_dir_all(proj.join(sid).join("tool-results")).unwrap();
        fs::write(proj.join("s1.jsonl"), "{}\n").unwrap();
        fs::write(proj.join(sid).join("subagents/agent-a1.jsonl"), "{}\n").unwrap();
        fs::write(proj.join(sid).join("subagents/agent-a1.meta.json"), "{}").unwrap();
        let wf = proj.join(sid).join("subagents/workflows/wf1");
        fs::write(wf.join("agent-w1.jsonl"), "{}\n").unwrap();
        fs::write(wf.join("journal.jsonl"), "{}\n").unwrap();
        fs::write(proj.join(sid).join("workflows/wf1.json"), "{}").unwrap();
        fs::write(proj.join(sid).join("tool-results/x.txt"), "x").unwrap();
        std::os::unix::fs::symlink("-p", root.join("projects/-q")).unwrap();

        let r = scan_root(root);
        let roles: Vec<_> = r
            .files
            .iter()
            .map(|f| (f.role, f.agent_id.clone(), f.workflow_run_id.clone()))
            .collect();
        assert_eq!(
            roles,
            vec![
                (FileRole::Subagent, Some("a1".into()), None),
                (
                    FileRole::WorkflowSubagent,
                    Some("w1".into()),
                    Some("wf1".into())
                ),
                (FileRole::Main, None, None),
            ]
        );
        assert!(r.files.iter().all(|f| f.session_id == "s1"));
        assert_eq!(r.sidecars.len(), 3);
    }
}
