//! Engine facade: the only API src-tauri calls. Blocking; callers run it off the main thread.
//!
//! Index sync (phase 1, plan §4): stat-walk the data root, diff against `files`, light-parse
//! changed files in parallel (rayon), write rows in one transaction per batch, then recompute
//! every dirty session from its skeleton. Appends read only `[parsed_offset, EOF)`; a shrunk,
//! replaced or rewritten file, or a removed file whose session still has other files, triggers a
//! session-level reparse (A7).
//!
//! Display: an LRU of assembled Sessions keyed by session id and validated by a revision hash over
//! (path, size, mtime) of every source and sidecar file (A9). Main files are kept parsed per
//! (dev, ino) with their consumed offset, so a live append parses only the new bytes (A14a).
//!
//! Source files are only ever opened with `File::open` (read-only, ADR-0002).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use parking_lot::{Mutex, RwLock};
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension};

use crate::assemble::{
    self, AssembledSession, LoadedFile, SessionAggregates, SessionSidecar, SubagentMetaFile,
    WorkflowFiles,
};
use crate::error::{CoreError, CoreResult};
use crate::index::reader::{self, FileRecord};
use crate::index::schema::{HEAD_LEN, SCHEMA_VERSION};
use crate::index::writer::{self, AgentRow, FileRows, FileWrite, ProjectRow, SessionRow};
use crate::index::{Index, text};
use crate::live;
use crate::model::{
    AppInfo, DataRootSource, Diagnostics, FileRole, FindRequest, FindResult, ImageData,
    ImageRequest, IndexPhase, IndexStatus, JumpRequest, JumpTarget, LiveChanged, LiveEntry,
    LiveState, ProjectSummary, SearchRequest, SearchResponse, SessionDetail, SessionQuery,
    SessionSummary, SourceFile, Stats, StatsRequest, TokenTotals, ToolDetail, ToolDetailRequest,
    Transcript, TranscriptRequest, TranscriptScope,
};
use crate::parse::{ParsedChunk, parse_bytes};
use crate::paths::{index_db_path, prices_cache_path, sessions_dir};
use crate::pricing::PriceTable;
use crate::scan::{self, ScanResult, ScannedFile, SessionDirListing, SidecarFile, SidecarKind};
use crate::{diag, project, search, stats};

/// Bytes parsed per write transaction during a sync.
const BATCH_BYTES: u64 = 256 << 20;
/// Assembled Sessions kept for display (plan §5 T2.1).
const SESSION_CACHE_CAP: usize = 4;
/// Parsed Subagent Run files kept for the Subagent scope.
const AGENT_CACHE_CAP: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineConfig {
    /// Read-only Claude Code data root (ADR-0002).
    pub data_root: PathBuf,
    pub data_root_source: DataRootSource,
    /// App-owned cache dir holding `index.sqlite` (ADR-0003).
    pub cache_dir: PathBuf,
}

/// Session ids affected by a scan or a filesystem batch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeSet {
    pub changed: Vec<String>,
    pub removed: Vec<String>,
    /// A session appeared, disappeared or became (non-)empty, or a project row was added/removed.
    pub projects_changed: bool,
}

pub struct Engine {
    cfg: EngineConfig,
    index: Index,
    /// Also read by the index's `cv_cost` SQL function.
    prices: Arc<RwLock<PriceTable>>,
    status: Mutex<IndexStatus>,
    /// Serializes syncs and rebuilds (the single writer).
    sync_lock: Mutex<()>,
    /// Last snapshot returned by `poll_live`.
    live: Mutex<Option<Vec<LiveEntry>>>,
    sessions: Mutex<Lru<String, CachedSession>>,
    agents: Mutex<Lru<(String, String), (FileState, LoadedFile)>>,
    /// Sidecar (size, mtime, session) seen by the last sync; `None` before the first one.
    sidecar_state: Mutex<Option<SidecarState>>,
}

/// Sidecar path → (size, mtime, session id).
type SidecarState = HashMap<PathBuf, (u64, i64, String)>;

impl fmt::Debug for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Engine").field("cfg", &self.cfg).finish()
    }
}

impl Engine {
    /// Creates the cache dir and opens (or recreates) the index. Never writes under the data root.
    pub fn open(cfg: EngineConfig) -> CoreResult<Self> {
        std::fs::create_dir_all(&cfg.cache_dir)?;
        let prices = Arc::new(RwLock::new(
            std::fs::read_to_string(prices_cache_path(&cfg.cache_dir))
                .ok()
                .and_then(|j| PriceTable::from_litellm(&j))
                .unwrap_or_else(PriceTable::embedded),
        ));
        let index = Index::open(
            &index_db_path(&cfg.cache_dir),
            &cfg.data_root,
            prices.clone(),
        )?;
        let (sessions_total, text_ready) =
            index.read(|c| Ok((reader::session_count(c)?, reader::text_ready(c)?)))?;
        Ok(Engine {
            cfg,
            index,
            prices,
            status: Mutex::new(IndexStatus {
                sessions_total,
                text_ready,
                ..IndexStatus::default()
            }),
            sync_lock: Mutex::new(()),
            live: Mutex::new(None),
            sessions: Mutex::new(Lru::new(SESSION_CACHE_CAP)),
            agents: Mutex::new(Lru::new(AGENT_CACHE_CAP)),
            sidecar_state: Mutex::new(None),
        })
    }

    pub fn app_info(&self) -> AppInfo {
        AppInfo {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            data_root: self.cfg.data_root.display().to_string(),
            data_root_source: self.cfg.data_root_source,
            data_root_exists: self.cfg.data_root.is_dir(),
            cache_dir: self.cfg.cache_dir.display().to_string(),
            schema_version: SCHEMA_VERSION,
        }
    }

    pub fn index_status(&self) -> IndexStatus {
        self.status.lock().clone()
    }

    /// Phase 1: stat walk + metadata parse of changed files.
    pub fn scan_all(&self, p: &dyn Fn(&IndexStatus)) -> CoreResult<ChangeSet> {
        self.sync(Some(p), false)
    }

    /// Phase 2: full-text backlog (`index::text`, W3). `on_main_line` is set at insert time from
    /// the session's Main Line set (A2).
    pub fn index_text_backlog(
        &self,
        p: &dyn Fn(&IndexStatus),
        cancel: &AtomicBool,
    ) -> CoreResult<()> {
        let _g = self.sync_lock.lock();
        let mut main_set = |sid: &str| match self.main_set_of(sid) {
            Ok(set) => Some(set),
            Err(e) => {
                tracing::warn!(
                    session = sid,
                    "构建 Main Line 失败，暂不索引该 Session 的全文：{e}"
                );
                None
            }
        };
        // files/bytes done/total describe the text backlog while the phase is IndexingText.
        let progress = |c: text::TextProgress| {
            self.set_status(
                |s| {
                    s.phase = IndexPhase::IndexingText;
                    s.files_done = c.files_done;
                    s.files_total = c.files_total;
                    s.bytes_done = c.bytes_done as f64;
                    s.bytes_total = c.bytes_total as f64;
                },
                p,
            );
        };
        let res = self.index.write(|conn| {
            text::index_pending(conn, &mut main_set, &progress, cancel, text::BATCH_BYTES)
        });
        let ready = self.index.read(reader::text_ready).unwrap_or(false);
        self.set_status(
            |s| {
                s.phase = match &res {
                    // Yielded to another job; the worker resumes the backlog afterwards.
                    Err(CoreError::Cancelled) if !ready => IndexPhase::IndexingText,
                    Err(e) if !matches!(e, CoreError::Cancelled) => IndexPhase::Error,
                    _ => IndexPhase::Idle,
                };
                s.text_ready = ready;
                s.error = match &res {
                    Err(e) if !matches!(e, CoreError::Cancelled) => Some(e.to_string()),
                    _ => None,
                };
            },
            p,
        );
        res
    }

