//! Per-session compute: reads and parses a dirty session's files (no DB access).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use transcript::agy;
use transcript::assemble::{self, SessionAggregates};

use super::plan::{Ingest, SessionPlan};
use crate::engine::session_cache::{
    Body, CachedSession, cached_files, load_one, read_chunk, read_head, read_text, sort_main,
};
use crate::index::schema::HEAD_LEN;
use crate::index::writer::{self, AgentRow, FileRows, ProjectRow};
use crate::model::{Agent, FileRole, TitleSource};
use crate::project;
use crate::scan::{FileSource, ScannedFile, SidecarKind};

// -------------------------------------------------------------------- per-session compute

pub(super) struct FileOut {
    pub(super) file: ScannedFile,
    pub(super) rows: FileRows,
    pub(super) head: Vec<u8>,
    pub(super) parsed_offset: u64,
    pub(super) line_count: u64,
    pub(super) failed_lines: u64,
    pub(super) first_error: Option<String>,
}

pub(super) struct SessionSummaryOut {
    pub(super) agg: SessionAggregates,
    pub(super) project: ProjectRow,
    /// Newest main-file mtime (ms), used when no entry has a timestamp.
    pub(super) mtime_ms: f64,
    pub(super) agent: Agent,
    pub(super) haa_worker: bool,
    pub(super) agy_meta: Option<i64>,
}

pub(super) struct SessionWork {
    pub(super) sid: String,
    pub(super) reparse: bool,
    pub(super) recompute: bool,
    pub(super) files: Vec<FileOut>,
    pub(super) stat_only: Vec<(i64, u64, i64)>,
    /// `None` when the session has no readable main file.
    pub(super) summary: Option<SessionSummaryOut>,
    pub(super) agents: Vec<AgentRow>,
    pub(super) subagent_count: u32,
    pub(super) main_set: Option<HashSet<String>>,
    pub(super) cache: Option<CachedSession>,
    pub(super) bytes: u64,
}

/// The repo haa recorded for the worktree an agy Session ran in (its first workspace), if any.
pub(super) fn worktree_repo<'a>(
    meta: &agy::Meta,
    repos: &'a HashMap<PathBuf, PathBuf>,
) -> Option<&'a Path> {
    meta.workspaces
        .first()
        .and_then(|w| repos.get(w))
        .map(PathBuf::as_path)
}

/// `sessions.agy_meta`: the metadata fingerprint, folded (FNV-1a) with the repo haa recorded for its
/// worktree, so a worktree record arriving later re-projects the Session. Unchanged for the rest.
pub(super) fn agy_fingerprint(meta: &agy::Meta, repos: &HashMap<PathBuf, PathBuf>) -> i64 {
    let Some(repo) = worktree_repo(meta, repos) else {
        return meta.fingerprint as i64;
    };
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in meta
        .fingerprint
        .to_le_bytes()
        .iter()
        .chain(repo.as_os_str().as_encoded_bytes())
    {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h as i64
}

/// What an agy Session's summary reads besides its own files: the data root (the Project when it has
/// no workspace), haa's home (Worker detection) and haa's worktree records.
pub(super) struct AgyEnv<'a> {
    pub(super) root: &'a Path,
    pub(super) haa_home: &'a Path,
    pub(super) haa_repos: &'a HashMap<PathBuf, PathBuf>,
}

