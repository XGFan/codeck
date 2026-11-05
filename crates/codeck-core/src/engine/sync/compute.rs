//! Per-session compute: reads and parses a dirty session's files (no DB access).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use transcript::assemble::{self, SessionAggregates};

use super::plan::{Ingest, SessionPlan};
use crate::engine::session_cache::{
    Body, CachedSession, cached_files, load_one, read_chunk, read_head, read_text, sort_main,
};
use crate::index::schema::HEAD_LEN;
use crate::index::writer::{self, AgentRow, FileRows, ProjectRow};
use crate::model::FileRole;
use crate::project;
use crate::scan::{ScannedFile, SidecarKind};

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

/// Reads and parses a dirty session's files (no DB access; runs on the rayon pool).
pub(super) fn compute_session(
    plan: SessionPlan,
    cached: Option<CachedSession>,
    keep: bool,
    need_main_set: bool,
) -> SessionWork {
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
            work.files.push(FileOut {
                file: f.clone(),
                rows: writer::extract_rows(lf.records.iter().filter(|r| r.offset >= ing.from)),
                head,
                parsed_offset: st.consumed,
                line_count: lf.records.len() as u64 + u64::from(st.failed),
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
        let agg = assemble::summarize(&skeleton);
        if need_main_set {
            work.main_set = Some(assemble::tree::main_set(&skeleton));
        }
        let cwd = agg.cwd.clone().or_else(|| {
            loaded
                .iter()
                .flat_map(|f| &f.records)
                .find_map(|r| r.entry.cwd.clone())
        });
        let mtime_ms = main.iter().map(|f| f.mtime_ns).max().unwrap_or(0) as f64 / 1e6;
        work.summary = Some(SessionSummaryOut {
            project: project::Resolver::default().start_project(cwd.as_deref(), &main[0].path),
            agg: SessionAggregates { cwd, ..agg },
            mtime_ms,
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
