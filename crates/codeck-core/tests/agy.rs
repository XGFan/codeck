//! Engine integration tests for agy (Antigravity CLI) history loading, scanning, index meta
//! validation, and read-only fallback.
mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use codeck_core::index::schema::{ROLE_MAIN, SCHEMA_VERSION, SOURCE_AGY, SOURCE_CLAUDE};
use codeck_core::model::{
    DataRootSource, NodeBody, SessionQuery, TranscriptRequest, TranscriptScope,
};
use codeck_core::{CoreError, Engine, EngineConfig};
use common::engine::{open_engine_with_agy, test_haa_home};
use common::{fixture_root, ids};
use rusqlite::Connection;

const AGY_TUI: &str = "8c54e127-d8c4-55e2-9034-566dac2ca233";
const AGY_HAA_WORKER: &str = "c23ef150-997c-55e8-8dc2-3debd3f67351";
const AGY_SHORT: &str = "830e3bc5-0957-5b0b-a5dd-d340b130039e";

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

fn read_only(claude_root: &Path, agy_root: &Path, cache: &Path) -> Result<Engine, CoreError> {
    Engine::open_read_only(EngineConfig {
        data_root: claude_root.to_path_buf(),
        data_root_source: DataRootSource::Settings,
        agy_root: agy_root.to_path_buf(),
        haa_home: test_haa_home(cache),
        cache_dir: cache.to_path_buf(),
    })
}

fn main_transcript_req(session_id: &str) -> TranscriptRequest {
    TranscriptRequest {
        session_id: session_id.to_owned(),
        scope: TranscriptScope::Main,
        branch_choices: Vec::new(),
        include_hidden: false,
    }
}