    /// Applies a debounced filesystem batch. Paths only under `<root>/sessions` affect liveness
    /// (see `poll_live`), not the index; anything else re-runs the (cheap) stat diff.
    pub fn apply_changes(&self, paths: &[PathBuf]) -> CoreResult<ChangeSet> {
        let sessions = sessions_dir(&self.cfg.data_root);
        if !paths.is_empty() && paths.iter().all(|p| p.starts_with(&sessions)) {
            return Ok(ChangeSet::default());
        }
        self.sync(None, true)
    }

    /// Returns the new Live Session snapshot when it changed since the last poll.
    pub fn poll_live(&self) -> CoreResult<Option<LiveChanged>> {
        let snapshot: Vec<LiveEntry> = self
            .live_map()?
            .into_iter()
            .map(|(session_id, state)| LiveEntry { session_id, state })
            .collect();
        let mut last = self.live.lock();
        let changed = match last.as_ref() {
            Some(prev) => *prev != snapshot,
            None => !snapshot.is_empty(),
        };
        *last = Some(snapshot.clone());
        Ok(changed.then_some(LiveChanged { live: snapshot }))
    }

    /// When the price list was last fetched online (the cache file's mtime).
    pub fn prices_fetched_at(&self) -> Option<std::time::SystemTime> {
        std::fs::metadata(prices_cache_path(&self.cfg.cache_dir))
            .and_then(|m| m.modified())
            .ok()
    }

    /// Adopts a price list fetched online (LiteLLM's JSON): caches its `claude-*` entries and,
    /// when they differ from the current table, re-prices every message. Returns whether costs
    /// may have changed.
    pub fn update_prices(&self, litellm_json: &str) -> CoreResult<bool> {
        let Some(table) = PriceTable::from_litellm(litellm_json) else {
            return Err(CoreError::Internal("价格表格式无法识别".into()));
        };
        std::fs::write(
            prices_cache_path(&self.cfg.cache_dir),
            table.to_litellm_json(),
        )?;
        if *self.prices.read() == table {
            return Ok(false);
        }
        let _g = self.sync_lock.lock();
        *self.prices.write() = table;
        self.index.write(|conn| {
            let tx = conn.transaction()?;
            writer::reprice_all(&tx)?;
            tx.commit()?;
            Ok(())
        })?;
        Ok(true)
    }

    /// Drops and recreates the index, then runs a full scan. Everything is new to the fresh
    /// index, so the change set always reports `projects_changed`.
    pub fn rebuild(&self, p: &dyn Fn(&IndexStatus)) -> CoreResult<ChangeSet> {
        {
            let _g = self.sync_lock.lock();
            self.index.reset()?;
            *self.sessions.lock() = Lru::new(SESSION_CACHE_CAP);
            *self.agents.lock() = Lru::new(AGENT_CACHE_CAP);
            *self.sidecar_state.lock() = None;
        }
        let mut cs = self.scan_all(p)?;
        cs.projects_changed = true;
        Ok(cs)
    }

    pub fn list_projects(&self) -> CoreResult<Vec<ProjectSummary>> {
        self.index.read(reader::list_projects)
    }

    pub fn list_sessions(&self, q: &SessionQuery) -> CoreResult<Vec<SessionSummary>> {
        let live = self.live_map()?;
        let mut rows = self.index.read(|c| reader::list_sessions(c, q))?;
        for s in &mut rows {
            s.live = live.get(&s.id).cloned();
        }
        Ok(rows)
    }

    pub fn get_session(&self, id: &str) -> CoreResult<SessionDetail> {
        let live = self.live_map()?;
        let (row, files, forks, failed, model_costs) = self.index.read(|c| {
            let Some(row) = reader::session_detail(c, id)? else {
                return Err(CoreError::NotFound("该 Session".into()));
            };
            let files = reader::session_files(c, id)?;
            let forks = reader::fork_children(c, id)?;
            let failed = files.iter().map(|f| f.failed_lines).sum::<u64>();
            let model_costs = reader::model_costs(c, id)?;
            Ok((row, files, forks, failed, model_costs))
        })?;
        let mut summary = row.summary;
        summary.live = live.get(id).cloned();
        let tokens_subagents = TokenTotals {
            input: summary.tokens.input - row.tokens_main.input,
            output: summary.tokens.output - row.tokens_main.output,
            cache_read: summary.tokens.cache_read - row.tokens_main.cache_read,
            cache_creation: summary.tokens.cache_creation - row.tokens_main.cache_creation,
        };
        Ok(SessionDetail {
            summary,
            cwd_missing: row.cwd.as_deref().is_some_and(|c| !Path::new(c).is_dir()),
            cwd: row.cwd,
            project_path: row.project_path,
            project_missing: row.project_missing,
            duration_ms: row.duration_ms,
            models: row.models,
            tokens_main: row.tokens_main,
            tokens_subagents,
            model_costs,
            versions: row.versions,
            files: files
                .iter()
                .map(|f| SourceFile {
                    path: f.path.clone(),
                    size: f.size as f64,
                    role: f.role,
                })
                .collect(),
            forks,
            failed_lines: failed as u32,
            resume_command: format!("claude --resume {}", shell_quote(id)),
        })
    }

    pub fn get_transcript(&self, r: &TranscriptRequest) -> CoreResult<Transcript> {
        self.with_session(&r.session_id, &r.scope, |s, agent| {
            let mut t = assemble::transcript(s, r, agent)?;
            t.revision = s.revision;
            Ok(t)
        })
    }

    pub fn get_tool_detail(&self, r: &ToolDetailRequest) -> CoreResult<ToolDetail> {
        self.with_session(&r.session_id, &r.scope, |s, agent| {
            Ok(assemble::content::tool_detail(s, r, agent)?)
        })
    }

    pub fn get_image(&self, r: &ImageRequest) -> CoreResult<ImageData> {
        self.with_session(&r.session_id, &r.scope, |s, agent| {
            Ok(assemble::content::image(s, r, agent)?)
        })
    }

