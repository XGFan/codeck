//! Display caches: assembled Sessions and Subagent Run files kept parsed, validated by a
//! revision over every source and sidecar file, and extended in place on appends.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use transcript::assemble::{
    self, AssembledSession, LoadedFile, SessionSidecar, SubagentMetaFile, WorkflowFiles,
};
use transcript::parse::{ParsedChunk, parse_bytes};

use super::Engine;
use crate::error::{CoreError, CoreResult};
use crate::index::reader;
use crate::index::schema::HEAD_LEN;
use crate::model::{FileRole, TranscriptScope};
use crate::scan::{self, ScannedFile, SessionDirListing, SidecarKind};

impl Engine {
    /// The Main Line set of a session, from its main files.
    /// Reuses (and extends) a cached parse of the session, so live appends stay cheap.
    pub(super) fn main_set_of(&self, sid: &str) -> CoreResult<HashSet<String>> {
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
    pub(super) fn with_session<T>(
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
pub(super) struct Lru<K, V> {
    pub(super) cap: usize,
    pub(super) items: Vec<(K, V)>,
}

impl<K: PartialEq, V> Lru<K, V> {
    pub(super) fn new(cap: usize) -> Self {
        Lru {
            cap,
            items: Vec::new(),
        }
    }

    pub(super) fn take(&mut self, k: &K) -> Option<V> {
        let i = self.items.iter().position(|(key, _)| key == k)?;
        Some(self.items.remove(i).1)
    }

    pub(super) fn put(&mut self, k: K, v: V) {
        self.items.retain(|(key, _)| *key != k);
        self.items.insert(0, (k, v));
        self.items.truncate(self.cap);
    }

    /// Keeps `v` only if there is room: it evicts nothing (a sync warming the cache with the
    /// sessions it parsed must not push out the ones being read).
    pub(super) fn put_cold(&mut self, k: K, v: V) {
        self.items.retain(|(key, _)| *key != k);
        if self.items.len() < self.cap {
            self.items.push((k, v));
        }
    }
}

/// Parse state of one cached file (extended in place while the file only grows).
#[derive(Clone, Debug)]
pub(super) struct FileState {
    pub(super) path: PathBuf,
    pub(super) dev: u64,
    pub(super) ino: u64,
    /// Absolute offset just past the last consumed `\n`.
    pub(super) consumed: u64,
    /// `\n` count in `[0, consumed)` (absolute line numbers of appended records).
    pub(super) lines: u32,
    pub(super) failed: u32,
    pub(super) first_error: Option<String>,
    /// First ≤ [`HEAD_LEN`] bytes, to detect an in-place rewrite.
    pub(super) head: Vec<u8>,
}

impl FileState {
    /// Same file (dev, ino, head bytes) that has not shrunk below what was parsed.
    pub(super) fn matches(&self, f: &ScannedFile) -> bool {
        self.dev == f.dev
            && self.ino == f.ino
            && f.size >= self.consumed
            && read_head(&f.path, self.head.len()).is_ok_and(|h| h == self.head)
    }
}

pub(super) enum Body {
    Files(Vec<LoadedFile>),
    Assembled(Box<AssembledSession>),
}

/// Parsed main files of a session, optionally assembled.
pub(super) struct CachedSession {
    /// Parallel to the main files.
    pub(super) states: Vec<FileState>,
    pub(super) body: Body,
}

impl CachedSession {
    pub(super) fn assembled_revision(&self) -> Option<f64> {
        match &self.body {
            Body::Assembled(a) => Some(a.revision),
            Body::Files(_) => None,
        }
    }

    pub(super) fn into_parts(self) -> (Vec<FileState>, Vec<LoadedFile>) {
        let files = match self.body {
            Body::Files(f) => f,
            Body::Assembled(mut a) => std::mem::take(&mut a.main_files),
        };
        (self.states, files)
    }
}

pub(super) struct SessionCtx {
    /// Largest first.
    pub(super) main: Vec<ScannedFile>,
    pub(super) session_dir: PathBuf,
    pub(super) listing: SessionDirListing,
    pub(super) revision: f64,
}

/// FNV-1a over (path, size, mtime) of a session's files, as an exact f64 (53 bits).
pub(super) struct Fnv(u64);

impl Fnv {
    pub(super) fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    pub(super) fn bytes(&mut self, b: &[u8]) {
        for x in b {
            self.0 ^= u64::from(*x);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    pub(super) fn file(&mut self, path: &Path, size: u64, mtime_ns: i64) {
        self.bytes(path.as_os_str().as_encoded_bytes());
        self.bytes(&size.to_le_bytes());
        self.bytes(&mtime_ns.to_le_bytes());
    }

    pub(super) fn revision(&self) -> f64 {
        (self.0 & ((1u64 << 53) - 1)) as f64
    }
}

// -------------------------------------------------------------------- file reading

/// Reads `[from, EOF)` and parses its complete lines; also returns the `\n` count consumed.
pub(super) fn read_chunk(path: &Path, from: u64) -> io::Result<(ParsedChunk, u32)> {
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
pub(super) fn read_head(path: &Path, len: usize) -> io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(len);
    File::open(path)?.take(len as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

pub(super) fn read_text(path: &Path) -> Option<String> {
    let mut s = String::new();
    File::open(path).ok()?.read_to_string(&mut s).ok()?;
    Some(s)
}

pub(super) fn load_fresh(f: &ScannedFile) -> io::Result<(FileState, LoadedFile)> {
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
pub(super) fn extend_loaded(st: &mut FileState, lf: &mut LoadedFile) -> io::Result<()> {
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

pub(super) type CachedFiles = HashMap<PathBuf, (FileState, LoadedFile)>;

pub(super) fn cached_files(cached: Option<(Vec<FileState>, Vec<LoadedFile>)>) -> CachedFiles {
    cached
        .map(|(s, f)| {
            s.into_iter()
                .zip(f)
                .map(|(s, f)| (s.path.clone(), (s, f)))
                .collect()
        })
        .unwrap_or_default()
}

/// Parses `f`, extending its cached parse when the file only grew.
pub(super) fn load_one(
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
pub(super) fn load_main(
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

pub(super) fn sort_main(main: &mut [ScannedFile]) {
    main.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
}

pub(super) fn workflow_entry<'a>(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_keeps_most_recent() {
        let mut l = Lru::new(2);
        l.put(1, "a");
        l.put(2, "b");
        l.put(3, "c");
        assert_eq!(l.take(&1), None);
        assert_eq!(l.take(&3), Some("c"));
    }

    #[test]
    fn lru_cold_put_evicts_nothing() {
        let mut l = Lru::new(2);
        l.put_cold(1, "a");
        l.put(2, "b");
        l.put_cold(3, "c");
        assert_eq!(l.take(&3), None);
        assert_eq!(l.take(&2), Some("b"));
        assert_eq!(l.take(&1), Some("a"));
    }
}
