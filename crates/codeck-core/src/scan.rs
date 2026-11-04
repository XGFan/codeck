//! Filesystem discovery under `<root>/projects`: symlink / (dev, ino) dedupe and
//! path → (session, role, agent) mapping. Only `agent-*.jsonl` count as agent transcripts (A9).
//!
//! Layout (per project dir, i.e. each child of `<root>/projects`, symlinks followed):
//! - `<sid>.jsonl`: main file;
//! - `<sid>/subagents/agent-<id>.jsonl` (+ `.meta.json`): Subagent Run;
//! - `<sid>/subagents/workflows/<runId>/agent-<id>.jsonl` (+ `.meta.json`, `journal.jsonl`):
//!   Workflow Run agents;
//! - `<sid>/workflows/<runId>.json`: Workflow Run summary.
//!
//! Everything else (`tool-results/`, `memory/`, …) is ignored. Directories and files are visited
//! once per realpath and (dev, ino), so a symlinked project dir or a hard link adds nothing.

use std::collections::HashSet;
use std::fs::{self, Metadata};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::model::FileRole;
use crate::paths::projects_dir;

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

/// A non-transcript file that assembly reads (A9: tracked for change detection and revisions).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidecarFile {
    pub path: PathBuf,
    pub session_id: String,
    pub kind: SidecarKind,
    pub size: u64,
    pub mtime_ns: i64,
}

/// A project dir entry that is a symlink (the watcher also watches targets outside `projects/`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymlinkedDir {
    pub link: PathBuf,
    pub target: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct ScanResult {
    pub files: Vec<ScannedFile>,
    pub sidecars: Vec<SidecarFile>,
    /// Canonical project dirs, deduplicated.
    pub project_dirs: Vec<PathBuf>,
    pub symlinked_project_dirs: Vec<SymlinkedDir>,
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
    let Ok(entries) = read_dir_sorted(&projects_dir(root)) else {
        return out;
    };
    let mut seen_dirs: HashSet<(u64, u64)> = HashSet::new();
    let mut seen_files: HashSet<(u64, u64)> = HashSet::new();
    for entry in entries {
        if let Ok(lmd) = fs::symlink_metadata(&entry)
            && lmd.file_type().is_symlink()
            && let Ok(target) = fs::canonicalize(&entry)
            && target.is_dir()
        {
            out.symlinked_project_dirs.push(SymlinkedDir {
                link: entry.clone(),
                target,
            });
        }
        let Ok(real) = fs::canonicalize(&entry) else {
            continue;
        };
        let Ok(md) = fs::metadata(&real) else {
            continue;
        };
        if !md.is_dir() || !seen_dirs.insert((md.dev(), md.ino())) {
            continue;
        }
        scan_project_dir(&real, &mut seen_files, &mut out);
        out.project_dirs.push(real);
    }
    out
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
    let subagents = session_dir.join("subagents");
    collect_agents(
        &subagents,
        session_id,
        FileRole::Subagent,
        None,
        seen,
        &mut out,
    );
    if let Ok(runs) = read_dir_sorted(&subagents.join("workflows")) {
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
            let journal = run_dir.join("journal.jsonl");
            if let Some(sc) = sidecar(&journal, session_id, SidecarKind::Journal { run_id }, seen) {
                out.sidecars.push(sc);
            }
        }
    }
    if let Ok(wfs) = read_dir_sorted(&session_dir.join("workflows")) {
        for p in wfs {
            let Some(run_id) = p
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".json"))
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
        let Some(rest) = name.strip_prefix("agent-") else {
            continue;
        };
        if let Some(agent_id) = rest.strip_suffix(".meta.json") {
            let kind = SidecarKind::AgentMeta {
                agent_id: agent_id.to_owned(),
                role,
                workflow_run_id: run_id.map(str::to_owned),
            };
            if let Some(sc) = sidecar(&p, session_id, kind, seen) {
                out.sidecars.push(sc);
            }
        } else if let Some(agent_id) = rest.strip_suffix(".jsonl") {
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
        assert_eq!(r.project_dirs.len(), 1);
        assert_eq!(r.symlinked_project_dirs.len(), 1);
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