    pub fn resolve_jump(&self, r: &JumpRequest) -> CoreResult<JumpTarget> {
        self.with_session(&r.session_id, &TranscriptScope::Main, |s, _| {
            Ok(assemble::jump::jump(s, r)?)
        })
    }

    pub fn find_in_session(&self, r: &FindRequest) -> CoreResult<FindResult> {
        self.with_session(&r.session_id, &r.scope, |s, agent| {
            Ok(assemble::find::find(s, r, agent)?)
        })
    }

    pub fn search(&self, r: &SearchRequest) -> CoreResult<SearchResponse> {
        let live = self.live_map()?;
        let mut resp = self.index.read(|c| search::run_fts(c, r))?;
        for g in &mut resp.groups {
            g.session.live = live.get(&g.session.id).cloned();
        }
        Ok(resp)
    }

    pub fn stats(&self, r: &StatsRequest) -> CoreResult<Stats> {
        self.index.read(|c| stats::compute(c, r))
    }

    pub fn diagnostics(&self) -> CoreResult<Diagnostics> {
        self.index.read(diag::compute)
    }

    /// Main file (or the agent file when `agent_id` is set) of a Session, for "在 Finder 中显示".
    pub fn session_file_path(
        &self,
        session_id: &str,
        agent_id: Option<&str>,
    ) -> CoreResult<PathBuf> {
        let files = self.index.read(|c| reader::session_files(c, session_id))?;
        let found = match agent_id {
            None => files.iter().find(|f| f.role == FileRole::Main),
            Some(a) => files.iter().find(|f| f.agent_id.as_deref() == Some(a)),
        };
        found
            .map(|f| PathBuf::from(&f.path))
            .ok_or_else(|| CoreError::NotFound("该 Session 的文件".into()))
    }

    /// Slash commands for completion while connected to a Session started in `cwd`.
    pub fn slash_commands(&self, cwd: Option<&str>) -> Vec<crate::model::SlashCommand> {
        crate::slash::slash_commands(&self.cfg.data_root, cwd.map(Path::new))
    }

    // ---------------------------------------------------------------- live

    fn live_map(&self) -> CoreResult<BTreeMap<String, LiveState>> {
        let pfs = live::read_process_files(&sessions_dir(&self.cfg.data_root));
        Ok(live::detect(&pfs))
    }

    /// Whether a Claude Code process is running `session_id` now (read afresh, not the last poll).
    pub fn is_live(&self, session_id: &str) -> bool {
        let pfs = live::read_process_files(&sessions_dir(&self.cfg.data_root));
        live::detect(&pfs).contains_key(session_id)
    }

    // ---------------------------------------------------------------- status

    fn set_status(&self, f: impl FnOnce(&mut IndexStatus), p: &dyn Fn(&IndexStatus)) {
        let snapshot = {
            let mut s = self.status.lock();
            f(&mut s);
            s.clone()
        };
        p(&snapshot);
    }

    // ---------------------------------------------------------------- sync (phase 1)

    fn sync(&self, p: Option<&dyn Fn(&IndexStatus)>, keep_cache: bool) -> CoreResult<ChangeSet> {
        let _g = self.sync_lock.lock();
        let p: &dyn Fn(&IndexStatus) = p.unwrap_or(&|_| {});
        let scan = scan::scan_root(&self.cfg.data_root);
        let db_files = self.index.read(reader::all_files)?;
        let (sidecar_dirty, sidecar_now) = self.diff_sidecars(&scan, &db_files);
        let (plans, removed) = plan_sync(&scan, db_files, &sidecar_dirty);
        let with_text = self.index.read(reader::sessions_with_text)?;

        let files_total: u32 = plans.iter().map(|p| p.ingest.len() as u32).sum();
        let bytes_total: u64 = plans.iter().map(SessionPlan::ingest_bytes).sum();
        self.set_status(
            |s| {
                s.phase = IndexPhase::Scanning;
                s.files_total = files_total;
                s.files_done = 0;
                s.bytes_total = bytes_total as f64;
                s.bytes_done = 0.0;
                s.error = None;
            },
            p,
        );

        let mut cs = ChangeSet::default();
        // Sessions sharing a message with a removed or rewritten one (billing may move to them).
        let mut neighbors: HashSet<String> = HashSet::new();
        let mut resolver = project::Resolver::for_data_root(&self.cfg.data_root);
        let result = (|| -> CoreResult<()> {
            self.index.write(|conn| {
                let tx = conn.transaction()?;
                for sid in &removed {
                    self.sessions.lock().take(sid);
                    if session_exists(&tx, sid)?.is_some() {
                        cs.removed.push(sid.clone());
                        cs.projects_changed = true;
                    }
                    neighbors.extend(writer::copy_neighbors(&tx, sid)?);
                    writer::delete_session(&tx, sid)?;
                }
                tx.commit()?;
                Ok(())
            })?;
            for batch in batches(plans) {
                let jobs: Vec<(SessionPlan, Option<CachedSession>)> = {
                    let mut cache = self.sessions.lock();
                    batch
                        .into_iter()
                        .map(|plan| {
                            let cached = cache.take(&plan.sid);
                            (plan, cached)
                        })
                        .collect()
                };
                let works: Vec<SessionWork> = jobs
                    .into_par_iter()
                    .map(|(plan, cached)| {
                        let keep = keep_cache || cached.is_some();
                        let need_main_set = !plan.reparse && with_text.contains(&plan.sid);
                        compute_session(plan, cached, keep, need_main_set)
                    })
                    .collect();
                let (files_done, bytes_done) = works.iter().fold((0u32, 0u64), |(f, b), w| {
                    (f + w.files.len() as u32, b + w.bytes)
                });
                self.index.write(|conn| {
                    let tx = conn.transaction()?;
                    for w in &works {
                        write_work(&tx, w, &mut cs, &mut neighbors, &mut resolver)?;
                    }
                    tx.commit()?;
                    Ok(())
                })?;
                {
                    let mut cache = self.sessions.lock();
                    for w in works {
                        if let Some(c) = w.cache {
                            cache.put(w.sid, c);
                        }
                    }
                }
                self.set_status(
                    |s| {
                        s.files_done += files_done;
                        s.bytes_done += bytes_done as f64;
                    },
                    p,
                );
            }
            self.index.write(|conn| {
                let tx = conn.transaction()?;
                rebill_changed(&tx, &mut cs, neighbors)?;
                if writer::prune_projects(&tx)? > 0 {
                    cs.projects_changed = true;
                }
                tx.commit()?;
                Ok(())
            })?;
            Ok(())
        })();

        let (total, ready) = self
            .index
            .read(|c| Ok((reader::session_count(c)?, reader::text_ready(c)?)))
            .unwrap_or((0, false));
        self.set_status(
            |s| {
                s.phase = if result.is_ok() {
                    IndexPhase::Idle
                } else {
                    IndexPhase::Error
                };
                s.sessions_total = total;
                s.text_ready = ready;
                s.error = result.as_ref().err().map(ToString::to_string);
            },
            p,
        );
        result?;
        *self.sidecar_state.lock() = Some(sidecar_now);
        cs.changed.sort();
        cs.changed.dedup();
        cs.removed.sort();
        Ok(cs)
    }