/// Reads and parses a dirty session's files (no DB access; runs on the rayon pool).
pub(super) fn compute_session(
    plan: SessionPlan,
    cached: Option<CachedSession>,
    keep: bool,
    need_main_set: bool,
    agy_meta: Option<&agy::Meta>,
    agy: &AgyEnv,
) -> SessionWork {
    let &AgyEnv {
        root: agy_root,
        haa_home,
        haa_repos,
    } = agy;
    let mut work = SessionWork {
        bytes: plan.ingest_bytes(),
        sid: plan.sid.clone(),
        reparse: plan.reparse,
        recompute: plan.recompute,
        files: Vec::new(),
        stat_only: plan.stat_only,
        summary: None,
        agents: Vec::new(),
        subagent_count: 0,
        main_set: None,
        cache: None,
    };
    if !plan.recompute {
        // Only stat columns change; hand a cached entry back untouched.
        work.cache = cached;
        return work;
    }
    let ingest: HashMap<&Path, &Ingest> = plan
        .ingest
        .iter()
        .map(|i| (i.file.path.as_path(), i))
        .collect();

    // Main files: always fully parsed (the skeleton needs every record); rows come from the
    // records at or after the ingest offset.
    let mut main: Vec<ScannedFile> = plan
        .current
        .iter()
        .filter(|f| f.role == FileRole::Main)
        .cloned()
        .collect();
    sort_main(&mut main);
    // A reparse means some file was rewritten: never reuse its parse.
    let cached = cached.filter(|_| !plan.reparse);
    let mut old = cached_files(cached.map(CachedSession::into_parts));
    let mut states = Vec::new();
    let mut loaded = Vec::new();
    for f in &main {
        let Ok((st, lf)) = load_one(f, old.remove(&f.path)) else {
            tracing::warn!(path = %f.path.display(), "读取会话文件失败");
            continue;
        };
        if let Some(ing) = ingest.get(f.path.as_path())
            && let Ok(head) = read_head(&f.path, HEAD_LEN)
        {
            let rows = match f.source {
                FileSource::Claude => {
                    writer::extract_rows(lf.records.iter().filter(|r| r.offset >= ing.from))
                }
                FileSource::Agy => {
                    let unknown = st.agy.as_ref().map_or(&[][..], |a| a.unknown.as_slice());
                    writer::extract_agy_rows(
                        lf.records.iter().filter(|r| r.offset >= ing.from),
                        unknown,
                    )
                }
            };
            work.files.push(FileOut {
                file: f.clone(),
                rows,
                head,
                parsed_offset: st.consumed,
                // agy records are deduplicated by step index: count physical lines instead.
                line_count: match f.source {
                    FileSource::Agy => u64::from(st.lines),
                    FileSource::Claude => lf.records.len() as u64 + u64::from(st.failed),
                },
                failed_lines: u64::from(st.failed),
                first_error: st.first_error.clone(),
            });
        }
        states.push(st);
        loaded.push(lf);
    }

    // Agent files: only the new bytes are read.
    let mut agents: BTreeMap<String, AgentRow> = BTreeMap::new();
    for f in plan.current.iter().filter(|f| f.role != FileRole::Main) {
        let Some(aid) = &f.agent_id else { continue };
        agents.insert(
            aid.clone(),
            AgentRow {
                agent_id: aid.clone(),
                workflow_run_id: f.workflow_run_id.clone(),
                ..AgentRow::default()
            },
        );
        let Some(ing) = ingest.get(f.path.as_path()) else {
            continue;
        };
        let (Ok((chunk, _)), Ok(head)) =
            (read_chunk(&f.path, ing.from), read_head(&f.path, HEAD_LEN))
        else {
            tracing::warn!(path = %f.path.display(), "读取 Subagent 文件失败");
            continue;
        };
        let (lines0, failed0, err0) = ing.existing.as_ref().map_or((0, 0, None), |e| {
            (e.line_count, e.failed_lines, e.first_error.clone())
        });
        work.files.push(FileOut {
            file: f.clone(),
            rows: writer::extract_rows(&chunk.records),
            head,
            parsed_offset: chunk.consumed_to,
            line_count: lines0 + chunk.records.len() as u64 + u64::from(chunk.failed),
            failed_lines: failed0 + u64::from(chunk.failed),
            first_error: err0.or(chunk.first_error),
        });
    }
    work.subagent_count = agents.len() as u32;
    for s in &plan.sidecars {
        if let SidecarKind::AgentMeta {
            agent_id,
            workflow_run_id,
            ..
        } = &s.kind
        {
            let row = agents.entry(agent_id.clone()).or_insert_with(|| AgentRow {
                agent_id: agent_id.clone(),
                workflow_run_id: workflow_run_id.clone(),
                ..AgentRow::default()
            });
            *row = std::mem::take(row).with_meta(read_text(&s.path).as_deref());
        }
    }
    work.agents = agents.into_values().collect();

    if !loaded.is_empty() {
        let skeleton = assemble::build_skeleton(&loaded);
        let mut agg = assemble::summarize(&skeleton);
        if need_main_set {
            work.main_set = Some(assemble::tree::main_set(&skeleton));
        }
        let is_agy = main.first().is_some_and(|f| f.source == FileSource::Agy);
        let mtime_ms = main.iter().map(|f| f.mtime_ns).max().unwrap_or(0) as f64 / 1e6;
        let (agent, haa_worker, agy_meta_fp, project) = if is_agy {
            let repo = agy_meta.and_then(|m| worktree_repo(m, haa_repos));
            if let Some(title) = agy_meta.and_then(|m| m.title.as_deref()) {
                agg.title = assemble::prompt::truncate_chars(title, assemble::prompt::TITLE_CHARS);
                agg.title_source = TitleSource::Ai;
            }
            let cwd = agy_meta
                .and_then(|m| m.workspaces.first())
                .map(|p| p.to_string_lossy().into_owned());
            let mut resolver = project::Resolver::default();
            let project = if let Some(repo) = repo {
                // A haa worktree (deleted once merged): the repo haa created it in.
                resolver.start_project(Some(&repo.to_string_lossy()), &main[0].path)
            } else if cwd.is_some() {
                resolver.start_project(cwd.as_deref(), &main[0].path)
            } else {
                // No workspace: fall back to the agy data root itself (not `<root>/brain/.../logs`).
                let root =
                    std::fs::canonicalize(agy_root).unwrap_or_else(|_| agy_root.to_path_buf());
                resolver.start_project(None, &root.join("_"))
            };
            agg.cwd = cwd;
            let uses_haa_tools = states
                .iter()
                .any(|st| st.agy.as_ref().is_some_and(|a| a.uses_haa_tools));
            let haa_worker =
                agy_meta.is_some_and(|m| agy::is_haa_workspace(m, haa_home)) || uses_haa_tools;
            let fp = agy_meta.map(|m| agy_fingerprint(m, haa_repos));
            (Agent::Agy, haa_worker, fp, project)
        } else {
            let cwd = agg.cwd.clone().or_else(|| {
                loaded
                    .iter()
                    .flat_map(|f| &f.records)
                    .find_map(|r| r.entry.cwd.clone())
            });
            let project = project::Resolver::default().start_project(cwd.as_deref(), &main[0].path);
            agg.cwd = cwd;
            (Agent::Claude, false, None, project)
        };
        work.summary = Some(SessionSummaryOut {
            project,
            agg,
            mtime_ms,
            agent,
            haa_worker,
            agy_meta: agy_meta_fp,
        });
    }
    if keep && !loaded.is_empty() {
        work.cache = Some(CachedSession {
            states,
            body: Body::Files(loaded),
        });
    }
    work
}
