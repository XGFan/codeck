//! Engine integration tests for agy (Antigravity CLI) history: scanning, loading, sync,
//! metadata fingerprinting, title/Project resolution, FTS exclusion vs ⌘F, pricing/stats
//! exclusion, per-agent diagnostics filtering, and Resume commands.
mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use codeck_core::index::schema::{
    DIAG_ENTRY, DIAG_TOOL, ROLE_MAIN, SCHEMA_VERSION, SOURCE_AGY, SOURCE_CLAUDE,
};
use codeck_core::model::{
    Agent, AssistantBlock, DataRootSource, Drill, FindRequest, NodeBody, SearchRequest,
    SessionQuery, StatsRequest, TimeRange, TitleSource, TranscriptRequest, TranscriptScope,
};
use codeck_core::{ChangeSet, CoreError, Engine, EngineConfig};
use common::engine::{open_engine, open_engine_with_agy, test_haa_home};
use common::{fixture_root, ids};
use rusqlite::Connection;
use serde_json::{Value, json};

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

        let listed = engine.list_sessions(&SessionQuery::default()).unwrap();
        let agents: BTreeMap<String, Agent> =
            listed.iter().map(|s| (s.id.clone(), s.agent)).collect();
        assert_eq!(
            agents,
            BTreeMap::from([
                (ids::BASIC.to_owned(), Agent::Claude),
                (AGY_TUI.to_owned(), Agent::Agy),
                (AGY_HAA_WORKER.to_owned(), Agent::Agy),
                (AGY_SHORT.to_owned(), Agent::Agy),
            ])
        );
        assert_eq!(
            serde_json::to_value(Agent::Claude).unwrap(),
            json!("claude")
        );
        assert_eq!(serde_json::to_value(Agent::Agy).unwrap(), json!("agy"));

        // Resume commands differ by agent.
        assert_eq!(
            engine.get_session(ids::BASIC).unwrap().resume_command,
            format!("claude --resume {}", ids::BASIC)
        );
        assert_eq!(
            engine.get_session(AGY_SHORT).unwrap().resume_command,
            format!("agy --conversation {}", AGY_SHORT)
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

        // `sessions` carries agent, haa_worker and the metadata fingerprint.
        let session_cols = |sid: &str| -> (String, i64, Option<i64>) {
            db.query_row(
                "SELECT agent, haa_worker, agy_meta FROM sessions WHERE id=?1",
                [sid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
        };
        let (agent, haa_worker, agy_meta) = session_cols(AGY_SHORT);
        assert_eq!(agent, SOURCE_AGY);
        assert_eq!(haa_worker, 0);
        assert!(agy_meta.is_some());

        let (agent, haa_worker, agy_meta) = session_cols(AGY_HAA_WORKER);
        assert_eq!(agent, SOURCE_AGY);
        assert_eq!(haa_worker, 1, "mcp_haa_* calls mark a haa Worker");
        assert!(agy_meta.is_some());

        let (agent, haa_worker, agy_meta) = session_cols(ids::BASIC);
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

    // Translated tools come out of the Engine with their results attached.
    let worker = engine
        .get_transcript(&main_transcript_req(AGY_HAA_WORKER))
        .unwrap();
    let calls: Vec<_> = worker
        .nodes
        .iter()
        .filter_map(|n| match &n.body {
            NodeBody::Assistant { blocks, .. } => Some(blocks),
            _ => None,
        })
        .flatten()
        .filter_map(|b| match b {
            AssistantBlock::ToolCall(c) => Some(c),
            _ => None,
        })
        .collect();
    for name in ["Bash", "Read", "Edit"] {
        assert!(
            calls.iter().any(|c| c.name == name && c.result.is_some()),
            "{name} call with its result"
        );
    }

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

fn read_meta_json(agy_root: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(transcript::agy::metadata_path(agy_root)).unwrap())
        .unwrap()
}

fn write_meta_json(agy_root: &Path, value: &Value) {
    let path = transcript::agy::metadata_path(agy_root);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

#[test]
fn agy_title_project_and_haa_workspace_resolution() {
    let claude = tempfile::tempdir().unwrap();
    let agy = fixture_root("agy");
    let cache = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let haa_home = work.path().join(".haa");
    let repo = work.path().join("my-repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let subdir = repo.join("packages/app");
    fs::create_dir_all(&subdir).unwrap();
    let repo_real = fs::canonicalize(&repo).unwrap();

    // Mutate metadata to exercise all title/workspace/haa combinations:
    // - AGY_TUI: long Title (> 120 chars, truncated), workspace inside a git repo -> folds to repo root
    // - AGY_SHORT: no Title (falls back to first prompt), workspace under haa_home -> haa_worker = 1
    // - AGY_HAA_WORKER: remove metadata entry -> falls back to first prompt, cwd = None, project = agy_root
    let long_title = "T".repeat(150);
    let mut meta = read_meta_json(agy.path());
    let convs = meta["conversations"].as_object_mut().unwrap();
    convs[AGY_TUI]["summary"]["Title"] = json!(long_title);
    convs[AGY_TUI]["summary"]["WorkspaceURIs"] = json!([format!("file://{}", subdir.display())]);
    convs[AGY_SHORT]["summary"]["Title"] = json!("");
    convs[AGY_SHORT]["summary"]["WorkspaceURIs"] =
        json!([format!("file://{}/worktrees/wt-1", haa_home.display())]);
    convs.remove(AGY_HAA_WORKER);
    write_meta_json(agy.path(), &meta);

    let engine = Engine::open(EngineConfig {
        data_root: claude.path().to_path_buf(),
        data_root_source: DataRootSource::Settings,
        agy_root: agy.path().to_path_buf(),
        haa_home: haa_home.clone(),
        cache_dir: cache.path().to_path_buf(),
    })
    .unwrap();
    engine.scan_all(&|_| {}).unwrap();

    let tui = engine.get_session(AGY_TUI).unwrap();
    assert_eq!(tui.summary.title_source, TitleSource::Ai);
    assert_eq!(tui.summary.title.chars().count(), 120);
    assert_eq!(tui.summary.project_id, repo_real.to_string_lossy());
    assert_eq!(tui.cwd.as_deref(), Some(subdir.to_string_lossy().as_ref()));

    let short = engine.get_session(AGY_SHORT).unwrap();
    assert_eq!(short.summary.title_source, TitleSource::FirstPrompt);
    assert_eq!(short.summary.title, "nostrud dolore lorem m");

    let agy_real = fs::canonicalize(agy.path()).unwrap();
    let haa = engine.get_session(AGY_HAA_WORKER).unwrap();
    assert_eq!(haa.summary.title_source, TitleSource::FirstPrompt);
    assert!(haa.cwd.is_none());
    assert_eq!(haa.summary.project_id, agy_real.to_string_lossy());
    assert_eq!(haa.project_path, agy_real.to_string_lossy());
    assert!(!haa.project_missing);

    // Check haa_worker flags in DB: AGY_SHORT via haa_home workspace, AGY_HAA_WORKER via mcp_haa_* tools, AGY_TUI neither.
    let db = Connection::open(cache.path().join("index.sqlite")).unwrap();
    let flag = |sid: &str| -> i64 {
        db.query_row("SELECT haa_worker FROM sessions WHERE id=?1", [sid], |r| {
            r.get(0)
        })
        .unwrap()
    };
    assert_eq!(flag(AGY_TUI), 0);
    assert_eq!(flag(AGY_SHORT), 1);
    assert_eq!(flag(AGY_HAA_WORKER), 1);
}

#[test]
fn haa_worktree_record_puts_a_worker_under_its_repo_on_the_next_sync() {
    let claude = tempfile::tempdir().unwrap();
    let agy = fixture_root("agy");
    let cache = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let haa_home = work.path().join(".haa");
    fs::create_dir_all(&haa_home).unwrap();
    let repo = work.path().join("my-repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let repo_real = fs::canonicalize(&repo).unwrap();
    // The worktree is gone (haa deletes it once merged): only agy's metadata still names it.
    let worktree = haa_home.join("worktrees/haa-agy-web-ecdd8f");
    let mut meta = read_meta_json(agy.path());
    meta["conversations"][AGY_SHORT]["summary"]["WorkspaceURIs"] =
        json!([format!("file://{}", worktree.display())]);
    write_meta_json(agy.path(), &meta);

    let engine = Engine::open(EngineConfig {
        data_root: claude.path().to_path_buf(),
        data_root_source: DataRootSource::Settings,
        agy_root: agy.path().to_path_buf(),
        haa_home: haa_home.clone(),
        cache_dir: cache.path().to_path_buf(),
    })
    .unwrap();
    engine.scan_all(&|_| {}).unwrap();
    let before = engine.get_session(AGY_SHORT).unwrap();
    assert_eq!(before.summary.project_id, worktree.to_string_lossy());
    assert!(before.project_missing);
    let tui_before = engine.get_session(AGY_TUI).unwrap().summary.project_id;

    // haa's record arrives (or is backfilled) without any agy file changing.
    fs::write(
        haa_home.join("usage.jsonl"),
        format!(
            "{}\n",
            json!({"kind": "worktree", "worker": "agy-web", "cwd": worktree, "repo": repo, "branch": "haa/agy-web-ecdd8f"})
        ),
    )
    .unwrap();
    engine.scan_all(&|_| {}).unwrap();
    let after = engine.get_session(AGY_SHORT).unwrap();
    assert_eq!(after.summary.project_id, repo_real.to_string_lossy());
    assert!(!after.project_missing);
    // The Session's own directory stays the worktree it ran in.
    assert_eq!(
        after.cwd.as_deref(),
        Some(worktree.to_string_lossy().as_ref())
    );
    // Sessions outside haa worktrees keep their project.
    assert_eq!(
        engine.get_session(AGY_TUI).unwrap().summary.project_id,
        tui_before
    );
}

#[test]
fn agy_skips_fts_with_done_progress_while_find_in_session_works() {
    let claude = fixture_root("basic");
    let agy = fixture_root("agy");
    let cache = tempfile::tempdir().unwrap();

    // Plant a unique marker only inside an agy assistant reply.
    let needle = "zqxjkv_agy_reply_only_marker";
    let short_path = transcript::agy::transcript_path(agy.path(), AGY_SHORT);
    let mut f = OpenOptions::new().append(true).open(&short_path).unwrap();
    writeln!(
        f,
        r#"{{"step_index":2,"type":"PLANNER_RESPONSE","created_at":"2026-11-27T08:57:06Z","content":"answer containing {needle}","model":"gemini-3.8-flash-low","input_tokens":10,"output_tokens":5}}"#
    )
    .unwrap();
    drop(f);

    // 1. agy-only root: immediately after scan_all (before index_text_backlog), text_ready is true.
    let agy_only_cache = tempfile::tempdir().unwrap();
    let empty_claude = tempfile::tempdir().unwrap();
    let agy_only = open_engine_with_agy(empty_claude.path(), agy.path(), agy_only_cache.path());
    agy_only.scan_all(&|_| {}).unwrap();
    assert!(
        agy_only.index_status().text_ready,
        "agy files must mark text_offset = parsed_offset immediately"
    );

    // 2. Mixed roots: agy files start at text_offset == parsed_offset while Claude files stay at 0.
    let engine = open_engine_with_agy(claude.path(), agy.path(), cache.path());
    engine.scan_all(&|_| {}).unwrap();
    assert!(!engine.index_status().text_ready);

    let db = Connection::open(cache.path().join("index.sqlite")).unwrap();
    let offsets: Vec<(String, i64, i64)> = db
        .prepare("SELECT source, text_offset, parsed_offset FROM files")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for (src, text_off, parsed_off) in &offsets {
        assert!(*parsed_off > 0);
        if src == SOURCE_AGY {
            assert_eq!(text_off, parsed_off, "agy file must already be text-done");
        } else {
            assert_eq!(*text_off, 0, "claude file awaits phase-2 backlog");
        }
    }

    // Run phase-2 FTS backlog: agy produces zero `msg_text` rows; ⌘K misses the needle; ⌘F finds it.
    engine
        .index_text_backlog(&|_| {}, &AtomicBool::new(false))
        .unwrap();
    assert!(engine.index_status().text_ready);

    let agy_text_rows: i64 = db
        .query_row(
            "SELECT count(*) FROM msg_text WHERE session_id IN (?1, ?2, ?3)",
            [AGY_TUI, AGY_HAA_WORKER, AGY_SHORT],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(agy_text_rows, 0);

    let search_res = engine
        .search(&SearchRequest {
            query: needle.to_owned(),
            project_id: None,
            max_sessions: 20,
            hits_per_session: 5,
        })
        .unwrap();
    assert!(
        search_res.groups.is_empty(),
        "⌘K must not return agy content"
    );

    let find_res = engine
        .find_in_session(&FindRequest {
            session_id: AGY_SHORT.to_owned(),
            scope: TranscriptScope::Main,
            branch_choices: Vec::new(),
            query: needle.to_owned(),
            include_hidden: false,
        })
        .unwrap();
    assert!(
        !find_res.matches.is_empty(),
        "⌘F must find the needle in the agy transcript"
    );
}

#[test]
fn agy_metadata_updates_live_after_restart_mixed_batch_and_noop_sync() {
    let claude = fixture_root("basic");
    let agy = fixture_root("agy");
    let cache = tempfile::tempdir().unwrap();
    let meta_path = transcript::agy::metadata_path(agy.path());
    let short_path = transcript::agy::transcript_path(agy.path(), AGY_SHORT);

    {
        let engine = open_engine_with_agy(claude.path(), agy.path(), cache.path());
        engine.scan_all(&|_| {}).unwrap();

        // Warm the display cache for AGY_TUI and AGY_SHORT.
        let _ = engine
            .get_transcript(&main_transcript_req(AGY_TUI))
            .unwrap();
        let _ = engine
            .get_transcript(&main_transcript_req(AGY_SHORT))
            .unwrap();

        // 1. No-op sync: nothing changed -> empty ChangeSet.
        assert_eq!(engine.scan_all(&|_| {}).unwrap(), ChangeSet::default());
        assert_eq!(
            engine
                .apply_changes(std::slice::from_ref(&meta_path))
                .unwrap(),
            ChangeSet::default()
        );

        // Touch an unrelated field (`UpdatedAt`) in metadata: fingerprint unchanged -> no recompute.
        let mut meta = read_meta_json(agy.path());
        meta["conversations"][AGY_TUI]["summary"]["UpdatedAt"] =
            json!("2026-11-28T00:00:00.000000Z");
        write_meta_json(agy.path(), &meta);
        assert_eq!(
            engine
                .apply_changes(std::slice::from_ref(&meta_path))
                .unwrap(),
            ChangeSet::default()
        );

        // 2. Runtime metadata-only change: change AGY_TUI's Title only -> only AGY_TUI updates.
        meta["conversations"][AGY_TUI]["summary"]["Title"] = json!("Runtime Renamed Title");
        write_meta_json(agy.path(), &meta);
        let cs = engine
            .apply_changes(std::slice::from_ref(&meta_path))
            .unwrap();
        assert_eq!(cs.changed, vec![AGY_TUI.to_owned()]);
        assert!(cs.removed.is_empty());
        assert_eq!(
            engine.get_session(AGY_TUI).unwrap().summary.title,
            "Runtime Renamed Title"
        );

        // 3. Mixed batch: AGY_SHORT's JSONL is overwritten (step_index 0) AND AGY_HAA_WORKER's
        // metadata title changes in the same `apply_changes` batch; AGY_TUI is untouched.
        let mut f = OpenOptions::new().append(true).open(&short_path).unwrap();
        writeln!(
            f,
            r#"{{"step_index":0,"type":"USER_INPUT","created_at":"2026-11-27T08:57:05Z","content":"<USER_REQUEST>mixed batch prompt</USER_REQUEST>"}}"#
        )
        .unwrap();
        drop(f);
        meta["conversations"][AGY_HAA_WORKER]["summary"]["Title"] =
            json!("Mixed Batch Worker Title");
        write_meta_json(agy.path(), &meta);

        let cs = engine
            .apply_changes(&[short_path.clone(), meta_path.clone()])
            .unwrap();
        let changed: BTreeSet<String> = cs.changed.into_iter().collect();
        assert_eq!(
            changed,
            BTreeSet::from([AGY_SHORT.to_owned(), AGY_HAA_WORKER.to_owned()])
        );
        assert_eq!(
            engine.get_session(AGY_HAA_WORKER).unwrap().summary.title,
            "Mixed Batch Worker Title"
        );
        // The cached transcript for AGY_SHORT reflects the rewritten step_index 0.
        let short_t = engine
            .get_transcript(&main_transcript_req(AGY_SHORT))
            .unwrap();
        let prompt = short_t
            .nodes
            .iter()
            .find_map(|n| match &n.body {
                NodeBody::UserPrompt { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .unwrap();
        assert_eq!(prompt, "mixed batch prompt");

        // Second sync with nothing changed does nothing.
        assert_eq!(engine.scan_all(&|_| {}).unwrap(), ChangeSet::default());
    }

    // 4. After restart: edit metadata while Engine is stopped, then reopen + scan_all.
    let mut meta = read_meta_json(agy.path());
    meta["conversations"][AGY_TUI]["summary"]["Title"] = json!("Offline Renamed Title");
    write_meta_json(agy.path(), &meta);

    let engine = open_engine_with_agy(claude.path(), agy.path(), cache.path());
    let cs = engine.scan_all(&|_| {}).unwrap();
    assert_eq!(cs.changed, vec![AGY_TUI.to_owned()]);
    assert_eq!(
        engine.get_session(AGY_TUI).unwrap().summary.title,
        "Offline Renamed Title"
    );
    assert_eq!(engine.scan_all(&|_| {}).unwrap(), ChangeSet::default());
}

#[test]
fn agy_pricing_rebill_reprice_and_stats_exclusion() {
    let claude = fixture_root("basic");
    let agy = fixture_root("agy");

    // Append an agy step using a priced Claude model name to ensure pricing skips agy even when
    // the model exists in the price table.
    let short_path = transcript::agy::transcript_path(agy.path(), AGY_SHORT);
    let mut f = OpenOptions::new().append(true).open(&short_path).unwrap();
    writeln!(
        f,
        r#"{{"step_index":2,"type":"PLANNER_RESPONSE","created_at":"2026-11-27T08:57:06Z","content":"using claude model","model":"claude-sonnet-4-5-20250929","input_tokens":500,"output_tokens":200,"cache_read_tokens":50}}"#
    )
    .unwrap();
    drop(f);

    // Baseline: Claude-only engine.
    let claude_cache = tempfile::tempdir().unwrap();
    let claude_only = open_engine(claude.path(), claude_cache.path());
    claude_only.scan_all(&|_| {}).unwrap();
    let base_stats = claude_only.stats(&StatsRequest::default()).unwrap();

    // Mixed: Claude + agy engine.
    let mixed_cache = tempfile::tempdir().unwrap();
    let mixed = open_engine_with_agy(claude.path(), agy.path(), mixed_cache.path());
    mixed.scan_all(&|_| {}).unwrap();

    // agy tokens are summed onto the Session, but cost = 0, unpriced = 0, model_costs = empty,
    // and DB messages.cost IS NULL.
    let short = mixed.get_session(AGY_SHORT).unwrap();
    assert_eq!(short.summary.tokens.input, 11_170.0 + 500.0);
    assert_eq!(short.summary.tokens.output, 1.0 + 200.0);
    assert_eq!(short.summary.tokens.cache_read, 50.0);
    assert_eq!(short.summary.cost, 0.0);
    assert_eq!(short.summary.unpriced_messages, 0);
    assert!(short.model_costs.is_empty());

    let db = Connection::open(mixed_cache.path().join("index.sqlite")).unwrap();
    let priced_agy_msgs: i64 = db
        .query_row(
            "SELECT count(*) FROM messages
             WHERE session_id IN (?1, ?2, ?3) AND cost IS NOT NULL",
            [AGY_TUI, AGY_HAA_WORKER, AGY_SHORT],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(priced_agy_msgs, 0);

    // Full reprice (`update_prices` with a table pricing both Claude and gemini-3.8-flash-low):
    // agy must stay unpriced with tokens intact and cost = 0 / unpriced = 0.
    let custom_prices = json!({
        "claude-sonnet-4-5-20250929": {
            "input_cost_per_token": 0.00001,
            "output_cost_per_token": 0.00002
        },
        "claude-opus-4-6": {
            "input_cost_per_token": 0.000015,
            "output_cost_per_token": 0.000075
        }
    })
    .to_string();
    assert!(mixed.update_prices(&custom_prices).unwrap());
    assert!(claude_only.update_prices(&custom_prices).unwrap());

    let short_after = mixed.get_session(AGY_SHORT).unwrap();
    assert_eq!(short_after.summary.tokens, short.summary.tokens);
    assert_eq!(short_after.summary.cost, 0.0);
    assert_eq!(short_after.summary.unpriced_messages, 0);
    assert!(short_after.model_costs.is_empty());
    let priced_agy_after: i64 = db
        .query_row(
            "SELECT count(*) FROM messages
             WHERE session_id IN (?1, ?2, ?3) AND cost IS NOT NULL",
            [AGY_TUI, AGY_HAA_WORKER, AGY_SHORT],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(priced_agy_after, 0);

    // Stats (unranged + single-day range covering agy's 2026-11-27 timestamp) match Claude-only.
    let base_stats_after = claude_only.stats(&StatsRequest::default()).unwrap();
    let mixed_stats = mixed.stats(&StatsRequest::default()).unwrap();
    assert_eq!(mixed_stats, base_stats_after);
    assert!(base_stats.overview.requests > 0);

    // Single-day range centered on agy's 2026-11-27T08:57:05Z must report zero agy usage.
    let agy_ts_ms = 1_795_769_825_000.0; // 2026-11-27T08:57:05Z
    let agy_day_req = StatsRequest {
        time_range: Some(TimeRange {
            from_ms: Some(agy_ts_ms - 3_600_000.0),
            to_ms: Some(agy_ts_ms + 3_600_000.0),
        }),
    };
    let agy_day_stats = mixed.stats(&agy_day_req).unwrap();
    assert_eq!(agy_day_stats.overview.requests, 0);
    assert!(agy_day_stats.sessions.is_empty());
    assert!(agy_day_stats.hourly_models.is_empty());

    // Stats drill-down (`Drill::Model` and `Drill::Day`) never returns agy sessions.
    let drill_model = mixed
        .list_sessions(&SessionQuery {
            drill: Some(Drill::Model {
                models: vec![
                    "gemini-3.8-flash-low".into(),
                    "claude-sonnet-4-5-20250929".into(),
                ],
            }),
            ..SessionQuery::default()
        })
        .unwrap();
    assert!(drill_model.iter().all(|s| s.agent == Agent::Claude));

    let drill_day = mixed
        .list_sessions(&SessionQuery {
            drill: Some(Drill::Day {
                day: "2026-11-27".into(),
            }),
            ..SessionQuery::default()
        })
        .unwrap();
    assert!(drill_day.iter().all(|s| s.agent == Agent::Claude));
}

#[test]
fn diagnostics_uses_agy_unknowns_and_filters_per_agent_without_fake_versions() {
    let claude = fixture_root("basic");
    let agy = fixture_root("agy");

    // 1. Add unknown items to the Claude file whose names are KNOWN in agy (`run_command`, `USER_INPUT`),
    //    plus a shared unknown tool (`shared_alien_tool`).
    let basic_path = claude
        .path()
        .join("projects")
        .join(ids::BASIC_DIR)
        .join(format!("{}.jsonl", ids::BASIC));
    let mut basic_text = fs::read_to_string(&basic_path).unwrap();
    basic_text.push('\n');
    basic_text.push_str(&format!(
        r#"{{"type":"USER_INPUT","uuid":"c-u1","parentUuid":null,"sessionId":"{}","version":"2.1.500","timestamp":"2026-11-06T03:00:00.000Z"}}"#,
        ids::BASIC
    ));
    basic_text.push('\n');
    basic_text.push_str(&format!(
        r#"{{"type":"assistant","uuid":"c-a1","parentUuid":null,"sessionId":"{}","version":"2.1.500","timestamp":"2026-11-06T03:00:01.000Z","message":{{"id":"m-diag","role":"assistant","model":"claude-sonnet-4-5-20250929","content":[{{"type":"tool_use","id":"tu-1","name":"run_command","input":{{}}}},{{"type":"tool_use","id":"tu-2","name":"shared_alien_tool","input":{{}}}}]}}}}"#,
        ids::BASIC
    ));
    basic_text.push('\n');
    fs::write(&basic_path, basic_text).unwrap();

    // 2. Add unknown items to the agy file whose names are KNOWN in Claude (`Bash`, `api-request`),
    //    plus the same `shared_alien_tool` and an agy-only `agy_alien_tool`.
    let short_path = transcript::agy::transcript_path(agy.path(), AGY_SHORT);
    let mut f = OpenOptions::new().append(true).open(&short_path).unwrap();
    writeln!(
        f,
        r#"{{"step_index":2,"type":"api-request","created_at":"2026-11-27T08:57:06Z"}}"#
    )
    .unwrap();
    writeln!(
        f,
        r#"{{"step_index":3,"type":"PLANNER_RESPONSE","created_at":"2026-11-27T08:57:07Z","tool_calls":[{{"name":"Bash","args":{{}}}},{{"name":"agy_alien_tool","args":{{}}}},{{"name":"shared_alien_tool","args":{{}}}}]}}"#
    )
    .unwrap();
    drop(f);

    let cache = tempfile::tempdir().unwrap();
    let engine = open_engine_with_agy(claude.path(), agy.path(), cache.path());
    engine.scan_all(&|_| {}).unwrap();

    // 3. Also seed stale `diag_counts` rows that are KNOWN to each row's own agent (`Bash` on the
    //    Claude file, `run_command` on the agy file): reading diagnostics must drop each agent's
    //    own known name without dropping the other agent's row of the same name.
    let db = Connection::open(cache.path().join("index.sqlite")).unwrap();
    let claude_fid: i64 = db
        .query_row(
            "SELECT id FROM files WHERE source = ?1 LIMIT 1",
            [SOURCE_CLAUDE],
            |r| r.get(0),
        )
        .unwrap();
    let agy_fid: i64 = db
        .query_row(
            "SELECT id FROM files WHERE session_id = ?1 LIMIT 1",
            [AGY_SHORT],
            |r| r.get(0),
        )
        .unwrap();
    db.execute(
        "INSERT INTO diag_counts(file_id, category, name, version, count) VALUES (?1, ?2, 'Bash', '2.1.500', 7)",
        rusqlite::params![claude_fid, DIAG_TOOL],
    )
    .unwrap();
    db.execute(
        "INSERT INTO diag_counts(file_id, category, name, version, count) VALUES (?1, ?2, 'run_command', '', 9)",
        rusqlite::params![agy_fid, DIAG_TOOL],
    )
    .unwrap();
    db.execute(
        "INSERT INTO diag_counts(file_id, category, name, version, count) VALUES (?1, ?2, 'api-request', '2.1.500', 4)",
        rusqlite::params![claude_fid, DIAG_ENTRY],
    )
    .unwrap();
    db.execute(
        "INSERT INTO diag_counts(file_id, category, name, version, count) VALUES (?1, ?2, 'USER_INPUT', '', 6)",
        rusqlite::params![agy_fid, DIAG_ENTRY],
    )
    .unwrap();

    let d = engine.diagnostics().unwrap();
    fn find<'a>(
        list: &'a [codeck_core::model::NameCount],
        name: &str,
    ) -> &'a codeck_core::model::NameCount {
        list.iter()
            .find(|n| n.name == name)
            .unwrap_or_else(|| panic!("{name} missing from {list:?}"))
    }

    // `Bash` on Claude was filtered out; `Bash` on agy (count 1, no version) survived.
    let bash = find(&d.unknown_tools, "Bash");
    assert_eq!(bash.count, 1);
    assert!(bash.versions.is_empty());

    // `run_command` on agy was filtered out; `run_command` on Claude (count 1, version 2.1.500) survived.
    let run_cmd = find(&d.unknown_tools, "run_command");
    assert_eq!(run_cmd.count, 1);
    assert_eq!(run_cmd.versions, vec!["2.1.500".to_owned()]);

    // Same for entry/step types: `api-request` survives only from agy, `USER_INPUT` only from Claude.
    let api_req = find(&d.unknown_entry_types, "api-request");
    assert_eq!(api_req.count, 1);
    assert!(api_req.versions.is_empty());

    let user_input = find(&d.unknown_entry_types, "USER_INPUT");
    assert_eq!(user_input.count, 1);
    assert_eq!(user_input.versions, vec!["2.1.500".to_owned()]);

    // agy-only unknown has no fabricated version; shared unknown merges counts with Claude's version only.
    let agy_alien = find(&d.unknown_tools, "agy_alien_tool");
    assert_eq!(agy_alien.count, 1);
    assert!(agy_alien.versions.is_empty());

    let shared = find(&d.unknown_tools, "shared_alien_tool");
    assert_eq!(shared.count, 2);
    assert_eq!(shared.versions, vec!["2.1.500".to_owned()]);

    // Known agy tools (`mcp_haa_report`, `manage_task`, etc.) from the agy fixture must NOT appear.
    assert!(
        d.unknown_tools
            .iter()
            .all(|n| !n.name.starts_with("mcp_haa_") && n.name != "manage_task"),
        "{:?}",
        d.unknown_tools
    );
    // agy must not fabricate empty or synthetic entries in `versions`.
    assert!(d.versions.iter().all(|v| !v.version.is_empty()));
}

#[test]
fn agy_live_probe_merges_into_list_detail_poll_and_is_live_skipping_missing_jsonl() {
    use codeck_core::model::{LiveState, LiveStatus};
    use std::sync::{Arc, Mutex};

    let claude = tempfile::tempdir().unwrap();
    let agy = fixture_root("agy");
    let cache = tempfile::tempdir().unwrap();
    let missing_jsonl_id = "00000000-0000-5000-8000-000000000000";

    let holders = Arc::new(Mutex::new(BTreeMap::from([
        (AGY_TUI.to_owned(), 4242u32),
        (missing_jsonl_id.to_owned(), 9999u32),
    ])));
    let probe_holders = Arc::clone(&holders);

    let engine = open_engine_with_agy(claude.path(), agy.path(), cache.path())
        .with_agy_live_probe(move |_| probe_holders.lock().unwrap().clone());
    engine.scan_all(&|_| {}).unwrap();

    let want = LiveState {
        status: LiveStatus::Unknown,
        raw_status: None,
        waiting_for: None,
        pid: Some(4242),
    };

    // list_sessions and get_session attach agy Live (status Unknown, pid set).
    let listed = engine.list_sessions(&SessionQuery::default()).unwrap();
    let tui_summary = listed.iter().find(|s| s.id == AGY_TUI).unwrap();
    assert_eq!(tui_summary.live.as_ref(), Some(&want));
    let short_summary = listed.iter().find(|s| s.id == AGY_SHORT).unwrap();
    assert_eq!(short_summary.live, None);
    assert_eq!(
        engine.get_session(AGY_TUI).unwrap().summary.live.as_ref(),
        Some(&want)
    );

    // live_now, poll_live and is_live include AGY_TUI and exclude the holder with no JSONL yet.
    let now = engine.live_now().unwrap();
    assert_eq!(now.len(), 1);
    assert_eq!(now[0].session_id, AGY_TUI);
    assert_eq!(now[0].state, want);

    let first_poll = engine.poll_live().unwrap().expect("initial live snapshot");
    assert_eq!(first_poll.live, now);
    assert_eq!(engine.poll_live().unwrap(), None, "unchanged snapshot");

    assert!(engine.is_live(AGY_TUI));
    assert!(!engine.is_live(AGY_SHORT));
    assert!(!engine.is_live(missing_jsonl_id));

    // Once the holder drops the lock, poll_live reports empty and is_live flips to false.
    holders.lock().unwrap().clear();
    let cleared = engine.poll_live().unwrap().expect("live cleared");
    assert!(cleared.live.is_empty());
    assert!(!engine.is_live(AGY_TUI));
}

#[test]
fn haa_worker_ownership_from_fixture_attaches_to_list_and_detail() {
    use codeck_core::model::HaaWorker;

    let claude = fixture_root("basic");
    let agy = fixture_root("agy");
    let haa_fixture = common::fixture_dir("haa");

    // 1. With the haa fixture: list & detail populate owner, name, and latest task title.
    let cache = tempfile::tempdir().unwrap();
    let engine = Engine::open(EngineConfig {
        data_root: claude.path().to_path_buf(),
        data_root_source: DataRootSource::Settings,
        agy_root: agy.path().to_path_buf(),
        haa_home: haa_fixture,
        cache_dir: cache.path().to_path_buf(),
    })
    .unwrap();
    engine.scan_all(&|_| {}).unwrap();

    let expected = HaaWorker {
        owner_session_id: Some(ids::BASIC.to_owned()),
        name: Some("agy-fixer".to_owned()),
        task_title: Some("implement agy live and haa ownership".to_owned()),
    };
    let listed = engine.list_sessions(&SessionQuery::default()).unwrap();
    let by_id: BTreeMap<String, Option<HaaWorker>> =
        listed.into_iter().map(|s| (s.id, s.haa_worker)).collect();
    assert_eq!(by_id[AGY_HAA_WORKER], Some(expected.clone()));
    assert_eq!(by_id[AGY_TUI], None);
    assert_eq!(by_id[AGY_SHORT], None);
    assert_eq!(by_id[ids::BASIC], None);
    assert_eq!(
        engine
            .get_session(AGY_HAA_WORKER)
            .unwrap()
            .summary
            .haa_worker,
        Some(expected)
    );

    // 2. Without haa files: sessions.haa_worker = 1 still surfaces an empty HaaWorker marker.
    let cache_empty = tempfile::tempdir().unwrap();
    let engine_no_haa = open_engine_with_agy(claude.path(), agy.path(), cache_empty.path());
    engine_no_haa.scan_all(&|_| {}).unwrap();
    assert_eq!(
        engine_no_haa
            .get_session(AGY_HAA_WORKER)
            .unwrap()
            .summary
            .haa_worker,
        Some(HaaWorker::default())
    );
    assert_eq!(
        engine_no_haa
            .get_session(AGY_TUI)
            .unwrap()
            .summary
            .haa_worker,
        None
    );
}

#[test]
fn get_session_attaches_sorted_haa_tasks_only_for_claude_sessions() {
    use codeck_core::model::HaaTask;

    let claude = fixture_root("basic");
    let agy = fixture_root("agy");
    let haa_dir = tempfile::tempdir().unwrap();
    fs::write(
        haa_dir.path().join("usage.jsonl"),
        [
            // Out-of-order tasks owned by the Claude session ids::BASIC.
            json!({
                "ts": "2026-11-06T03:05:00Z",
                "owner": ids::BASIC,
                "kind": "task",
                "worker": "agy-web",
                "title": "second task",
                "task": "implement frontend cards",
                "status": "done",
                "model": "gemini-4-neon-high",
                "durationMs": 180000,
                "sessionId": AGY_HAA_WORKER
            })
            .to_string(),
            json!({
                "ts": "2026-11-06T03:01:00Z",
                "owner": ids::BASIC,
                "kind": "task",
                "worker": "agy-old",
                "title": "first task",
                "task": "check legacy cache",
                "status": "failed",
                "model": "gemini-4-neon-high",
                "durationMs": 45000
            })
            .to_string(),
            // Task owned by an agy session id: agy get_session must still return empty haa_tasks.
            json!({
                "ts": "2026-11-06T03:02:00Z",
                "owner": AGY_HAA_WORKER,
                "kind": "task",
                "worker": "agy-nested",
                "status": "done"
            })
            .to_string(),
            // Bad timestamp skipped.
            json!({
                "ts": "invalid",
                "owner": ids::BASIC,
                "kind": "task",
                "worker": "agy-bad"
            })
            .to_string(),
        ]
        .join("\n"),
    )
    .unwrap();

    let cache = tempfile::tempdir().unwrap();
    let engine = Engine::open(EngineConfig {
        data_root: claude.path().to_path_buf(),
        data_root_source: DataRootSource::Settings,
        agy_root: agy.path().to_path_buf(),
        haa_home: haa_dir.path().to_path_buf(),
        cache_dir: cache.path().to_path_buf(),
    })
    .unwrap();
    engine.scan_all(&|_| {}).unwrap();

    let detail = engine.get_session(ids::BASIC).unwrap();
    assert_eq!(
        detail.haa_tasks,
        vec![
            HaaTask {
                worker: Some("agy-old".into()),
                session_id: None,
                title: Some("first task".into()),
                task: Some("check legacy cache".into()),
                status: Some("failed".into()),
                model: Some("gemini-4-neon-high".into()),
                ended_ms: chrono::DateTime::parse_from_rfc3339("2026-11-06T03:01:00Z")
                    .unwrap()
                    .timestamp_millis() as f64,
                duration_ms: Some(45_000.0),
            },
            HaaTask {
                worker: Some("agy-web".into()),
                session_id: Some(AGY_HAA_WORKER.into()),
                title: Some("second task".into()),
                task: Some("implement frontend cards".into()),
                status: Some("done".into()),
                model: Some("gemini-4-neon-high".into()),
                ended_ms: chrono::DateTime::parse_from_rfc3339("2026-11-06T03:05:00Z")
                    .unwrap()
                    .timestamp_millis() as f64,
                duration_ms: Some(180_000.0),
            },
        ]
    );

    assert!(
        engine
            .get_session(AGY_HAA_WORKER)
            .unwrap()
            .haa_tasks
            .is_empty()
    );
    assert!(engine.get_session(AGY_SHORT).unwrap().haa_tasks.is_empty());
}