    /// Sessions whose sidecar files changed since the previous sync (A9), and the sidecar state
    /// to commit once this sync succeeds. Before the first sync, a sidecar newer than every
    /// indexed file of its session counts as changed (it was written while the app was closed).
    fn diff_sidecars(
        &self,
        scan: &ScanResult,
        db: &[FileRecord],
    ) -> (HashSet<String>, SidecarState) {
        let now: SidecarState = scan
            .sidecars
            .iter()
            .map(|s| (s.path.clone(), (s.size, s.mtime_ns, s.session_id.clone())))
            .collect();
        let mut dirty = HashSet::new();
        if let Some(prev) = self.sidecar_state.lock().as_ref() {
            for (path, v) in &now {
                if prev.get(path) != Some(v) {
                    dirty.insert(v.2.clone());
                }
            }
            for (path, v) in prev {
                if !now.contains_key(path) {
                    dirty.insert(v.2.clone());
                }
            }
        } else {
            let mut newest: HashMap<&str, i64> = HashMap::new();
            for r in db {
                let m = newest.entry(&r.session_id).or_insert(r.mtime_ns);
                *m = (*m).max(r.mtime_ns);
            }
            for (_, mtime, sid) in now.values() {
                if newest.get(sid.as_str()).is_some_and(|m| mtime > m) {
                    dirty.insert(sid.clone());
                }
            }
        }
        (dirty, now)
    }

    /// A2: the Main Line set of a session, from its main files.
    /// Reuses (and extends) a cached parse of the session, so live appends stay cheap.
    fn main_set_of(&self, sid: &str) -> CoreResult<HashSet<String>> {
        let ctx = self.session_ctx(sid)?;
        let cached = self.sessions.lock().take(&sid.to_owned());
        let Some(c) = cached else {
            let (_, files) = load_main(&ctx.main, None)?;
            return Ok(assemble::tree::main_set(&assemble::build_skeleton(&files)));
        };
        if let Body::Assembled(a) = &c.body
            && a.revision == ctx.revision
        {
            let set = assemble::tree::main_set(&a.skeleton);
            self.sessions.lock().put(sid.to_owned(), c);
            return Ok(set);
        }
        let (states, files) = load_main(&ctx.main, Some(c.into_parts()))?;
        let set = assemble::tree::main_set(&assemble::build_skeleton(&files));
        self.sessions.lock().put(
            sid.to_owned(),
            CachedSession {
                states,
                body: Body::Files(files),
            },
        );
        Ok(set)
    }

    // ---------------------------------------------------------------- display

    /// Current on-disk view of a session: main files (stat now), sidecar listing and revision.
    fn session_ctx(&self, sid: &str) -> CoreResult<SessionCtx> {
        let rows = self.index.read(|c| reader::session_files(c, sid))?;
        let mut main: Vec<ScannedFile> = rows
            .iter()
            .filter(|f| f.role == FileRole::Main)
            .filter_map(|f| {
                let md = std::fs::metadata(&f.path).ok()?;
                Some(ScannedFile {
                    path: PathBuf::from(&f.path),
                    dev: md.dev(),
                    ino: md.ino(),
                    size: md.size(),
                    mtime_ns: scan::mtime_ns(&md),
                    role: FileRole::Main,
                    session_id: sid.to_owned(),
                    agent_id: None,
                    workflow_run_id: None,
                })
            })
            .collect();
        if main.is_empty() {
            return Err(CoreError::NotFound("该 Session".into()));
        }
        sort_main(&mut main);
        let mut dirs: Vec<PathBuf> = Vec::new();
        for f in &main {
            if let Some(d) = f.path.parent().map(|p| p.join(sid))
                && !dirs.contains(&d)
            {
                dirs.push(d);
            }
        }
        let session_dir = dirs.iter().find(|d| d.is_dir()).unwrap_or(&dirs[0]).clone();
        let mut seen = HashSet::new();
        let mut listing = SessionDirListing::default();
        for d in &dirs {
            let l = scan::list_session_dir(d, sid, &mut seen);
            listing.agent_files.extend(l.agent_files);
            listing.sidecars.extend(l.sidecars);
        }
        let mut h = Fnv::new();
        for f in &main {
            h.file(&f.path, f.size, f.mtime_ns);
        }
        for f in &listing.agent_files {
            h.file(&f.path, f.size, f.mtime_ns);
        }
        for s in &listing.sidecars {
            h.file(&s.path, s.size, s.mtime_ns);
        }
        Ok(SessionCtx {
            main,
            session_dir,
            listing,
            revision: h.revision(),
        })
    }

    fn build_sidecar(&self, sid: &str, listing: &SessionDirListing) -> CoreResult<SessionSidecar> {
        let mut metas: BTreeMap<String, SubagentMetaFile> = BTreeMap::new();
        let mut workflows: BTreeMap<String, WorkflowFiles> = BTreeMap::new();
        for f in &listing.agent_files {
            let Some(aid) = f.agent_id.clone() else {
                continue;
            };
            let m = metas
                .entry(aid.clone())
                .or_insert_with(|| SubagentMetaFile {
                    agent_id: aid,
                    role: f.role,
                    workflow_run_id: f.workflow_run_id.clone(),
                    transcript_path: None,
                    meta_json: None,
                });
            m.transcript_path = Some(f.path.clone());
            if let Some(run) = &f.workflow_run_id {
                workflow_entry(&mut workflows, run);
            }
        }
        for s in &listing.sidecars {
            match &s.kind {
                SidecarKind::AgentMeta {
                    agent_id,
                    role,
                    workflow_run_id,
                } => {
                    let m = metas
                        .entry(agent_id.clone())
                        .or_insert_with(|| SubagentMetaFile {
                            agent_id: agent_id.clone(),
                            role: *role,
                            workflow_run_id: workflow_run_id.clone(),
                            transcript_path: None,
                            meta_json: None,
                        });
                    m.meta_json = read_text(&s.path);
                }
                SidecarKind::WorkflowJson { run_id } => {
                    workflow_entry(&mut workflows, run_id).workflow_json = read_text(&s.path);
                }
                SidecarKind::Journal { run_id } => {
                    workflow_entry(&mut workflows, run_id).journal_jsonl = read_text(&s.path);
                }
            }
        }
        let (agent_stats, fork_origin) = self
            .index
            .read(|c| Ok((reader::agent_stats(c, sid)?, reader::fork_origin(c, sid)?)))?;
        Ok(SessionSidecar {
            subagent_metas: metas.into_values().collect(),
            workflows: workflows.into_values().collect(),
            agent_stats,
            fork_origin,
        })
    }

