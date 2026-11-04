//! Live Sessions (T2.1): process files with pid + procStart checks, and the recent-write fallback.
mod common;

use std::fs::{self, File};
use std::path::Path;
use std::time::{Duration, SystemTime};

use common::{fixture_root, fixture_root_with_pids, ids, proc_start};
use codeck_core::model::{DataRootSource, LiveChanged, LiveSource, LiveState, LiveStatus};
use codeck_core::{Engine, EngineConfig};
use tempfile::TempDir;

fn engine(root: &Path, cache: &TempDir) -> Engine {
    let e = Engine::open(EngineConfig {
        data_root: root.to_path_buf(),
        data_root_source: DataRootSource::Settings,
        cache_dir: cache.path().to_path_buf(),
    })
    .unwrap();
    e.scan_all(&|_| {}).unwrap();
    e
}

fn state<'a>(live: &'a LiveChanged, sid: &str) -> Option<&'a LiveState> {
    live.live
        .iter()
        .find(|e| e.session_id == sid)
        .map(|e| &e.state)
}

fn set_mtime(path: &Path, age: Duration) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - age)
        .unwrap();
}

/// Ages every main file so only process files (or explicitly touched files) make a session live.
fn age_all(root: &Path) {
    for dir in fs::read_dir(root.join("projects")).unwrap() {
        for f in fs::read_dir(dir.unwrap().path()).unwrap() {
            let p = f.unwrap().path();
            if p.extension().is_some_and(|x| x == "jsonl") {
                set_mtime(&p, Duration::from_secs(3600));
            }
        }
    }
}

#[test]
fn alive_pid_with_matching_proc_start_is_live() {
    let (root, pids) = fixture_root_with_pids("live");
    let pids = pids.unwrap();
    age_all(root.path());
    let cache = tempfile::tempdir().unwrap();
    let e = engine(root.path(), &cache);

    let live = e
        .poll_live()
        .unwrap()
        .expect("first poll reports the live set");
    let busy = state(&live, ids::LIVE_BUSY).expect("busy session is live");
    assert_eq!(busy.status, LiveStatus::Busy);
    assert_eq!(busy.source, LiveSource::ProcessFile);
    assert_eq!(busy.pid, Some(pids.pid));
    assert_eq!(busy.raw_status.as_deref(), Some("busy"));

    let shell = state(&live, ids::LIVE_SHELL).expect("shell session is live");
    assert_eq!(shell.status, LiveStatus::Idle);
    assert_eq!(shell.raw_status.as_deref(), Some("shell"));

    assert!(state(&live, ids::LIVE_DEAD).is_none(), "exited pid");
    assert_eq!(e.poll_live().unwrap(), None, "unchanged set: no event");
}

#[test]
fn stale_proc_start_is_not_live() {
    let (root, pids) = fixture_root_with_pids("live");
    let pids = pids.unwrap();
    age_all(root.path());
    let cache = tempfile::tempdir().unwrap();
    let e = engine(root.path(), &cache);

    // pid 1 exists but started at another time than its file says (pid reuse).
    let live = e.poll_live().unwrap().unwrap();
    assert!(state(&live, ids::LIVE_REUSED).is_none());

    // Our own pid with a wrong procStart: the busy session is no longer live, and the
    // recent-write fallback does not apply because a process file names it.
    let pf = root.path().join(format!("sessions/{}.json", pids.pid));
    let text = fs::read_to_string(&pf)
        .unwrap()
        .replace(&proc_start(pids.pid), "Mon Jan  1 00:00:00 2024");
    fs::write(&pf, text).unwrap();
    let main = root
        .path()
        .join("projects")
        .join(ids::LIVE_DIR)
        .join(format!("{}.jsonl", ids::LIVE_BUSY));
    set_mtime(&main, Duration::ZERO);
    e.apply_changes(std::slice::from_ref(&main)).unwrap();
    let live = e.poll_live().unwrap().expect("live set changed");
    assert!(state(&live, ids::LIVE_BUSY).is_none());
    assert!(state(&live, ids::LIVE_SHELL).is_some());
}

#[test]
fn recent_write_fallback() {
    let root = fixture_root("basic");
    age_all(root.path());
    let main = root
        .path()
        .join("projects")
        .join(ids::BASIC_DIR)
        .join(format!("{}.jsonl", ids::BASIC));
    set_mtime(&main, Duration::from_secs(30));
    let cache = tempfile::tempdir().unwrap();
    let e = engine(root.path(), &cache);

    let live = e
        .poll_live()
        .unwrap()
        .expect("recently written session is live");
    let s = state(&live, ids::BASIC).unwrap();
    assert_eq!(s.source, LiveSource::RecentWrite);
    assert_eq!(s.status, LiveStatus::Idle);
    assert_eq!(s.pid, None);

    set_mtime(&main, Duration::from_secs(10 * 60));
    e.apply_changes(std::slice::from_ref(&main)).unwrap();
    let live = e.poll_live().unwrap().expect("fallback expired");
    assert!(live.live.is_empty());
}
