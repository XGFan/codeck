//! Sync planning: pure functions from the scan, the `files` rows and the previous sidecar
//! state to per-session plans and batches.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::engine::SidecarState;
use crate::engine::session_cache::read_head;
use crate::index::reader::FileRecord;
use crate::model::FileRole;
use crate::scan::{ScanResult, ScannedFile, SidecarFile};

/// Bytes parsed per write transaction during a sync.
const BATCH_BYTES: u64 = 256 << 20;

// -------------------------------------------------------------------- sync planning

/// A file whose rows must be (re)read from `from`.
pub(super) struct Ingest {
    pub(super) file: ScannedFile,
    pub(super) from: u64,
    /// The current row (append); `None` for a new file or a reparse.
    pub(super) existing: Option<FileRecord>,
}

/// Work for one dirty session.
pub(super) struct SessionPlan {
    pub(super) sid: String,
    /// Delete every row of the session first and re-ingest all its files from 0.
    pub(super) reparse: bool,
    pub(super) ingest: Vec<Ingest>,
    /// (file id, size, mtime) for files whose stat changed without new complete lines.
    pub(super) stat_only: Vec<(i64, u64, i64)>,
    /// Recompute the session row (false when only stat columns change).
    pub(super) recompute: bool,
    /// Every current file of the session.
    pub(super) current: Vec<ScannedFile>,
    pub(super) sidecars: Vec<SidecarFile>,
}

impl SessionPlan {
    pub(super) fn ingest_bytes(&self) -> u64 {
        self.ingest
            .iter()
            .map(|i| i.file.size.saturating_sub(i.from))
            .sum()
    }

    /// Rough parse cost: ingested bytes plus main files that may need a full load.
    pub(super) fn cost(&self) -> u64 {
        let main: u64 = self
            .current
            .iter()
            .filter(|f| f.role == FileRole::Main)
            .map(|f| f.size)
            .sum();
        self.ingest_bytes() + if self.recompute { main } else { 0 }
    }
}

pub(super) enum FileAction {
    Unchanged,
    StatOnly,
    Append,
    Reparse,
}