    /// Runs `f` on the assembled session (cached by revision) and, for the Subagent scope, the
    /// Subagent Run's parsed file.
    fn with_session<T>(
        &self,
        sid: &str,
        scope: &TranscriptScope,
        f: impl FnOnce(&AssembledSession, Option<&LoadedFile>) -> CoreResult<T>,
    ) -> CoreResult<T> {
        let ctx = self.session_ctx(sid)?;
        let cached = self.sessions.lock().take(&sid.to_owned());
        let entry = match cached {
            Some(c) if c.assembled_revision() == Some(ctx.revision) => c,
            other => {
                let (states, files) = load_main(&ctx.main, other.map(CachedSession::into_parts))?;
                let sidecar = self.build_sidecar(sid, &ctx.listing)?;
                let mut a = assemble::assemble(sid, &ctx.session_dir, files, sidecar);
                a.revision = ctx.revision;
                CachedSession {
                    states,
                    body: Body::Assembled(Box::new(a)),
                }
            }
        };
        let agent = match scope {
            TranscriptScope::Main => None,
            TranscriptScope::Subagent { agent_id } => Some(self.load_agent(sid, agent_id, &ctx)?),
        };
        let out = match &entry.body {
            Body::Assembled(a) => f(a, agent.as_ref().map(|(_, lf)| lf)),
            Body::Files(_) => Err(CoreError::Internal("Session 缓存状态异常".into())),
        };
        if let (Some(a), TranscriptScope::Subagent { agent_id }) = (agent, scope) {
            self.agents
                .lock()
                .put((sid.to_owned(), agent_id.clone()), a);
        }
        self.sessions.lock().put(sid.to_owned(), entry);
        out
    }

    fn load_agent(
        &self,
        sid: &str,
        agent_id: &str,
        ctx: &SessionCtx,
    ) -> CoreResult<(FileState, LoadedFile)> {
        let Some(file) = ctx
            .listing
            .agent_files
            .iter()
            .find(|f| f.agent_id.as_deref() == Some(agent_id))
        else {
            return Err(CoreError::NotFound("该 Subagent".into()));
        };
        let key = (sid.to_owned(), agent_id.to_owned());
        let cached = self.agents.lock().take(&key);
        Ok(load_one(file, cached)?)
    }
}

// -------------------------------------------------------------------- caches

/// A tiny most-recently-used-first cache.
struct Lru<K, V> {
    cap: usize,
    items: Vec<(K, V)>,
}

impl<K: PartialEq, V> Lru<K, V> {
    fn new(cap: usize) -> Self {
        Lru {
            cap,
            items: Vec::new(),
        }
    }

    fn take(&mut self, k: &K) -> Option<V> {
        let i = self.items.iter().position(|(key, _)| key == k)?;
        Some(self.items.remove(i).1)
    }

    fn put(&mut self, k: K, v: V) {
        self.items.retain(|(key, _)| *key != k);
        self.items.insert(0, (k, v));
        self.items.truncate(self.cap);
    }
}

/// Parse state of one cached file (A14a).
#[derive(Clone, Debug)]
struct FileState {
    path: PathBuf,
    dev: u64,
    ino: u64,
    /// Absolute offset just past the last consumed `\n`.
    consumed: u64,
    /// `\n` count in `[0, consumed)` (absolute line numbers of appended records).
    lines: u32,
    failed: u32,
    first_error: Option<String>,
    /// First ≤ [`HEAD_LEN`] bytes, to detect an in-place rewrite.
    head: Vec<u8>,
}

impl FileState {
    /// Same file (dev, ino, head bytes) that has not shrunk below what was parsed.
    fn matches(&self, f: &ScannedFile) -> bool {
        self.dev == f.dev
            && self.ino == f.ino
            && f.size >= self.consumed
            && read_head(&f.path, self.head.len()).is_ok_and(|h| h == self.head)
    }
}

enum Body {
    Files(Vec<LoadedFile>),
    Assembled(Box<AssembledSession>),
}

/// Parsed main files of a session, optionally assembled.
struct CachedSession {
    /// Parallel to the main files.
    states: Vec<FileState>,
    body: Body,
}

impl CachedSession {
    fn assembled_revision(&self) -> Option<f64> {
        match &self.body {
            Body::Assembled(a) => Some(a.revision),
            Body::Files(_) => None,
        }
    }

    fn into_parts(self) -> (Vec<FileState>, Vec<LoadedFile>) {
        let files = match self.body {
            Body::Files(f) => f,
            Body::Assembled(mut a) => std::mem::take(&mut a.main_files),
        };
        (self.states, files)
    }
}

struct SessionCtx {
    /// Largest first.
    main: Vec<ScannedFile>,
    session_dir: PathBuf,
    listing: SessionDirListing,
    revision: f64,
}

/// FNV-1a over (path, size, mtime) of a session's files, as an exact f64 (53 bits).
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, b: &[u8]) {
        for x in b {
            self.0 ^= u64::from(*x);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn file(&mut self, path: &Path, size: u64, mtime_ns: i64) {
        self.bytes(path.as_os_str().as_encoded_bytes());
        self.bytes(&size.to_le_bytes());
        self.bytes(&mtime_ns.to_le_bytes());
    }

    fn revision(&self) -> f64 {
        (self.0 & ((1u64 << 53) - 1)) as f64
    }
}

// -------------------------------------------------------------------- file reading

/// Reads `[from, EOF)` and parses its complete lines; also returns the `\n` count consumed.
fn read_chunk(path: &Path, from: u64) -> io::Result<(ParsedChunk, u32)> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if len <= from {
        return Ok((
            ParsedChunk {
                consumed_to: from,
                ..ParsedChunk::default()
            },
            0,
        ));
    }
    file.seek(SeekFrom::Start(from))?;
    let mut buf = Vec::with_capacity((len - from) as usize);
    file.read_to_end(&mut buf)?;
    let chunk = parse_bytes(&buf, from);
    let consumed = (chunk.consumed_to - from) as usize;
    let newlines = memchr::memchr_iter(b'\n', &buf[..consumed]).count() as u32;
    Ok((chunk, newlines))
}

