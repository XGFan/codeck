//! Index sync (phase 1): plan from the scan and the `files` rows, compute sessions in
//! parallel, write them batch by batch, then re-bill.

mod compute;
mod plan;
mod write;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::Ordering;

use rayon::prelude::*;
use transcript::agy;

use super::session_cache::CachedSession;
use super::{ChangeSet, Engine};
use crate::error::CoreResult;
use crate::index::{reader, writer};
use crate::model::{FileRole, IndexPhase, IndexStatus};
use crate::scan::FileSource;
use crate::{project, scan};
use compute::{SessionWork, compute_session};
use plan::{SessionPlan, batches, diff_sidecars, plan_sync};
use write::{rebill_changed, session_exists, update_superseded, write_work};

/// Scope of an index sync.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SyncScope {
    /// Scan both roots in full.
    Full,
    /// Scan only `sessions` on the Claude Code side; on the agy side, scan all of `agy_root`
    /// when `agy_all` is set (e.g. `conversation_metadata.json` changed) or only `sessions`.
    Scoped {
        sessions: BTreeSet<String>,
        agy_all: bool,
    },
}

fn read_agy_metadata(agy_root: &Path) -> HashMap<String, agy::Meta> {
    match agy::read_metadata(&agy::metadata_path(agy_root)) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
        Err(e) => {
            tracing::warn!(error = %e, "读取 agy metadata 失败");
            HashMap::new()
        }
    }
}

