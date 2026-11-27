//! Read-only haa ownership lookup (`<haa_home>/usage.jsonl` and `<haa_home>/workers.json`).
//!
//! Maps agy conversation ids (`sessionId`) to their owning Main Session, Worker name, and Task
//! title. Cached by `(haa_home, file mtime + size)` so repeated `list_sessions` / `get_session`
//! calls avoid re-reading unchanged files. Missing or malformed files are treated as empty.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

use crate::model::HaaWorker;

type FileStamp = Option<(SystemTime, u64)>;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct UsageRow {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    worker: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    repo: Option<String>,
}

#[derive(Deserialize, Default)]
struct WorkersFile {
    #[serde(default)]
    workers: Vec<WorkerEntry>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct WorkerEntry {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
}

/// Cached view of `<haa_home>/usage.jsonl` and `<haa_home>/workers.json`, keyed by path and file
/// `(mtime, size)`.
#[derive(Debug, Default)]
pub struct HaaCache {
    haa_home: Option<PathBuf>,
    usage_stamp: FileStamp,
    workers_stamp: FileStamp,
    entries: HashMap<String, HaaWorker>,
}

impl HaaCache {
    /// Returns the cached `session_id -> HaaWorker` map for `haa_home`, reloading whenever the
    /// directory changes or either source file's mtime/size changes.
    pub fn entries(&mut self, haa_home: &Path) -> &HashMap<String, HaaWorker> {
        let usage_path = haa_home.join("usage.jsonl");
        let workers_path = haa_home.join("workers.json");
        let usage_stamp = file_stamp(&usage_path);
        let workers_stamp = file_stamp(&workers_path);

        if self.haa_home.as_deref() == Some(haa_home)
            && self.usage_stamp == usage_stamp
            && self.workers_stamp == workers_stamp
        {
            return &self.entries;
        }

        self.entries = load_files(&usage_path, &workers_path);
        self.haa_home = Some(haa_home.to_path_buf());
        self.usage_stamp = usage_stamp;
        self.workers_stamp = workers_stamp;
        &self.entries
    }
}

/// Reads `<haa_home>/usage.jsonl` and `<haa_home>/workers.json` without caching.
pub fn load(haa_home: &Path) -> HashMap<String, HaaWorker> {
    load_files(
        &haa_home.join("usage.jsonl"),
        &haa_home.join("workers.json"),
    )
}

/// `worktree directory -> repo root` from haa's `kind = "worktree"` records in `<haa_home>/usage.jsonl`
/// (written when `--worktree` creates one; old ones backfilled). haa deletes a worktree once merged, so
/// this is what puts an agy Session that ran in one back under its repo. Read on every sync: the file is
/// small. Missing or malformed lines are skipped; the last record for a directory wins.
pub fn worktree_repos(haa_home: &Path) -> HashMap<PathBuf, PathBuf> {
    let mut out = HashMap::new();
    let Ok(file) = File::open(haa_home.join("usage.jsonl")) else {
        return out;
    };
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else {
            break;
        };
        let Ok(row) = serde_json::from_str::<UsageRow>(&line) else {
            continue;
        };
        if row.kind.as_deref() != Some("worktree") {
            continue;
        }
        if let (Some(cwd), Some(repo)) = (non_empty(row.cwd), non_empty(row.repo)) {
            out.insert(PathBuf::from(cwd), PathBuf::from(repo));
        }
    }
    out
}

fn file_stamp(path: &Path) -> FileStamp {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    Some((mtime, meta.len()))
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.is_empty())
}

fn load_files(usage_path: &Path, workers_path: &Path) -> HashMap<String, HaaWorker> {
    let mut out = HashMap::new();
    read_usage(usage_path, &mut out);
    read_workers(workers_path, &mut out);
    out
}

fn read_usage(path: &Path, out: &mut HashMap<String, HaaWorker>) {
    let Ok(file) = File::open(path) else {
        return;
    };
    let reader = BufReader::new(file);
    for line in reader.lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(row) = serde_json::from_str::<UsageRow>(&line) else {
            continue;
        };
        if row.kind.as_deref() != Some("task") {
            continue;
        }
        let Some(sid) = non_empty(row.session_id) else {
            continue;
        };
        // Last task record wins for a given sessionId.
        out.insert(
            sid,
            HaaWorker {
                owner_session_id: non_empty(row.owner),
                name: non_empty(row.worker),
                task_title: non_empty(row.title),
            },
        );
    }
}