/// First ≤ [`HEAD_LEN`] bytes.
fn read_head(path: &Path, len: usize) -> io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(len);
    File::open(path)?.take(len as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

fn read_text(path: &Path) -> Option<String> {
    let mut s = String::new();
    File::open(path).ok()?.read_to_string(&mut s).ok()?;
    Some(s)
}

fn load_fresh(f: &ScannedFile) -> io::Result<(FileState, LoadedFile)> {
    let head = read_head(&f.path, HEAD_LEN)?;
    let (chunk, lines) = read_chunk(&f.path, 0)?;
    Ok((
        FileState {
            path: f.path.clone(),
            dev: f.dev,
            ino: f.ino,
            consumed: chunk.consumed_to,
            lines,
            failed: chunk.failed,
            first_error: chunk.first_error,
            head,
        },
        LoadedFile {
            path: f.path.clone(),
            role: f.role,
            agent_id: f.agent_id.clone(),
            records: chunk.records,
        },
    ))
}

/// Appends records written since `st.consumed`.
fn extend_loaded(st: &mut FileState, lf: &mut LoadedFile) -> io::Result<()> {
    let (chunk, lines) = read_chunk(&st.path, st.consumed)?;
    let base = st.lines;
    lf.records.extend(chunk.records.into_iter().map(|mut r| {
        r.line_no += base;
        r
    }));
    st.lines += lines;
    st.failed += chunk.failed;
    if st.first_error.is_none() {
        st.first_error = chunk.first_error;
    }
    st.consumed = chunk.consumed_to;
    if st.head.len() < HEAD_LEN {
        st.head = read_head(&st.path, HEAD_LEN)?;
    }
    Ok(())
}

type CachedFiles = HashMap<PathBuf, (FileState, LoadedFile)>;

fn cached_files(cached: Option<(Vec<FileState>, Vec<LoadedFile>)>) -> CachedFiles {
    cached
        .map(|(s, f)| {
            s.into_iter()
                .zip(f)
                .map(|(s, f)| (s.path.clone(), (s, f)))
                .collect()
        })
        .unwrap_or_default()
}

/// Parses `f`, extending its cached parse when the file only grew (A14a).
fn load_one(
    f: &ScannedFile,
    cached: Option<(FileState, LoadedFile)>,
) -> io::Result<(FileState, LoadedFile)> {
    match cached {
        Some((mut st, mut lf)) if st.matches(f) => {
            extend_loaded(&mut st, &mut lf)?;
            Ok((st, lf))
        }
        _ => load_fresh(f),
    }
}

/// Parsed main files (in `main` order), reusing and extending cached ones.
fn load_main(
    main: &[ScannedFile],
    cached: Option<(Vec<FileState>, Vec<LoadedFile>)>,
) -> io::Result<(Vec<FileState>, Vec<LoadedFile>)> {
    let mut old = cached_files(cached);
    let mut states = Vec::with_capacity(main.len());
    let mut files = Vec::with_capacity(main.len());
    for f in main {
        let (st, lf) = load_one(f, old.remove(&f.path))?;
        states.push(st);
        files.push(lf);
    }
    Ok((states, files))
}

fn sort_main(main: &mut [ScannedFile]) {
    main.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
}

fn workflow_entry<'a>(
    map: &'a mut BTreeMap<String, WorkflowFiles>,
    run_id: &str,
) -> &'a mut WorkflowFiles {
    map.entry(run_id.to_owned())
        .or_insert_with(|| WorkflowFiles {
            run_id: run_id.to_owned(),
            workflow_json: None,
            journal_jsonl: None,
        })
}

// -------------------------------------------------------------------- sync planning

/// A file whose rows must be (re)read from `from`.
struct Ingest {
    file: ScannedFile,
    from: u64,
    /// The current row (append); `None` for a new file or a reparse.
    existing: Option<FileRecord>,
}

/// Work for one dirty session.
struct SessionPlan {
    sid: String,
    /// Delete every row of the session first and re-ingest all its files from 0.
    reparse: bool,
    ingest: Vec<Ingest>,
    /// (file id, size, mtime) for files whose stat changed without new complete lines.
    stat_only: Vec<(i64, u64, i64)>,
    /// Recompute the session row (false when only stat columns change).
    recompute: bool,
    /// Every current file of the session.
    current: Vec<ScannedFile>,
    sidecars: Vec<SidecarFile>,
}

impl SessionPlan {
    fn ingest_bytes(&self) -> u64 {
        self.ingest
            .iter()
            .map(|i| i.file.size.saturating_sub(i.from))
            .sum()
    }

    /// Rough parse cost: ingested bytes plus main files that may need a full load.
    fn cost(&self) -> u64 {
        let main: u64 = self
            .current
            .iter()
            .filter(|f| f.role == FileRole::Main)
            .map(|f| f.size)
            .sum();
        self.ingest_bytes() + if self.recompute { main } else { 0 }
    }
}

enum FileAction {
    Unchanged,
    StatOnly,
    Append,
    Reparse,
}

fn classify(f: &ScannedFile, row: &FileRecord) -> FileAction {
    if f.dev != row.dev || f.ino != row.ino || f.size < row.parsed_offset {
        return FileAction::Reparse;
    }
    if f.size == row.size && f.mtime_ns == row.mtime_ns {
        return FileAction::Unchanged;
    }
    match read_head(&f.path, row.head.len()) {
        Ok(head) if head == row.head => {}
        _ => return FileAction::Reparse,
    }
    if f.size > row.parsed_offset {
        FileAction::Append
    } else {
        FileAction::StatOnly
    }
}

/// Diffs the scan against the `files` rows: dirty session plans (sorted by id) and the sessions
/// whose last file disappeared.
fn plan_sync(
    scan: &ScanResult,
    db: Vec<FileRecord>,
    sidecar_dirty: &HashSet<String>,
) -> (Vec<SessionPlan>, Vec<String>) {
    let mut db_by_path: HashMap<String, FileRecord> =
        db.into_iter().map(|r| (r.path.clone(), r)).collect();
    let mut current: BTreeMap<String, Vec<ScannedFile>> = BTreeMap::new();
    for f in &scan.files {
        current
            .entry(f.session_id.clone())
            .or_default()
            .push(f.clone());
    }
    let mut sidecars: HashMap<String, Vec<SidecarFile>> = HashMap::new();
    for s in &scan.sidecars {
        sidecars
            .entry(s.session_id.clone())
            .or_default()
            .push(s.clone());
    }

    #[derive(Default)]
    struct Acc {
        reparse: bool,
        recompute: bool,
        ingest: Vec<Ingest>,
        stat_only: Vec<(i64, u64, i64)>,
    }
    let mut acc: BTreeMap<String, Acc> = BTreeMap::new();
    for f in &scan.files {
        let key = f.path.to_string_lossy().into_owned();
        let a = acc.entry(f.session_id.clone()).or_default();
        match db_by_path.remove(&key) {
            None => {
                a.recompute = true;
                a.ingest.push(Ingest {
                    file: f.clone(),
                    from: 0,
                    existing: None,
                });
            }
            Some(row) => match classify(f, &row) {
                FileAction::Unchanged => {}
                FileAction::StatOnly => a.stat_only.push((row.id, f.size, f.mtime_ns)),
                FileAction::Append => {
                    a.recompute = true;
                    a.ingest.push(Ingest {
                        file: f.clone(),
                        from: row.parsed_offset,
                        existing: Some(row),
                    });
                }
                FileAction::Reparse => a.reparse = true,
            },
        }
    }
    // Rows whose file is gone: reparse the session if other files survive (A7), else remove it.
    let mut removed = Vec::new();
    for row in db_by_path.into_values() {
        if current.contains_key(&row.session_id) {
            acc.entry(row.session_id).or_default().reparse = true;
        } else if !removed.contains(&row.session_id) {
            removed.push(row.session_id);
        }
    }
    for sid in sidecar_dirty {
        if current.contains_key(sid) {
            acc.entry(sid.clone()).or_default().recompute = true;
        }
    }
    removed.sort();

    let mut plans = Vec::new();
    for (sid, a) in acc {
        if !a.reparse && !a.recompute && a.stat_only.is_empty() {
            continue;
        }
        let files = current.remove(&sid).unwrap_or_default();
        let ingest = if a.reparse {
            files
                .iter()
                .map(|f| Ingest {
                    file: f.clone(),
                    from: 0,
                    existing: None,
                })
                .collect()
        } else {
            a.ingest
        };
        plans.push(SessionPlan {
            reparse: a.reparse,
            recompute: a.reparse || a.recompute,
            stat_only: if a.reparse { Vec::new() } else { a.stat_only },
            ingest,
            current: files,
            sidecars: sidecars.remove(&sid).unwrap_or_default(),
            sid,
        });
    }
    (plans, removed)
}