impl Engine {
    /// `scope`: what changed (a filesystem batch); both roots are scanned in full without a scope,
    /// before the first sync (no sidecar state to diff against yet) and after a failed sync.
    pub(super) fn sync(
        &self,
        p: Option<&dyn Fn(&IndexStatus)>,
        keep_cache: bool,
        scope: SyncScope,
    ) -> CoreResult<ChangeSet> {
        let _g = self.sync_lock.lock();
        let scope = if self.sidecar_state.lock().is_some()
            && !self.full_scan_next.load(Ordering::Relaxed)
        {
            scope
        } else {
            SyncScope::Full
        };
        // A session that became superseded, stopped being, or changed the session carrying it on
        // hands its agent files over (or gets them back) only on a scan made after that: more
        // passes over those sessions until nothing changes (a few at most). A later pass that
        // fails, or a handover left unfinished, still reports what was changed; the next sync
        // scans everything (also set by `scan_scope`).
        if matches!(scope, SyncScope::Full) {
            self.full_scan_next.store(false, Ordering::Relaxed);
        }
        let mut later_failed = false;
        let r = self
            .sync_locked(p, keep_cache, &scope)
            .map(|(mut cs, mut moved)| {
                for _ in 0..MAX_HANDOVER_PASSES {
                    if moved.is_empty() {
                        break;
                    }
                    let pass_scope = SyncScope::Scoped {
                        sessions: std::mem::take(&mut moved),
                        agy_all: false,
                    };
                    match self.sync_locked(p, keep_cache, &pass_scope) {
                        Ok((more, next)) => {
                            cs.changed.extend(more.changed);
                            cs.removed.extend(more.removed);
                            cs.projects_changed |= more.projects_changed;
                            moved = next;
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "交接被接替 Session 的文件失败");
                            later_failed = true;
                            break;
                        }
                    }
                }
                later_failed |= !moved.is_empty();
                cs.changed.sort();
                cs.changed.dedup();
                cs.removed.sort();
                cs.removed.dedup();
                cs
            });
        if r.is_err() || later_failed {
            self.full_scan_next.store(true, Ordering::Relaxed);
        }
        r
    }

    /// Also returns the sessions whose agent files may now belong elsewhere: those whose
    /// superseded flag or carrying session changed, with the sessions carrying them before and
    /// after.
    fn sync_locked(
        &self,
        p: Option<&dyn Fn(&IndexStatus)>,
        keep_cache: bool,
        scope: &SyncScope,
    ) -> CoreResult<(ChangeSet, BTreeSet<String>)> {
        let p: &dyn Fn(&IndexStatus) = p.unwrap_or(&|_| {});
        let agy_meta = read_agy_metadata(&self.cfg.agy_root);
        let (mut scan, db_files, claude_scope, stored_fps) = match scope {
            SyncScope::Scoped { sessions, agy_all } => {
                let (scan, db, widened) = self.scan_scope(sessions, *agy_all)?;
                let fps = self
                    .index
                    .read(|c| reader::agy_meta_fingerprints(c, (!agy_all).then_some(&widened)))?;
                (scan, db, Some(widened), fps)
            }
            SyncScope::Full => (
                scan::scan_roots(&self.cfg.data_root, &self.cfg.agy_root),
                self.index.read(reader::all_files)?,
                None,
                self.index
                    .read(|c| reader::agy_meta_fingerprints(c, None))?,
            ),
        };
        let claude_scope = claude_scope.as_ref();
        let adopt_before = self.index.read(reader::adoptions)?;
        let mains: HashSet<&String> = scan
            .files
            .iter()
            .filter(|f| f.role == FileRole::Main)
            .map(|f| &f.session_id)
            .collect();
        let adopt: Vec<(String, String)> = adopt_before
            .iter()
            .filter(|(_, to)| mains.contains(to))
            .cloned()
            .collect();
        scan::adopt(&mut scan.files, &mut scan.sidecars, &adopt);
        let (sidecar_dirty, sidecar_now) = {
            let prev = self.sidecar_state.lock();
            match (claude_scope, prev.as_ref()) {
                (Some(s), Some(prev)) => {
                    let prev = prev
                        .iter()
                        .filter(|(_, v)| s.contains(&v.2))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    diff_sidecars(Some(&prev), &scan, &db_files)
                }
                _ => diff_sidecars(prev.as_ref(), &scan, &db_files),
            }
        };
        let mut recompute_dirty = sidecar_dirty;
        for f in scan.files.iter().filter(|f| f.source == FileSource::Agy) {
            let cur_fp = agy_meta.get(&f.session_id).map(|m| m.fingerprint as i64);
            if stored_fps
                .get(&f.session_id)
                .is_some_and(|&stored| stored != cur_fp)
            {
                recompute_dirty.insert(f.session_id.clone());
            }
        }
        let (plans, removed, moved) = plan_sync(&scan, db_files, &recompute_dirty);
        let with_text = self.index.read(|c| {
            reader::sessions_with_text(c, plans.iter().filter(|p| !p.reparse).map(|p| &p.sid))
        })?;

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
        // Session rows are committed batch by batch, billing only at the end: a sync that stops in
        // between leaves the flag set, and the next one re-bills everything.
        let mut rebill_all = false;
        let mut flipped = Vec::new();
        let result = (|| -> CoreResult<()> {
            self.index.write(|conn| {
                let tx = conn.transaction()?;
                rebill_all = writer::rebill_pending(&tx)?;
                if !removed.is_empty() || !plans.is_empty() {
                    writer::set_rebill_pending(&tx, true)?;
                }
                for row in &moved {
                    writer::delete_file(&tx, row.id)?;
                }
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
                let jobs: Vec<(SessionPlan, Option<CachedSession>, bool)> = {
                    let mut cache = self.sessions.lock();
                    batch
                        .into_iter()
                        .map(|plan| {
                            // An agy metadata-only recompute does not alter the transcript; leave
                            // any assembled display cache entry intact.
                            let meta_only = !plan.reparse
                                && plan.ingest.is_empty()
                                && plan.current.iter().all(|f| f.source == FileSource::Agy);
                            if meta_only {
                                (plan, None, false)
                            } else {
                                let cached = cache.take(&plan.sid);
                                let keep = keep_cache || cached.is_some();
                                (plan, cached, keep)
                            }
                        })
                        .collect()
                };
                let was_cached: HashSet<String> = jobs
                    .iter()
                    .filter(|(_, c, _)| c.is_some())
                    .map(|(p, _, _)| p.sid.clone())
                    .collect();
                let works: Vec<SessionWork> = jobs
                    .into_par_iter()
                    .map(|(plan, cached, keep)| {
                        let need_main_set = !plan.reparse && with_text.contains(&plan.sid);
                        let meta = agy_meta.get(&plan.sid);
                        compute_session(
                            plan,
                            cached,
                            keep,
                            need_main_set,
                            meta,
                            &self.cfg.agy_root,
                            &self.cfg.haa_home,
                        )
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
                        match w.cache {
                            Some(c) if was_cached.contains(&w.sid) => cache.put(w.sid, c),
                            Some(c) => cache.put_cold(w.sid, c),
                            None => {}
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
                flipped = update_superseded(&tx, &mut cs, rebill_all)?;
                if rebill_all {
                    writer::rebill(&tx, None)?;
                } else {
                    rebill_changed(&tx, &mut cs, neighbors)?;
                }
                writer::set_rebill_pending(&tx, false)?;
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
        {
            let mut state = self.sidecar_state.lock();
            match (claude_scope, state.as_mut()) {
                (Some(s), Some(state)) => {
                    state.retain(|_, v| !s.contains(&v.2));
                    state.extend(sidecar_now);
                }
                _ => *state = Some(sidecar_now),
            }
        }
        cs.changed.sort();
        cs.changed.dedup();
        cs.removed.sort();
        let adopt_after = self.index.read(reader::adoptions)?;
        let mut moved: BTreeSet<String> = flipped.into_iter().collect();
        for (old, to) in adopt_before.iter().chain(&adopt_after) {
            let before = adopt_before.iter().find(|(o, _)| o == old).map(|(_, t)| t);
            let after = adopt_after.iter().find(|(o, _)| o == old).map(|(_, t)| t);
            if before != after {
                moved.insert(old.clone());
                moved.insert(to.clone());
            }
        }
        Ok((cs, moved))
    }

    /// The scan of `scope` widened until it holds every session an agent file may move between:
    /// those linked by `continued-in`, the owner of each scanned agent file's row, and the
    /// session whose directory holds each agent file the rows of `scope` record (a file adopted
    /// by a session the links no longer reach). Also returns the `files` rows of the final scope.
    /// When `agy_all` is true, every file under `agy_root` (and every agy row in `files`) is
    /// included alongside the scoped Claude Code sessions.
    fn scan_scope(
        &self,
        scope: &BTreeSet<String>,
        agy_all: bool,
    ) -> CoreResult<(scan::ScanResult, Vec<reader::FileRecord>, BTreeSet<String>)> {
        let mut scope = if scope.is_empty() {
            BTreeSet::new()
        } else {
            self.index.read(|c| reader::with_continued(c, scope))?
        };
        let mut tries = 0;
        let (mut found, mut db) = loop {
            let found = if agy_all {
                if scope.is_empty() {
                    scan::ScanResult::default()
                } else {
                    scan::scan_sessions(&self.cfg.data_root, &scope)
                }
            } else {
                scan::scan_sessions_in_roots(&self.cfg.data_root, &self.cfg.agy_root, &scope)
            };
            let db = if scope.is_empty() {
                Vec::new()
            } else {
                let mut rows = self.index.read(|c| reader::files_of_sessions(c, &scope))?;
                if agy_all {
                    rows.retain(|r| r.source == FileSource::Claude);
                }
                rows
            };
            let scanned: HashSet<&std::path::Path> =
                found.files.iter().map(|f| f.path.as_path()).collect();
            let agent_paths: Vec<String> = found
                .files
                .iter()
                .filter(|f| f.role != FileRole::Main)
                .map(|f| f.path.to_string_lossy().into_owned())
                .collect();
            let mut more = self.index.read(|c| {
                reader::owners_outside(c, agent_paths.iter().map(String::as_str), &scope)
            })?;
            for r in db.iter().filter(|r| r.role != FileRole::Main) {
                let path = std::path::Path::new(&r.path);
                if !scanned.contains(path)
                    && path.exists()
                    && let Some(sid) = scan::session_of_agent_path(path)
                    && !scope.contains(&sid)
                {
                    more.insert(sid);
                }
            }
            tries += 1;
            if more.is_empty() {
                break (found, db);
            }
            if tries > MAX_HANDOVER_PASSES {
                // Still not every session involved: the next sync scans everything.
                self.full_scan_next.store(true, Ordering::Relaxed);
                break (found, db);
            }
            scope.extend(more);
            scope = self.index.read(|c| reader::with_continued(c, &scope))?;
        };
        if agy_all {
            found
                .files
                .extend(scan::scan_agy_root(&self.cfg.agy_root).files);
            db.extend(
                self.index
                    .read(|c| reader::files_of_source(c, FileSource::Agy))?,
            );
        }
        Ok((found, db, scope))
    }
}

/// Extra sync passes for agent files changing session, and widenings of a scoped scan.
const MAX_HANDOVER_PASSES: usize = 3;
