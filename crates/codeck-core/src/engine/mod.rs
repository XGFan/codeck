//! Engine facade: the only API src-tauri calls. Blocking; callers run it off the main thread.
//!
//! Index sync ([`sync`]; phase 1: metadata, phase 2 is the full-text backlog): stat-walk the data
//! root, diff against `files`, light-parse changed files in parallel (rayon), write rows in one
//! transaction per batch, then recompute every dirty session from its skeleton. Appends read only
//! `[parsed_offset, EOF)`; a shrunk, replaced or rewritten file, or a removed file whose session
//! still has other files, triggers a session-level reparse.
//!
//! Display ([`session_cache`]): an LRU of assembled Sessions keyed by session id and validated by
//! a revision hash over (path, size, mtime) of every source and sidecar file. Main files are kept
//! parsed per (dev, ino) with their consumed offset, so a live append parses only the new bytes.
//!
//! Source files are only ever opened with `File::open` (read-only, ADR-0002).

mod session_cache;
mod sync;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use parking_lot::{Mutex, RwLock};
use transcript::assemble::{self, LoadedFile};
use transcript::live;
use transcript::pricing::PriceTable;

use crate::error::{CoreError, CoreResult};
use crate::index::reader;
use crate::index::schema::SCHEMA_VERSION;
use crate::index::writer;
use crate::index::{Index, text};
use crate::model::{
    AppInfo, DataRootSource, Diagnostics, FileRole, FindRequest, FindResult, ImageData,
    ImageRequest, IndexPhase, IndexStatus, JumpRequest, JumpTarget, LiveChanged, LiveEntry,
    LiveState, ProjectSummary, SearchRequest, SearchResponse, SessionDetail, SessionQuery,
    SessionSummary, SourceFile, Stats, StatsRequest, TokenTotals, ToolDetail, ToolDetailRequest,
    Transcript, TranscriptRequest, TranscriptScope,
};
use crate::paths::{index_db_path, prices_cache_path, projects_dir, sessions_dir};
use crate::{diag, search, stats};
use session_cache::{CachedSession, FileState, Lru};

/// Assembled Sessions kept for display.
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
    /// A sync failed: the next one scans the whole root, whatever its batch names (the sessions
    /// of a failed per-session sync would otherwise wait until they change again).
    full_scan_next: AtomicBool,
}

/// Sidecar path → (size, mtime, session id).
type SidecarState = HashMap<PathBuf, (u64, i64, String)>;

/// Session ids the changed `paths` name (`projects/<dir>/<sid>.jsonl` or anything under
/// `projects/<dir>/<sid>/`, also via the realpath of `projects`), ignoring `<root>/sessions`;
/// `None` when a path names no session (a project dir itself, a `<sid>/` dir moved in, anything
/// else), or names none at all: then the whole root is scanned.
///
/// Changes no batch reports are caught by the next full scan: after a failed sync, after a
/// watcher error (the worker queues a Rescan), and at the latest by the periodic Rescan, the
/// last resort.
fn changed_sessions(root: &Path, paths: &[PathBuf]) -> Option<BTreeSet<String>> {
    let projects = projects_dir(root);
    let real_projects = projects.canonicalize().ok();
    let sessions = sessions_dir(root);
    let mut out = BTreeSet::new();
    for p in paths.iter().filter(|p| !p.starts_with(&sessions)) {
        let rel = p
            .strip_prefix(&projects)
            .ok()
            .or_else(|| p.strip_prefix(real_projects.as_ref()?).ok())?;
        let comps = rel
            .components()
            .map(|c| match c {
                Component::Normal(s) => s.to_str(),
                _ => None,
            })
            .collect::<Option<Vec<&str>>>()?;
        let sid = match comps.as_slice() {
            [_, name] => name.strip_suffix(".jsonl")?,
            [_, sid, _, ..] => sid,
            _ => return None,
        };
        out.insert(sid.to_owned());
    }
    (!out.is_empty()).then_some(out)
}

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
            full_scan_next: AtomicBool::new(false),
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
        self.sync(Some(p), false, None)
    }

    /// Phase 2: full-text backlog (`index::text`). `on_main_line` is set at insert time from
    /// the session's Main Line set (`assemble::tree::main_set`).
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
    /// (see `poll_live`), not the index; anything else re-runs the stat diff, limited to the
    /// sessions the paths name when every path names one.
    pub fn apply_changes(&self, paths: &[PathBuf]) -> CoreResult<ChangeSet> {
        let sessions = sessions_dir(&self.cfg.data_root);
        if !paths.is_empty() && paths.iter().all(|p| p.starts_with(&sessions)) {
            return Ok(ChangeSet::default());
        }
        let scope = changed_sessions(&self.cfg.data_root, paths);
        self.sync(None, true, scope.as_ref())
    }

    /// Returns the new Live Session snapshot when it changed since the last poll.
    /// Safe to call from several threads: the snapshot is taken under the lock, so an older one
    /// never replaces a newer one.
    pub fn poll_live(&self) -> CoreResult<Option<LiveChanged>> {
        let mut last = self.live.lock();
        let snapshot: Vec<LiveEntry> = self
            .live_map()?
            .into_iter()
            .map(|(session_id, state)| LiveEntry { session_id, state })
            .collect();
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
        let (row, files, forks, failed, model_costs, superseded_by) = self.index.read(|c| {
            let Some(row) = reader::session_detail(c, id)? else {
                return Err(CoreError::NotFound("该 Session".into()));
            };
            let files = reader::session_files(c, id)?;
            let forks = reader::fork_children(c, id)?;
            let failed = files.iter().map(|f| f.failed_lines).sum::<u64>();
            let model_costs = reader::model_costs(c, id)?;
            let superseded_by = reader::superseded_by(c, id)?;
            Ok((row, files, forks, failed, model_costs, superseded_by))
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
            superseded_by,
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

    /// Paths to complete `partial` with in a Session started in `cwd`: directories (`/add-dir`), or files too (`@`).
    pub fn complete_paths(&self, cwd: Option<&str>, partial: &str, files: bool) -> Vec<String> {
        crate::slash::complete_paths(cwd.map(Path::new), partial, files)
    }

    // ---------------------------------------------------------------- live

    fn live_map(&self) -> CoreResult<BTreeMap<String, LiveState>> {
        let pfs = live::read_process_files(&sessions_dir(&self.cfg.data_root));
        let mut live = live::detect(&pfs);
        // A superseded Session's process stays as the foreground of the one it was parked in; the
        // conversation is Live there, under the Session that carries it on.
        let superseded = self
            .index
            .read(|c| reader::superseded_among(c, live.keys()))?;
        live.retain(|id, _| !superseded.contains(id));
        Ok(live)
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
    use std::collections::HashSet;

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
}