pub(super) fn classify(f: &ScannedFile, row: &FileRecord) -> FileAction {
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

/// Diffs the scan against the `files` rows: dirty session plans (sorted by id), the sessions
/// whose last file disappeared, and the rows of files that now belong to another session (an
/// agent file adopted from a superseded session, or given back; see [`crate::scan::adopt`]): the
/// caller deletes those rows, the new owner ingests the file afresh and the old one is re-parsed.
pub(super) fn plan_sync(
    scan: &ScanResult,
    db: Vec<FileRecord>,
    sidecar_dirty: &HashSet<String>,
) -> (Vec<SessionPlan>, Vec<String>, Vec<FileRecord>) {
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
    let mut moved = Vec::new();
    for f in &scan.files {
        let key = f.path.to_string_lossy().into_owned();
        let a = acc.entry(f.session_id.clone()).or_default();
        match db_by_path.remove(&key) {
            Some(row) if row.session_id != f.session_id => {
                a.recompute = true;
                a.ingest.push(Ingest {
                    file: f.clone(),
                    from: 0,
                    existing: None,
                });
                moved.push(row);
            }
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
    // Rows whose file is gone or moved: reparse the session if other files survive, else remove it.
    let mut removed = Vec::new();
    for row in db_by_path.into_values().chain(moved.iter().cloned()) {
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
    (plans, removed, moved)
}

/// Splits plans into batches of about [`BATCH_BYTES`] parse cost.
pub(super) fn batches(plans: Vec<SessionPlan>) -> Vec<Vec<SessionPlan>> {
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

/// Sessions whose sidecar files changed since the previous sync, and the sidecar state
/// to commit once this sync succeeds. Before the first sync, a sidecar newer than every
/// indexed file of its session counts as changed (it was written while the app was closed).
pub(super) fn diff_sidecars(
    prev: Option<&SidecarState>,
    scan: &ScanResult,
    db: &[FileRecord],
) -> (HashSet<String>, SidecarState) {
    let now: SidecarState = scan
        .sidecars
        .iter()
        .map(|s| (s.path.clone(), (s.size, s.mtime_ns, s.session_id.clone())))
        .collect();
    let mut dirty = HashSet::new();
    if let Some(prev) = prev {
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::scan::SidecarKind;

    fn file(sid: &str, name: &str, size: u64) -> ScannedFile {
        ScannedFile {
            path: PathBuf::from(format!("/p/{name}")),
            dev: 1,
            ino: size,
            size,
            mtime_ns: 10,
            role: FileRole::Main,
            session_id: sid.into(),
            agent_id: None,
            workflow_run_id: None,
        }
    }

    fn row(f: &ScannedFile, id: i64) -> FileRecord {
        FileRecord {
            id,
            path: f.path.to_string_lossy().into_owned(),
            dev: f.dev,
            ino: f.ino,
            role: f.role,
            session_id: f.session_id.clone(),
            agent_id: None,
            size: f.size,
            mtime_ns: f.mtime_ns,
            head: Vec::new(),
            parsed_offset: f.size,
            text_offset: 0,
            line_count: 0,
            failed_lines: 0,
            first_error: None,
        }
    }

    fn sidecar(sid: &str, name: &str, mtime_ns: i64) -> SidecarFile {
        SidecarFile {
            path: PathBuf::from(format!("/p/{sid}/workflows/{name}.json")),
            session_id: sid.into(),
            kind: SidecarKind::WorkflowJson {
                run_id: name.into(),
            },
            size: 1,
            mtime_ns,
        }
    }

    #[test]
    fn plan_sync_classifies_new_replaced_and_vanished_files() {
        let new = file("a", "new.jsonl", 5);
        let same = file("b", "same.jsonl", 7);
        let replaced = file("c", "replaced.jsonl", 9);
        let survivor = file("d", "d1.jsonl", 3);
        let gone_d = file("d", "d2.jsonl", 4);
        let gone_e = file("e", "e.jsonl", 6);
        let sidecar_only = file("f", "f.jsonl", 8);
        let mut replaced_row = row(&replaced, 3);
        replaced_row.ino += 100;
        let scan = ScanResult {
            files: vec![
                new,
                same.clone(),
                replaced,
                survivor.clone(),
                sidecar_only.clone(),
            ],
            sidecars: Vec::new(),
        };
        let db = vec![
            row(&same, 2),
            replaced_row,
            row(&survivor, 4),
            row(&gone_d, 5),
            row(&gone_e, 6),
            row(&sidecar_only, 7),
        ];
        // `zz` has no files any more: a dirty sidecar alone does not bring it back.
        let dirty: HashSet<String> = ["f".to_owned(), "zz".to_owned()].into();
        let (plans, removed, moved) = plan_sync(&scan, db, &dirty);
        assert!(moved.is_empty());
        assert_eq!(removed, vec!["e".to_owned()]);
        let summary: Vec<(&str, bool, bool, Vec<u64>)> = plans
            .iter()
            .map(|p| {
                let from: Vec<u64> = p.ingest.iter().map(|i| i.from).collect();
                (p.sid.as_str(), p.reparse, p.recompute, from)
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("a", false, true, vec![0]),
                ("c", true, true, vec![0]),
                ("d", true, true, vec![0]),
                ("f", false, true, vec![]),
            ]
        );
    }

    #[test]
    fn batches_split_by_parse_cost_and_never_leave_one_empty() {
        let plan = |sid: &str, size: u64| SessionPlan {
            sid: sid.into(),
            reparse: false,
            ingest: vec![Ingest {
                file: file(sid, sid, size),
                from: 0,
                existing: None,
            }],
            stat_only: Vec::new(),
            recompute: false,
            current: Vec::new(),
            sidecars: Vec::new(),
        };
        let b = BATCH_BYTES / 10;
        let out = batches(vec![
            plan("a", 6 * b),
            plan("b", 3 * b),
            plan("c", 5 * b),
            plan("d", 20 * b),
        ]);
        let sids: Vec<Vec<&str>> = out
            .iter()
            .map(|batch| batch.iter().map(|p| p.sid.as_str()).collect())
            .collect();
        assert_eq!(sids, vec![vec!["a", "b"], vec!["c"], vec!["d"]]);
    }

    #[test]
    fn diff_sidecars_before_and_after_the_first_sync() {
        let mut db = vec![
            row(&file("a", "a.jsonl", 1), 1),
            row(&file("b", "b.jsonl", 2), 2),
        ];
        db[0].mtime_ns = 100;
        db[1].mtime_ns = 100;
        let scan = ScanResult {
            files: Vec::new(),
            sidecars: vec![
                sidecar("a", "x", 200),
                sidecar("b", "y", 50),
                sidecar("c", "z", 300),
            ],
        };
        // First sync: only a sidecar newer than every indexed file of its session counts.
        let (dirty, now) = diff_sidecars(None, &scan, &db);
        assert_eq!(dirty, ["a".to_owned()].into());
        assert_eq!(now.len(), 3);

        // Later syncs: any change, addition or removal since the previous state counts.
        let mut prev = now.clone();
        prev.get_mut(&scan.sidecars[1].path).unwrap().0 = 2;
        prev.remove(&scan.sidecars[2].path);
        prev.insert(PathBuf::from("/p/d/gone.json"), (1, 1, "d".into()));
        let (dirty, _) = diff_sidecars(Some(&prev), &scan, &db);
        assert_eq!(
            dirty,
            ["b".to_owned(), "c".to_owned(), "d".to_owned()].into()
        );
        let (dirty, _) = diff_sidecars(Some(&now), &scan, &db);
        assert!(dirty.is_empty());
    }
}
