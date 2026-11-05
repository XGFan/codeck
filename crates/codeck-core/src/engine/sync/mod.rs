//! Index sync (phase 1): plan from the scan and the `files` rows, compute sessions in
//! parallel, write them batch by batch, then re-bill.

mod compute;
mod plan;
mod write;

use std::collections::HashSet;

use rayon::prelude::*;

use super::session_cache::CachedSession;
use super::{ChangeSet, Engine};
use crate::error::CoreResult;
use crate::index::{reader, writer};
use crate::model::{IndexPhase, IndexStatus};
use crate::{project, scan};
use compute::{SessionWork, compute_session};
use plan::{SessionPlan, batches, diff_sidecars, plan_sync};
use write::{rebill_changed, session_exists, write_work};

impl Engine {
    pub(super) fn sync(
        &self,
        p: Option<&dyn Fn(&IndexStatus)>,
        keep_cache: bool,
    ) -> CoreResult<ChangeSet> {
        let _g = self.sync_lock.lock();
        let p: &dyn Fn(&IndexStatus) = p.unwrap_or(&|_| {});
        let scan = scan::scan_root(&self.cfg.data_root);
        let db_files = self.index.read(reader::all_files)?;
        let (sidecar_dirty, sidecar_now) =
            diff_sidecars(self.sidecar_state.lock().as_ref(), &scan, &db_files);
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
        // Session rows are committed batch by batch, billing only at the end: a sync that stops in
        // between leaves the flag set, and the next one re-bills everything.
        let mut rebill_all = false;
        let result = (|| -> CoreResult<()> {
            self.index.write(|conn| {
                let tx = conn.transaction()?;
                rebill_all = writer::rebill_pending(&tx)?;
                if !removed.is_empty() || !plans.is_empty() {
                    writer::set_rebill_pending(&tx, true)?;
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
        *self.sidecar_state.lock() = Some(sidecar_now);
        cs.changed.sort();
        cs.changed.dedup();
        cs.removed.sort();
        Ok(cs)
    }
}
