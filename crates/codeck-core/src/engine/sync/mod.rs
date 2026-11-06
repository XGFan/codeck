//! Index sync (phase 1): plan from the scan and the `files` rows, compute sessions in
//! parallel, write them batch by batch, then re-bill.

mod compute;
mod plan;
mod write;

use std::collections::{BTreeSet, HashSet};
use std::sync::atomic::Ordering;

use rayon::prelude::*;

use super::session_cache::CachedSession;
use super::{ChangeSet, Engine};
use crate::error::CoreResult;
use crate::index::{reader, writer};
use crate::model::{FileRole, IndexPhase, IndexStatus};
use crate::{project, scan};
use compute::{SessionWork, compute_session};
use plan::{SessionPlan, batches, diff_sidecars, plan_sync};
use write::{rebill_changed, session_exists, update_superseded, write_work};

impl Engine {
    /// `scope`: only these sessions changed (a filesystem batch); the whole root is scanned
    /// without it, before the first sync (no sidecar state to diff against yet) and after a
    /// failed sync.
    pub(super) fn sync(
        &self,
        p: Option<&dyn Fn(&IndexStatus)>,
        keep_cache: bool,
        scope: Option<&BTreeSet<String>>,
    ) -> CoreResult<ChangeSet> {
        let _g = self.sync_lock.lock();
        let scope = scope.filter(|_| {
            self.sidecar_state.lock().is_some() && !self.full_scan_next.load(Ordering::Relaxed)
        });
        // A session that became superseded (or stopped being) hands its agent files over (or gets
        // them back) only on a scan made after the flag changed: one more pass over those sessions.
        let r = self
            .sync_locked(p, keep_cache, scope)
            .and_then(|(mut cs, flipped)| {
                if !flipped.is_empty() {
                    let (more, _) = self.sync_locked(p, keep_cache, Some(&flipped))?;
                    cs.changed.extend(more.changed);
                    cs.removed.extend(more.removed);
                    cs.projects_changed |= more.projects_changed;
                    cs.changed.sort();
                    cs.changed.dedup();
                    cs.removed.sort();
                    cs.removed.dedup();
                }
                Ok(cs)
            });
        if r.is_err() {
            self.full_scan_next.store(true, Ordering::Relaxed);
        } else if scope.is_none() {
            self.full_scan_next.store(false, Ordering::Relaxed);
        }
        r
    }

    /// Also returns the sessions whose superseded flag changed.
    fn sync_locked(
        &self,
        p: Option<&dyn Fn(&IndexStatus)>,
        keep_cache: bool,
        scope: Option<&BTreeSet<String>>,
    ) -> CoreResult<(ChangeSet, BTreeSet<String>)> {
        let p: &dyn Fn(&IndexStatus) = p.unwrap_or(&|_| {});
        // Sessions linked by `continued-in` are synced together: agent files move between them.
        let scope = match scope {
            Some(s) => Some(self.index.read(|c| reader::with_continued(c, s))?),
            None => None,
        };
        let scope = scope.as_ref();
        let (mut scan, db_files) = match scope {
            Some(s) => (
                scan::scan_sessions(&self.cfg.data_root, s),
                self.index.read(|c| reader::files_of_sessions(c, s))?,
            ),
            None => (
                scan::scan_root(&self.cfg.data_root),
                self.index.read(reader::all_files)?,
            ),
        };
        let mut adopt = self.index.read(reader::adoptions)?;
        let mains: HashSet<&String> = scan
            .files
            .iter()
            .filter(|f| f.role == FileRole::Main)
            .map(|f| &f.session_id)
            .collect();
        adopt.retain(|_, to| mains.contains(to));
        scan::adopt(&mut scan.files, &mut scan.sidecars, &adopt);
        let (sidecar_dirty, sidecar_now) = {
            let prev = self.sidecar_state.lock();
            match (scope, prev.as_ref()) {
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
        let (plans, removed, moved) = plan_sync(&scan, db_files, &sidecar_dirty);
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
                let was_cached: HashSet<String> = jobs
                    .iter()
                    .filter(|(_, c)| c.is_some())
                    .map(|(p, _)| p.sid.clone())
                    .collect();
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
            match (scope, state.as_mut()) {
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
        Ok((cs, flipped.into_iter().collect()))
    }
}