fn read_workers(path: &Path, out: &mut HashMap<String, HaaWorker>) {
    let Ok(file) = File::open(path) else {
        return;
    };
    let Ok(parsed) = serde_json::from_reader::<_, WorkersFile>(BufReader::new(file)) else {
        return;
    };
    for w in parsed.workers {
        let Some(sid) = non_empty(w.session_id) else {
            continue;
        };
        let entry = out.entry(sid).or_default();
        if entry.owner_session_id.is_none() {
            entry.owner_session_id = non_empty(w.owner);
        }
        if entry.name.is_none() {
            entry.name = non_empty(w.name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

    #[test]
    fn parses_usage_last_task_and_merges_workers() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("usage.jsonl"),
            concat!(
                "not json at all\n",
                r#"{"ts":"2026-11-27T01:00:00Z","kind":"spawn","owner":"m1","worker":"w0","sessionId":"s1"}"#,
                "\n",
                r#"{"ts":"2026-11-27T01:01:00Z","kind":"task","owner":"m1","worker":"w1","title":"first","sessionId":"s1"}"#,
                "\n",
                r#"{"ts":"2026-11-27T01:02:00Z","kind":"task","owner":"m1","worker":"w1","title":"second","sessionId":"s1"}"#,
                "\n",
                r#"{"ts":"2026-11-27T01:03:00Z","kind":"task","owner":"m1","worker":"legacy","title":"no sid"}"#,
                "\n",
            ),
        )
        .unwrap();
        fs::write(
            dir.path().join("workers.json"),
            r#"{"seq":3,"workers":[
                {"id":"u1","name":"w1","owner":"m1","sessionId":"s1"},
                {"id":"u2","name":"active-only","owner":"m2","sessionId":"s2"}
            ]}"#,
        )
        .unwrap();

        let map = load(dir.path());
        assert_eq!(map.len(), 2);
        assert_eq!(
            map["s1"],
            HaaWorker {
                owner_session_id: Some("m1".into()),
                name: Some("w1".into()),
                task_title: Some("second".into()),
            }
        );
        assert_eq!(
            map["s2"],
            HaaWorker {
                owner_session_id: Some("m2".into()),
                name: Some("active-only".into()),
                task_title: None,
            }
        );
    }

    #[test]
    fn worktree_repos_reads_worktree_records_last_one_wins() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("usage.jsonl"),
            [
                r#"{"kind":"worktree","worker":"w1","cwd":"/h/worktrees/haa-w1-aaaaaa","repo":"/r/old","branch":"haa/w1-aaaaaa"}"#,
                "not json",
                r#"{"kind":"task","worker":"w1","cwd":"/h/worktrees/haa-w2-bbbbbb","repo":"/r/ignored"}"#,
                r#"{"kind":"worktree","worker":"w3","cwd":"/h/worktrees/haa-w3-cccccc"}"#,
                r#"{"kind":"worktree","worker":"w1","cwd":"/h/worktrees/haa-w1-aaaaaa","repo":"/r/new","backfilled":"branch"}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let repos = worktree_repos(dir.path());
        assert_eq!(
            repos,
            HashMap::from([(
                PathBuf::from("/h/worktrees/haa-w1-aaaaaa"),
                PathBuf::from("/r/new")
            )])
        );
        assert!(worktree_repos(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn cache_tracks_path_and_mtime_and_tolerates_corrupt_files() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let usage_a = dir_a.path().join("usage.jsonl");
        fs::write(
            &usage_a,
            r#"{"kind":"task","owner":"m1","worker":"w1","title":"t1","sessionId":"s1"}"#,
        )
        .unwrap();
        // Corrupt workers.json is treated as empty without failing.
        fs::write(dir_a.path().join("workers.json"), "{not valid json").unwrap();

        let mut cache = HaaCache::default();
        assert_eq!(
            cache.entries(dir_a.path())["s1"].task_title.as_deref(),
            Some("t1")
        );

        // Update file with a bumped mtime -> cache invalidates and reloads.
        let future = SystemTime::now() + Duration::from_secs(5);
        fs::write(
            &usage_a,
            r#"{"kind":"task","owner":"m1","worker":"w1","title":"t2","sessionId":"s1"}"#,
        )
        .unwrap();
        File::options()
            .write(true)
            .open(&usage_a)
            .unwrap()
            .set_modified(future)
            .unwrap();
        assert_eq!(
            cache.entries(dir_a.path())["s1"].task_title.as_deref(),
            Some("t2")
        );

        // Switching to another directory invalidates even when empty/missing.
        assert!(cache.entries(dir_b.path()).is_empty());
    }
}