#[test]
fn indexes_claude_and_agy_roots_together_and_opens_after_reopen() {
    let claude = fixture_root("basic");
    let agy = fixture_root("agy");
    let cache = tempfile::tempdir().unwrap();

    // 1. Scan both roots into one index.
    {
        let engine = open_engine_with_agy(claude.path(), agy.path(), cache.path());
        engine.scan_all(&|_| {}).unwrap();

        let listed: BTreeSet<String> = engine
            .list_sessions(&SessionQuery::default())
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(
            listed,
            BTreeSet::from([
                ids::BASIC.to_owned(),
                AGY_TUI.to_owned(),
                AGY_HAA_WORKER.to_owned(),
                AGY_SHORT.to_owned(),
            ])
        );

        let db = Connection::open(cache.path().join("index.sqlite")).unwrap();
        let version: u32 = db
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        let file_sources: BTreeMap<String, (String, i64)> = db
            .prepare("SELECT session_id, source, role FROM files")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            file_sources.get(ids::BASIC),
            Some(&(SOURCE_CLAUDE.to_owned(), ROLE_MAIN))
        );
        for sid in [AGY_TUI, AGY_HAA_WORKER, AGY_SHORT] {
            assert_eq!(
                file_sources.get(sid),
                Some(&(SOURCE_AGY.to_owned(), ROLE_MAIN))
            );
        }

        // New columns exist on `sessions` with their defaults.
        let (agent, haa_worker, agy_meta): (String, i64, Option<i64>) = db
            .query_row(
                "SELECT agent, haa_worker, agy_meta FROM sessions WHERE id=?1",
                [AGY_SHORT],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(agent, SOURCE_CLAUDE);
        assert_eq!(haa_worker, 0);
        assert_eq!(agy_meta, None);
    }

    // Plant a decoy `<parent>/<sid>/subagents/agent-decoy.jsonl` beside an agy transcript to
    // verify `session_ctx` never scans Claude-style sidecar/subagent directories for agy sessions.
    let decoy_dir = agy
        .path()
        .join("brain")
        .join(AGY_SHORT)
        .join(".system_generated/logs")
        .join(AGY_SHORT)
        .join("subagents");
    fs::create_dir_all(&decoy_dir).unwrap();
    fs::write(decoy_dir.join("agent-decoy.jsonl"), "{}\n").unwrap();

    // 2. Reopen Engine without rescanning: index stays intact and agy transcripts open cleanly.
    let engine = open_engine_with_agy(claude.path(), agy.path(), cache.path());
    let db = Connection::open(cache.path().join("index.sqlite")).unwrap();
    let files_count: i64 = db
        .query_row("SELECT count(*) FROM files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(files_count, 4, "reopen with same roots must not rebuild");

    let tui = engine
        .get_transcript(&main_transcript_req(AGY_TUI))
        .unwrap();
    assert!(
        tui.nodes
            .iter()
            .any(|n| matches!(n.body, NodeBody::UserPrompt { .. }))
    );
    assert!(
        tui.nodes
            .iter()
            .any(|n| matches!(n.body, NodeBody::Assistant { .. }))
    );

    let short = engine
        .get_transcript(&main_transcript_req(AGY_SHORT))
        .unwrap();
    assert!(!short.nodes.is_empty());
    assert!(short.subagents.is_empty());
    assert!(short.orphan_subagent_ids.is_empty());

    // Overwrite step_index 0 on disk while the session is cached: agy always re-reads in full.
    let short_path = transcript::agy::transcript_path(agy.path(), AGY_SHORT);
    let mut f = OpenOptions::new().append(true).open(&short_path).unwrap();
    writeln!(
        f,
        r#"{{"step_index":0,"type":"USER_INPUT","created_at":"2026-11-27T00:00:00Z","content":"<USER_REQUEST>updated agy prompt</USER_REQUEST>"}}"#
    )
    .unwrap();
    drop(f);

    let updated = engine
        .get_transcript(&main_transcript_req(AGY_SHORT))
        .unwrap();
    let first_user_text = updated
        .nodes
        .iter()
        .find_map(|n| match &n.body {
            NodeBody::UserPrompt { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .expect("user node");
    assert_eq!(first_user_text, "updated agy prompt");
}

#[test]
fn read_only_engine_opens_unindexed_agy_session_from_disk() {
    let claude = fixture_root("basic");
    let agy = fixture_root("agy");
    let cache = tempfile::tempdir().unwrap();

    {
        let engine = open_engine_with_agy(claude.path(), agy.path(), cache.path());
        engine.scan_all(&|_| {}).unwrap();
    }

    // Add a brand-new agy session after indexing closed.
    let new_id = "aaaaaaaa-bbbb-5ccc-8ddd-eeeeeeeeeeee";
    let src = transcript::agy::transcript_path(agy.path(), AGY_SHORT);
    let dst = transcript::agy::transcript_path(agy.path(), new_id);
    fs::create_dir_all(dst.parent().unwrap()).unwrap();
    fs::copy(&src, &dst).unwrap();

    let before = snapshot(cache.path());
    let ro = read_only(claude.path(), agy.path(), cache.path()).unwrap();
    assert!(matches!(
        ro.get_session(new_id),
        Err(CoreError::NotFound(_))
    ));
    let t = ro.get_transcript(&main_transcript_req(new_id)).unwrap();
    assert!(
        t.nodes
            .iter()
            .any(|n| matches!(n.body, NodeBody::UserPrompt { .. }))
    );
    assert!(
        t.nodes
            .iter()
            .any(|n| matches!(n.body, NodeBody::Assistant { .. }))
    );
    assert_eq!(snapshot(cache.path()), before);
}

#[test]
fn agy_root_mismatch_rebuilds_on_writable_open_and_preserves_roots_on_rebuild() {
    let claude = fixture_root("basic");
    let agy = fixture_root("agy");
    let cache = tempfile::tempdir().unwrap();
    let db_path = cache.path().join("index.sqlite");

    {
        let engine = open_engine_with_agy(claude.path(), agy.path(), cache.path());
        engine.scan_all(&|_| {}).unwrap();
        // Explicit `rebuild` keeps both roots in `meta`.
        engine.rebuild(&|_| {}).unwrap();
        let db = Connection::open(&db_path).unwrap();
        let stored_claude: String = db
            .query_row("SELECT value FROM meta WHERE key='data_root'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let stored_agy: String = db
            .query_row("SELECT value FROM meta WHERE key='agy_root'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(stored_claude, claude.path().display().to_string());
        assert_eq!(stored_agy, agy.path().display().to_string());
    }

    // Writable open with a different (even absent) agy_root deletes and recreates the index.
    let other_agy = cache.path().join("other-agy-missing");
    let engine = open_engine_with_agy(claude.path(), &other_agy, cache.path());
    {
        let db = Connection::open(&db_path).unwrap();
        let files: i64 = db
            .query_row("SELECT count(*) FROM files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(files, 0, "agy_root mismatch must recreate the index");
        let stored_claude: String = db
            .query_row("SELECT value FROM meta WHERE key='data_root'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let stored_agy: String = db
            .query_row("SELECT value FROM meta WHERE key='agy_root'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(stored_claude, claude.path().display().to_string());
        assert_eq!(stored_agy, other_agy.display().to_string());
    }

    engine.scan_all(&|_| {}).unwrap();
    let listed: Vec<String> = engine
        .list_sessions(&SessionQuery::default())
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(listed, vec![ids::BASIC.to_owned()]);
}

#[test]
fn read_only_open_refuses_mismatched_agy_root_without_touching_index() {
    let claude = fixture_root("basic");
    let agy = fixture_root("agy");
    let cache = tempfile::tempdir().unwrap();

    {
        let engine = open_engine_with_agy(claude.path(), agy.path(), cache.path());
        engine.scan_all(&|_| {}).unwrap();
    }

    let before = snapshot(cache.path());
    let err = read_only(
        claude.path(),
        Path::new("/some/other/agy-root"),
        cache.path(),
    );
    match err {
        Err(CoreError::IndexUnavailable(msg)) => {
            assert_eq!(msg, "索引对应的数据目录与当前不一致");
        }
        other => panic!("expected IndexUnavailable, got {other:?}"),
    }
    assert_eq!(snapshot(cache.path()), before);
}
