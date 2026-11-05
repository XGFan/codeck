//! Swapping the data root: the app state holds the previous root's Engine until the new one is in
//! place, so its index connections close after the new Engine has recreated the index at the same
//! path. SQLite notices its database file was replaced and leaves the new `-wal` / `-shm` alone.
mod common;

use codeck_core::model::SessionQuery;
use common::engine::open_engine;
use common::fixture_root;

#[test]
fn the_previous_roots_engine_closing_late_leaves_the_new_index_intact() {
    let (a, b) = (fixture_root("basic"), fixture_root("branch"));
    let cache = tempfile::tempdir().unwrap();
    let old = open_engine(a.path(), cache.path());
    old.scan_all(&|_| {}).unwrap();
    let new = open_engine(b.path(), cache.path());
    new.scan_all(&|_| {}).unwrap();
    let count = |e: &codeck_core::Engine| e.list_sessions(&SessionQuery::default()).unwrap().len();
    let before = count(&new);
    assert!(before > 0);
    drop(old);
    // A connection opened now (as the read pool does) sees everything the new Engine wrote.
    let db = codeck_core::paths::index_db_path(cache.path());
    let fresh = rusqlite::Connection::open(&db).unwrap();
    let rows: i64 = fresh
        .query_row(
            "SELECT count(*) FROM sessions WHERE is_empty = 0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows as usize, before);
    assert_eq!(count(&new), before);
}
