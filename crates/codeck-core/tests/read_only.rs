//! `Index::open_read_only` / `Engine::open_read_only`: the desktop's index is never rebuilt or
//! changed, whatever the reader finds in it.
mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use codeck_core::model::{DataRootSource, SessionQuery, TranscriptRequest, TranscriptScope};
use codeck_core::{CoreError, Engine, EngineConfig};
use common::engine::Env;
use common::{fixture_root, ids};
use rusqlite::Connection;

/// Every file in `dir`, by name.
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().to_string_lossy().into_owned(),
                fs::read(e.path()).unwrap(),
            )
        })
        .collect()
}

fn read_only(root: &Path, cache: &Path) -> Result<Engine, CoreError> {
    Engine::open_read_only(EngineConfig {
        data_root: root.to_path_buf(),
        data_root_source: DataRootSource::Settings,
        cache_dir: cache.to_path_buf(),
    })
}

/// An index for the `basic` fixture, with no connection left open.
fn closed_index() -> Env {
    let env = Env::scanned(fixture_root("basic"));
    let Env {
        root,
        cache,
        engine,
    } = env;
    drop(engine);
    let engine = read_only(root.path(), cache.path()).unwrap();
    Env {
        root,
        cache,
        engine,
    }
}

fn assert_refused(err: Result<Engine, CoreError>, message: &str) {
    match err {
        Err(CoreError::IndexUnavailable(m)) => assert_eq!(m, message),
        other => panic!("{other:?}"),
    }
}

#[test]
fn another_schema_version_is_an_error_and_changes_nothing() {
    let env = closed_index();
    let conn = Connection::open(env.cache.path().join("index.sqlite")).unwrap();
    conn.pragma_update(None, "user_version", 1).unwrap();
    drop(conn);
    let before = snapshot(env.cache.path());
    assert_refused(
        read_only(env.root.path(), env.cache.path()),
        "索引版本与 CLI 不一致，请升级 CoDeck",
    );
    assert_eq!(snapshot(env.cache.path()), before);
}

#[test]
fn another_data_root_is_an_error_and_changes_nothing() {
    let env = closed_index();
    let before = snapshot(env.cache.path());
    assert_refused(
        read_only(Path::new("/some/other/root"), env.cache.path()),
        "索引对应的数据目录与当前不一致",
    );
    assert_eq!(snapshot(env.cache.path()), before);
}

#[test]
fn a_missing_index_is_an_error_and_creates_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = tmp.path().join("no-such-cache");
    assert_refused(read_only(tmp.path(), &cache), "索引不存在");
    assert!(!cache.exists());
}

#[test]
fn reading_a_closed_index_creates_and_changes_nothing() {
    let env = closed_index();
    let before = snapshot(env.cache.path());
    assert_eq!(before.len(), 1, "only the database: {:?}", before.keys());
    let engine = read_only(env.root.path(), env.cache.path()).unwrap();
    let sessions = engine.list_sessions(&SessionQuery::default()).unwrap();
    assert_eq!(sessions.len(), 1);
    assert!(engine.indexed_at().is_some());
    assert_eq!(snapshot(env.cache.path()), before);
}

#[test]
fn reads_beside_a_running_writer_without_touching_its_files() {
    let env = Env::scanned(fixture_root("basic"));
    let cache = env.cache.path();
    let committed = |dir: &Path| {
        let mut s = snapshot(dir);
        // Readers mark the wal-index as they go; it is not persisted data.
        s.remove("index.sqlite-shm");
        s
    };
    assert!(cache.join("index.sqlite-wal").exists());
    let before = committed(cache);
    let engine = read_only(env.root.path(), cache).unwrap();
    assert_eq!(
        engine
            .list_sessions(&SessionQuery::default())
            .unwrap()
            .len(),
        1
    );
    assert_eq!(committed(cache), before);
}

#[test]
fn a_session_the_index_does_not_know_is_read_from_its_file() {
    let env = closed_index();
    let new_id = "11111111-2222-4333-8444-555555555555";
    let dir = env.root.path().join("projects").join(ids::BASIC_DIR);
    fs::copy(
        dir.join(format!("{}.jsonl", ids::BASIC)),
        dir.join(format!("{new_id}.jsonl")),
    )
    .unwrap();
    assert!(matches!(
        env.engine.get_session(new_id),
        Err(CoreError::NotFound(_))
    ));
    let t = env
        .engine
        .get_transcript(&TranscriptRequest {
            session_id: new_id.to_owned(),
            scope: TranscriptScope::Main,
            branch_choices: Vec::new(),
            include_hidden: false,
        })
        .unwrap();
    assert!(!t.nodes.is_empty());
}