/// Splits plans into batches of about [`BATCH_BYTES`] parse cost.
fn batches(plans: Vec<SessionPlan>) -> Vec<Vec<SessionPlan>> {
    let mut out: Vec<Vec<SessionPlan>> = Vec::new();
    let mut cur = Vec::new();
    let mut bytes = 0u64;
    for p in plans {
        let c = p.cost();
        if !cur.is_empty() && bytes + c > BATCH_BYTES {
            out.push(std::mem::take(&mut cur));
            bytes = 0;
        }
        bytes += c;
        cur.push(p);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

// -------------------------------------------------------------------- per-session compute

struct FileOut {
    file: ScannedFile,
    rows: FileRows,
    head: Vec<u8>,
    parsed_offset: u64,
    line_count: u64,
    failed_lines: u64,
    first_error: Option<String>,
}

struct SessionSummaryOut {
    agg: SessionAggregates,
    project: ProjectRow,
    /// Newest main-file mtime (ms), used when no entry has a timestamp.
    mtime_ms: f64,
}

struct SessionWork {
    sid: String,
    reparse: bool,
    recompute: bool,
    files: Vec<FileOut>,
    stat_only: Vec<(i64, u64, i64)>,
    /// `None` when the session has no readable main file.
    summary: Option<SessionSummaryOut>,
    agents: Vec<AgentRow>,
    subagent_count: u32,
    main_set: Option<HashSet<String>>,
    cache: Option<CachedSession>,
    bytes: u64,
}

/// Reads and parses a dirty session's files (no DB access; runs on the rayon pool).
fn compute_session(
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

// -------------------------------------------------------------------- write

/// `is_empty` of an existing session row.
fn session_exists(conn: &Connection, sid: &str) -> CoreResult<Option<bool>> {
    Ok(conn
        .prepare_cached("SELECT is_empty FROM sessions WHERE id=?1")?
        .query_row([sid], |r| r.get::<_, bool>(0))
        .optional()?)
}

/// Above this many changed sessions (a full scan), billing is redone for every session instead of
/// looking up the neighbors of each.
const REBILL_ALL_ABOVE: usize = 200;

/// Re-bills the changed sessions and every session sharing a message with them; those whose
/// totals may have moved are reported as changed too.
fn rebill_changed(
    conn: &Connection,
    cs: &mut ChangeSet,
    mut neighbors: HashSet<String>,
) -> CoreResult<()> {
    if cs.changed.is_empty() && neighbors.is_empty() {
        return Ok(());
    }
    if cs.changed.len() > REBILL_ALL_ABOVE {
        return writer::rebill(conn, None);
    }
    for sid in &cs.changed {
        neighbors.extend(writer::copy_neighbors(conn, sid)?);
    }
    let removed: HashSet<&String> = cs.removed.iter().collect();
    neighbors.retain(|s| !removed.contains(s));
    let mut scope: HashSet<String> = cs.changed.iter().cloned().collect();
    for n in neighbors {
        if scope.insert(n.clone()) {
            cs.changed.push(n);
        }
    }
    writer::rebill(conn, Some(&scope))
}

fn write_work(
    conn: &Connection,
    w: &SessionWork,
    cs: &mut ChangeSet,
    neighbors: &mut HashSet<String>,
    resolver: &mut project::Resolver,
) -> CoreResult<()> {
    for (id, size, mtime) in &w.stat_only {
        writer::write_file_stat(conn, *id, *size, *mtime)?;
    }
    if !w.recompute {
        return Ok(());
    }
    let before = session_exists(conn, &w.sid)?;
    if w.reparse {
        neighbors.extend(writer::copy_neighbors(conn, &w.sid)?);
        writer::delete_session(conn, &w.sid)?;
    }
    for f in &w.files {
        let path = f.file.path.to_string_lossy();
        let id = writer::write_file(
            conn,
            &FileWrite {
                path: &path,
                dev: f.file.dev,
                ino: f.file.ino,
                role: f.file.role,
                session_id: &w.sid,
                agent_id: f.file.agent_id.as_deref(),
                size: f.file.size,
                mtime_ns: f.file.mtime_ns,
                head: &f.head,
                parsed_offset: f.parsed_offset,
                line_count: f.line_count,
                failed_lines: f.failed_lines,
                first_error: f.first_error.as_deref(),
            },
        )?;
        // Phase-2 hook (T3.3): text docs for `[text_offset, parsed_offset)` are extracted by
        // `index::text::index_pending`; `text_offset` is deliberately left untouched here.
        writer::write_rows(
            conn,
            id,
            &w.sid,
            f.file.agent_id.as_deref().unwrap_or(""),
            &f.rows,
        )?;
    }
    let Some(s) = &w.summary else {
        if before.is_some() {
            conn.execute("DELETE FROM sessions WHERE id=?1", [&w.sid])?;
            writer::write_session_projects(conn, &w.sid, &[])?;
            cs.removed.push(w.sid.clone());
            cs.projects_changed = true;
        }
        return Ok(());
    };
    writer::write_subagents(conn, &w.sid, &w.agents)?;
    let a = &s.agg;
    // Placeholders: `rebill_changed` re-sums the token columns from billed messages.
    let main_tok = a.tokens.clone();
    let row = SessionRow {
        id: w.sid.clone(),
        project_id: s.project.id.clone(),
        cwd: a.cwd.clone(),
        title: a.title.clone(),
        title_source: a.title_source,
        first_prompt: a.first_prompt.clone(),
        created_ms: a.created_ms,
        last_active_ms: if a.last_active_ms > 0.0 {
            a.last_active_ms
        } else {
            s.mtime_ms
        },
        duration_ms: a.duration_ms,
        message_count: a.message_count,
        tool_call_count: a.tool_call_count,
        subagent_count: w.subagent_count,
        tokens: main_tok.clone(),
        main_tokens: main_tok,
        git_branch: a.git_branch.clone(),
        primary_model: a.primary_model.clone(),
        models: a.models.clone(),
        versions: a.versions.clone(),
        leaf_uuid: a.leaf_uuid.clone(),
        root_uuid: a.root_uuid.clone(),
        fork_origin_id: a.fork_origin_id.clone(),
        fork_point_uuid: a.fork_point_uuid.clone(),
        is_empty: a.is_empty,
        dup_uuids: a.duplicate_uuids,
    };
    writer::write_session(conn, &row)?;
    if writer::write_project(conn, &s.project)? || before != Some(a.is_empty) {
        cs.projects_changed = true;
    }
    if write_edited_projects(conn, &w.sid, &s.project, resolver)? {
        cs.projects_changed = true;
    }
    if let Some(ms) = &w.main_set
        && writer::has_text_rows(conn, &w.sid)?
    {
        writer::write_on_main_line(conn, &w.sid, ms)?;
    }
    cs.changed.push(w.sid.clone());
    Ok(())
}

/// Re-derives the Projects a session edited from every edit path recorded for it (main file and
/// subagents); returns true when the set or any Project row changed.
fn write_edited_projects(
    conn: &Connection,
    sid: &str,
    start: &ProjectRow,
    resolver: &mut project::Resolver,
) -> CoreResult<bool> {
    let mut rows: BTreeMap<String, ProjectRow> = BTreeMap::new();
    for path in writer::edited_paths(conn, sid)? {
        if let Some(p) = resolver.edit_project(&path, start) {
            rows.entry(p.id.clone()).or_insert(p);
        }
    }
    let mut changed = false;
    for p in rows.values() {
        changed |= writer::write_project(conn, p)?;
    }
    let ids: Vec<String> = rows.into_keys().collect();
    changed |= writer::write_session_projects(conn, sid, &ids)?;
    Ok(changed)
}

/// Quotes a session id for pasting into a shell; ids come from file names, so anything beyond
/// the usual UUID alphabet is single-quoted.
fn shell_quote(id: &str) -> String {
    if !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        id.to_owned()
    } else {
        format!("'{}'", id.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn resume_command_quotes_unusual_ids() {
        assert_eq!(
            super::shell_quote("6f1c2d3e-4a5b-4c6d-8e7f-0a1b2c3d4e5f"),
            "6f1c2d3e-4a5b-4c6d-8e7f-0a1b2c3d4e5f"
        );
        assert_eq!(super::shell_quote("x; rm -rf ~"), "'x; rm -rf ~'");
        assert_eq!(super::shell_quote("a'b"), "'a'\\''b'");
    }

    use super::*;

    #[test]
    fn open_creates_cache_dir_and_reports_app_info() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join("cache");
        let engine = Engine::open(EngineConfig {
            data_root: tmp.path().join("missing-root"),
            data_root_source: DataRootSource::Default,
            cache_dir: cache_dir.clone(),
        })
        .unwrap();
        assert!(cache_dir.is_dir());
        let info = engine.app_info();
        assert!(!info.data_root_exists);
        assert_eq!(info.schema_version, SCHEMA_VERSION);
        assert_eq!(engine.scan_all(&|_| {}).unwrap(), ChangeSet::default());
    }

    #[test]
    fn text_backlog_commits_before_yielding_and_reports_its_own_progress() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/subagents");
        let tmp = tempfile::tempdir().unwrap();
        let engine = Engine::open(EngineConfig {
            data_root: root,
            data_root_source: DataRootSource::Settings,
            cache_dir: tmp.path().to_path_buf(),
        })
        .unwrap();
        engine.scan_all(&|_| {}).unwrap();
        let counts = |e: &Engine| {
            e.index
                .read(|c| {
                    Ok(c.query_row(
                        "SELECT (SELECT count(*) FROM msg_text),
                                (SELECT count(*) FROM files WHERE text_offset >= parsed_offset),
                                (SELECT count(*) FROM files)",
                        [],
                        |r| {
                            Ok((
                                r.get::<_, u32>(0)?,
                                r.get::<_, u32>(1)?,
                                r.get::<_, u32>(2)?,
                            ))
                        },
                    )?)
                })
                .unwrap()
        };
        let (rows, done, files) = counts(&engine);
        assert_eq!((rows, done), (0, 0));
        assert!(files > 2);

        // Asked to yield before starting, with one file per transaction: exactly one commits.
        let seen = Mutex::new(Vec::new());
        let res = engine.index.write(|conn| {
            text::index_pending(
                conn,
                &mut |_| Some(HashSet::new()),
                &|c| seen.lock().push(c),
                &AtomicBool::new(true),
                1,
            )
        });
        assert!(matches!(res, Err(CoreError::Cancelled)));
        let (rows, done, _) = counts(&engine);
        assert!(rows > 0, "the first transaction committed");
        assert_eq!(done, 1);
        let seen = seen.into_inner();
        assert_eq!(seen.len(), 2);
        assert_eq!((seen[0].files_done, seen[0].files_total), (0, files));
        assert_eq!((seen[1].files_done, seen[1].files_total), (1, files));
        assert!(seen[1].bytes_done > 0 && seen[1].bytes_done < seen[1].bytes_total);

        // The resumed backlog reports phase-2 progress over all files and finishes.
        let statuses = Mutex::new(Vec::new());
        engine
            .index_text_backlog(
                &|s| statuses.lock().push(s.clone()),
                &AtomicBool::new(false),
            )
            .unwrap();
        let statuses = statuses.into_inner();
        let first = &statuses[0];
        assert_eq!(first.phase, IndexPhase::IndexingText);
        assert_eq!((first.files_done, first.files_total), (1, files));
        let last = statuses.last().unwrap();
        assert_eq!(last.phase, IndexPhase::Idle);
        assert!(last.text_ready);
        assert_eq!((last.files_done, last.files_total), (files, files));
        assert_eq!(last.bytes_done, last.bytes_total);
        assert_eq!(counts(&engine).1, files);
    }

    #[test]
    fn text_backlog_leaves_a_session_pending_when_its_main_line_fails() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/subagents");
        let tmp = tempfile::tempdir().unwrap();
        let engine = Engine::open(EngineConfig {
            data_root: root,
            data_root_source: DataRootSource::Settings,
            cache_dir: tmp.path().to_path_buf(),
        })
        .unwrap();
        engine.scan_all(&|_| {}).unwrap();
        let seen = Mutex::new(Vec::new());
        engine
            .index
            .write(|conn| {
                text::index_pending(
                    conn,
                    &mut |_| None,
                    &|c| seen.lock().push(c),
                    &AtomicBool::new(false),
                    text::BATCH_BYTES,
                )
            })
            .unwrap();
        let (rows, ready) = engine
            .index
            .read(|c| {
                let rows: u32 = c.query_row("SELECT count(*) FROM msg_text", [], |r| r.get(0))?;
                Ok((rows, reader::text_ready(c)?))
            })
            .unwrap();
        assert_eq!((rows, ready), (0, false));
        let last = *seen.lock().last().unwrap();
        assert_eq!((last.files_done, last.bytes_done), (0, 0));
    }

    #[test]
    fn lru_keeps_most_recent() {
        let mut l = Lru::new(2);
        l.put(1, "a");
        l.put(2, "b");
        l.put(3, "c");
        assert_eq!(l.take(&1), None);
        assert_eq!(l.take(&3), Some("c"));
    }
}
