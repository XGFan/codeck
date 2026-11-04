//! Index (T2.1): scan dedupe, incremental appends, session-level reparses, removals and schema
//! rebuilds. Fixture copies in temp dirs only; assertions read the index tables directly so they
//! hold independently of assembly (`summarize` only feeds the session row's derived columns).
mod common;

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use common::{fixture_root, ids};
use codeck_core::model::DataRootSource;
use codeck_core::{ChangeSet, Engine, EngineConfig};
use rusqlite::{Connection, params};
use tempfile::TempDir;

struct Env {
    root: TempDir,
    cache: TempDir,
    engine: Engine,
}

fn open(root: &Path, cache: &Path) -> Engine {
    Engine::open(EngineConfig {
        data_root: root.to_path_buf(),
        data_root_source: DataRootSource::Settings,
        cache_dir: cache.to_path_buf(),
    })
    .expect("open engine")
}

fn scanned(scenario: &str) -> Env {
    let root = fixture_root(scenario);
    let cache = tempfile::tempdir().unwrap();
    let engine = open(root.path(), cache.path());
    engine.scan_all(&|_| {}).expect("scan_all");
    Env {
        root,
        cache,
        engine,
    }
}

impl Env {
    fn db(&self) -> Connection {
        Connection::open(self.cache.path().join("index.sqlite")).unwrap()
    }

    /// Canonical path of a main file (temp dirs live under a symlinked /var on macOS).
    fn main_file(&self, dir: &str, sid: &str) -> PathBuf {
        fs::canonicalize(
            self.root
                .path()
                .join("projects")
                .join(dir)
                .join(format!("{sid}.jsonl")),
        )
        .unwrap()
    }

    fn apply(&self, path: &Path) -> ChangeSet {
        self.engine
            .apply_changes(&[path.to_path_buf()])
            .expect("apply_changes")
    }
}

fn count(conn: &Connection, sql: &str, arg: &str) -> i64 {
    conn.query_row(sql, [arg], |r| r.get(0)).unwrap()
}

fn messages(conn: &Connection, sid: &str) -> i64 {
    count(
        conn,
        "SELECT count(*) FROM messages WHERE session_id=?1",
        sid,
    )
}

/// (id, parsed_offset, text_offset, line_count, size) of a file row.
fn file_row(conn: &Connection, path: &Path) -> (i64, i64, i64, i64, i64) {
    conn.query_row(
        "SELECT id, parsed_offset, text_offset, line_count, size FROM files WHERE path=?1",
        [path.to_string_lossy()],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )
    .unwrap()
}

/// Marks a file row so a session-level reparse (which recreates the row) is detectable.
fn set_sentinel(conn: &Connection, path: &Path) {
    conn.execute(
        "UPDATE files SET text_offset = 7 WHERE path=?1",
        params![path.to_string_lossy()],
    )
    .unwrap();
}

fn append(path: &Path, text: &str) {
    let mut f = OpenOptions::new().append(true).open(path).unwrap();
    f.write_all(text.as_bytes()).unwrap();
}

fn prompt_line(uuid: &str, parent: &str, text: &str) -> String {
    format!(
        r#"{{"type":"user","uuid":"{uuid}","parentUuid":"{parent}","timestamp":"2026-10-07T10:00:00.000Z","sessionId":"{sid}","message":{{"role":"user","content":"{text}"}}}}"#,
        sid = ids::BASIC
    ) + "\n"
}

fn assistant_line(uuid: &str, parent: &str, msg_id: &str) -> String {
    format!(
        r#"{{"type":"assistant","uuid":"{uuid}","parentUuid":"{parent}","timestamp":"2026-10-07T10:00:01.000Z","sessionId":"{sid}","message":{{"id":"{msg_id}","role":"assistant","model":"claude-test","content":[{{"type":"text","text":"ok"}}],"usage":{{"input_tokens":1,"output_tokens":2}}}}}}"#,
        sid = ids::BASIC
    ) + "\n"
}

#[test]
fn symlinked_dir_and_copies_index_one_session_each() {
    let env = scanned("copies");
    let db = env.db();
    let sessions: i64 = db
        .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(sessions, 2, "one row per session id");
    let main_files: Vec<String> = db
        .prepare("SELECT path FROM files WHERE role=0 ORDER BY path")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(main_files.len(), 4, "2 real copies per session");
    assert!(
        main_files.iter().all(|p| !p.contains(ids::COPIES_LINK)),
        "symlinked project dir is deduplicated by realpath: {main_files:?}"
    );
    // Union by key across copies: the superset has 4 prompts + 4 assistant messages.
    assert_eq!(messages(&db, ids::COPIES_SUPERSET), 8);
    assert_eq!(messages(&db, ids::COPIES_IDENTICAL), 4);
    let changed = env.engine.apply_changes(&[]).unwrap();
    assert_eq!(
        changed,
        ChangeSet::default(),
        "a rescan without changes is a no-op"
    );
}

#[test]
fn append_is_incremental() {
    let env = scanned("basic");
    let db = env.db();
    let path = env.main_file(ids::BASIC_DIR, ids::BASIC);
    let (id, offset, _, lines, _) = file_row(&db, &path);
    let msgs = messages(&db, ids::BASIC);
    set_sentinel(&db, &path);

    append(
        &path,
        &(prompt_line(
            "t-u1",
            "0bd99c42-5d43-5421-bf13-548ddfe11016",
            "appended prompt",
        ) + &assistant_line("t-a1", "t-u1", "msg_t1")),
    );
    let cs = env.apply(&path);
    assert_eq!(cs.changed, vec![ids::BASIC.to_owned()]);

    let size = fs::metadata(&path).unwrap().len() as i64;
    let (id2, offset2, sentinel, lines2, _) = file_row(&db, &path);
    assert_eq!(id2, id, "file row kept");
    assert_eq!(sentinel, 7, "no session reparse (row not recreated)");
    assert!(offset2 > offset);
    assert_eq!(offset2, size);
    assert_eq!(lines2, lines + 2);
    assert_eq!(messages(&db, ids::BASIC), msgs + 2);
    let out_tok: i64 = db
        .query_row(
            "SELECT out_tok FROM messages WHERE session_id=?1 AND key='msg_t1'",
            [ids::BASIC],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(out_tok, 2);
}

#[test]
fn truncate_triggers_full_reparse() {
    let env = scanned("basic");
    let db = env.db();
    let path = env.main_file(ids::BASIC_DIR, ids::BASIC);
    let msgs = messages(&db, ids::BASIC);
    set_sentinel(&db, &path);

    let text = fs::read_to_string(&path).unwrap();
    let kept: String = text.split_inclusive('\n').take(20).collect();
    fs::write(&path, &kept).unwrap();
    let cs = env.apply(&path);
    assert_eq!(cs.changed, vec![ids::BASIC.to_owned()]);

    let (_, offset, sentinel, lines, size) = file_row(&db, &path);
    assert_eq!(sentinel, 0, "row recreated by the session reparse");
    assert_eq!(offset, kept.len() as i64);
    assert_eq!(size, kept.len() as i64);
    assert_eq!(lines, 20);
    assert!(messages(&db, ids::BASIC) < msgs);
}

#[test]
fn inode_replace_triggers_full_reparse() {
    let env = scanned("basic");
    let db = env.db();
    let path = env.main_file(ids::BASIC_DIR, ids::BASIC);
    let (_, _, _, lines, _) = file_row(&db, &path);
    let msgs = messages(&db, ids::BASIC);
    set_sentinel(&db, &path);

    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str(&prompt_line(
        "t-u2",
        "0bd99c42-5d43-5421-bf13-548ddfe11016",
        "after replace",
    ));
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, &text).unwrap();
    fs::rename(&tmp, &path).unwrap();
    env.apply(&path);

    let (_, offset, sentinel, lines2, _) = file_row(&db, &path);
    assert_eq!(sentinel, 0, "new inode: session reparsed");
    assert_eq!(offset, text.len() as i64);
    assert_eq!(lines2, lines + 1);
    assert_eq!(messages(&db, ids::BASIC), msgs + 1);
}

#[test]
fn partial_trailing_line_is_picked_up_after_completion() {
    let env = scanned("basic");
    let db = env.db();
    let path = env.main_file(ids::BASIC_DIR, ids::BASIC);
    let (_, offset, _, _, _) = file_row(&db, &path);
    let msgs = messages(&db, ids::BASIC);

    let line = prompt_line(
        "t-u3",
        "0bd99c42-5d43-5421-bf13-548ddfe11016",
        "slowly written",
    );
    let (head, tail) = line.split_at(line.len() / 2);
    append(&path, head);
    env.apply(&path);
    let (_, offset_mid, _, _, size_mid) = file_row(&db, &path);
    assert_eq!(offset_mid, offset, "an incomplete line is not consumed");
    assert_eq!(size_mid, fs::metadata(&path).unwrap().len() as i64);
    assert_eq!(messages(&db, ids::BASIC), msgs);

    append(&path, tail);
    env.apply(&path);
    let (_, offset_end, _, _, _) = file_row(&db, &path);
    assert_eq!(offset_end, fs::metadata(&path).unwrap().len() as i64);
    assert_eq!(messages(&db, ids::BASIC), msgs + 1);
}

#[test]
fn schema_or_root_mismatch_rebuilds() {
    let root = fixture_root("basic");
    let cache = tempfile::tempdir().unwrap();
    let files = |c: &Connection| -> i64 {
        c.query_row("SELECT count(*) FROM files", [], |r| r.get(0))
            .unwrap()
    };
    {
        let engine = open(root.path(), cache.path());
        engine.scan_all(&|_| {}).unwrap();
    }
    let db_path = cache.path().join("index.sqlite");
    {
        let c = Connection::open(&db_path).unwrap();
        assert_eq!(files(&c), 1);
        c.pragma_update(None, "user_version", 999).unwrap();
    }
    {
        let engine = open(root.path(), cache.path());
        let c = Connection::open(&db_path).unwrap();
        let v: u32 = c
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, codeck_core::index::schema::SCHEMA_VERSION);
        assert_eq!(files(&c), 0, "version mismatch: index recreated");
        let cs = engine.scan_all(&|_| {}).unwrap();
        assert_eq!(cs.changed, vec![ids::BASIC.to_owned()]);
        assert_eq!(files(&c), 1);
    }
    let other_root = fixture_root("live");
    let _engine = open(other_root.path(), cache.path());
    let c = Connection::open(&db_path).unwrap();
    assert_eq!(files(&c), 0, "data root mismatch: index recreated");
}

#[test]
fn deleted_file_removes_session() {
    let env = scanned("basic");
    let db = env.db();
    let path = env.main_file(ids::BASIC_DIR, ids::BASIC);
    fs::remove_file(&path).unwrap();
    let cs = env.apply(&path);
    assert_eq!(cs.removed, vec![ids::BASIC.to_owned()]);
    assert!(cs.projects_changed);
    for table in [
        "sessions WHERE id",
        "files WHERE session_id",
        "messages WHERE session_id",
    ] {
        assert_eq!(
            count(&db, &format!("SELECT count(*) FROM {table}=?1"), ids::BASIC),
            0,
            "{table}"
        );
    }
    let projects: i64 = db
        .query_row("SELECT count(*) FROM projects", [], |r| r.get(0))
        .unwrap();
    assert_eq!(projects, 0, "project without sessions is pruned");
}

#[test]
fn deleted_copy_with_surviving_sibling_keeps_session() {
    let env = scanned("copies");
    let db = env.db();
    // Drop the superset copy: the session is reparsed from the 2-turn prefix copy (A7).
    let big = env.main_file(ids::COPIES_DIR, ids::COPIES_SUPERSET);
    fs::remove_file(&big).unwrap();
    let cs = env.apply(&big);
    assert_eq!(cs.changed, vec![ids::COPIES_SUPERSET.to_owned()]);
    assert!(cs.removed.is_empty());
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM sessions WHERE id=?1",
            ids::COPIES_SUPERSET
        ),
        1
    );
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM files WHERE session_id=?1",
            ids::COPIES_SUPERSET
        ),
        1
    );
    assert_eq!(
        messages(&db, ids::COPIES_SUPERSET),
        4,
        "rows of the removed copy are gone"
    );

    // Drop one of two identical copies: nothing is lost.
    let old = env.main_file(ids::COPIES_DIR_OLD, ids::COPIES_IDENTICAL);
    fs::remove_file(&old).unwrap();
    env.apply(&old);
    assert_eq!(messages(&db, ids::COPIES_IDENTICAL), 4);
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM sessions WHERE id=?1",
            ids::COPIES_IDENTICAL
        ),
        1
    );
}

#[test]
fn agent_files_and_sidecars_are_classified() {
    let env = scanned("workflow");
    let db = env.db();
    let by_role = |role: i64| -> i64 {
        db.query_row("SELECT count(*) FROM files WHERE role=?1", [role], |r| {
            r.get(0)
        })
        .unwrap()
    };
    assert_eq!(by_role(0), 1);
    assert_eq!(by_role(2), 5, "only agent-*.jsonl are agent transcripts");
    let runs: i64 = db
        .query_row(
            "SELECT count(DISTINCT workflow_run_id) FROM subagents WHERE session_id=?1",
            [ids::WORKFLOW],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(runs, 2);

    let env = scanned("subagents");
    let db = env.db();
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM subagents WHERE session_id=?1",
            ids::SUBAGENTS
        ),
        5
    );
    // A8: session totals = main + Σ agents; the agent part comes from the index alone.
    let (total, main): (i64, i64) = db
        .query_row(
            "SELECT out_tok, main_out_tok FROM sessions WHERE id=?1",
            [ids::SUBAGENTS],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let agents: i64 = db
        .query_row(
            "SELECT sum(out_tok) FROM messages WHERE session_id=?1 AND agent_id != ''",
            [ids::SUBAGENTS],
            |r| r.get(0),
        )
        .unwrap();
    assert!(agents > 0);
    assert_eq!(total - main, agents);
}

/// Engine read API over the index. Titles/counts come from `assemble::summarize` (T2.2).
#[test]
fn engine_lists_projects_sessions_and_detail() {
    use codeck_core::model::{SessionQuery, SessionSort};
    let env = scanned("fork");
    let projects = env.engine.list_projects().unwrap();
    assert_eq!(projects.len(), 1);
    let p = &projects[0];
    assert_eq!(
        p.id, "/Users/dev/code/tidepool",
        "raw cwd when the directory is gone"
    );
    assert_eq!(p.display_name, "tidepool");
    assert!(p.missing);
    assert_eq!(p.session_count, 2);

    let q = SessionQuery {
        project_ids: vec![p.id.clone()],
        sort: SessionSort::Created,
        descending: false,
        ..SessionQuery::default()
    };
    let list = env.engine.list_sessions(&q).unwrap();
    let order: Vec<&str> = list.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(order, vec![ids::FORK_ORIGIN, ids::FORK_CHILD]);
    let child = &list[1];
    assert_eq!(
        child.fork_origin.as_ref().map(|o| o.session_id.as_str()),
        Some(ids::FORK_ORIGIN)
    );
    let other = SessionQuery {
        project_ids: vec!["/nowhere".into()],
        ..SessionQuery::default()
    };
    assert!(env.engine.list_sessions(&other).unwrap().is_empty());

    let detail = env.engine.get_session(ids::FORK_ORIGIN).unwrap();
    assert_eq!(
        detail.resume_command,
        format!("claude --resume {}", ids::FORK_ORIGIN)
    );
    assert_eq!(detail.files.len(), 1);
    assert_eq!(detail.forks.len(), 1);
    assert_eq!(detail.forks[0].session_id, ids::FORK_CHILD);
    assert!(env.engine.get_session("missing-id").is_err());
    let path = env.engine.session_file_path(ids::FORK_CHILD, None).unwrap();
    assert_eq!(path, env.main_file(ids::FORK_DIR, ids::FORK_CHILD));
}

/// Live follow: the cached parse is extended by the appended bytes and the revision changes.
/// Node shapes come from assembly (T2.2).
#[test]
fn transcript_follows_appends() {
    use codeck_core::model::{TranscriptRequest, TranscriptScope};
    let env = scanned("basic");
    let req = TranscriptRequest {
        session_id: ids::BASIC.into(),
        scope: TranscriptScope::Main,
        branch_choices: vec![],
        include_hidden: false,
    };
    let t1 = env.engine.get_transcript(&req).unwrap();
    assert_eq!(
        env.engine.get_transcript(&req).unwrap().revision,
        t1.revision
    );

    let path = env.main_file(ids::BASIC_DIR, ids::BASIC);
    append(
        &path,
        &(prompt_line("t-u4", "0bd99c42-5d43-5421-bf13-548ddfe11016", "follow me")
            + &assistant_line("t-a4", "t-u4", "msg_t4")),
    );
    env.apply(&path);
    let t2 = env.engine.get_transcript(&req).unwrap();
    assert_ne!(t2.revision, t1.revision);
    assert_eq!(t2.nodes.len(), t1.nodes.len() + 2);
    assert_eq!(t2.nodes.last().unwrap().id, "t-a4");
}

/// Rebuild rescans with progress and reports everything as changed (no second scan needed).
#[test]
fn rebuild_rescans_and_reports_changes() {
    use codeck_core::model::{IndexPhase, SessionQuery};
    let env = scanned("subagents");
    let phases = std::cell::RefCell::new(Vec::new());
    let cs = env
        .engine
        .rebuild(&|s| phases.borrow_mut().push(s.phase))
        .unwrap();
    assert!(cs.projects_changed);
    assert_eq!(cs.changed, vec![ids::SUBAGENTS.to_owned()]);
    assert!(phases.borrow().contains(&IndexPhase::Scanning));
    let sessions = env.engine.list_sessions(&SessionQuery::default()).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(env.engine.scan_all(&|_| {}).unwrap(), ChangeSet::default());
}

/// A9: a sidecar rewritten while the app was closed is picked up by the first scan after start.
#[test]
fn sidecar_changed_while_closed_is_picked_up() {
    let root = fixture_root("subagents");
    let cache = tempfile::tempdir().unwrap();
    let t0 = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    let set_mtime = |p: &Path, t| {
        fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(t)
            .unwrap();
    };
    let subagents = root
        .path()
        .join("projects")
        .join(ids::SUBAGENTS_DIR)
        .join(ids::SUBAGENTS)
        .join("subagents");
    for e in fs::read_dir(&subagents).unwrap() {
        set_mtime(&e.unwrap().path(), t0);
    }
    let main = subagents.parent().unwrap().with_extension("jsonl");
    set_mtime(&main, t0);
    open(root.path(), cache.path()).scan_all(&|_| {}).unwrap();
    // Restart without changes: nothing is recomputed.
    let restarted = || open(root.path(), cache.path()).scan_all(&|_| {}).unwrap();
    assert_eq!(restarted(), ChangeSet::default());

    let meta = subagents.join("agent-a87b5e24025a157e4.meta.json");
    fs::write(
        &meta,
        r#"{"agentType":"renamed","description":"quis ullamc","spawnDepth":1,"toolUseId":"toolu_01CZKpZhFGtnywQwyH8oUrsj"}"#,
    )
    .unwrap();
    set_mtime(&meta, t0 + std::time::Duration::from_secs(60));
    assert_eq!(restarted().changed, vec![ids::SUBAGENTS.to_owned()]);
    let db = Connection::open(cache.path().join("index.sqlite")).unwrap();
    let agent_type: String = db
        .query_row(
            "SELECT agent_type FROM subagents WHERE agent_id='a87b5e24025a157e4'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(agent_type, "renamed");
}

/// One-line Session entry for `sid` in `cwd`; `content` is the message's JSON content.
fn entry_line(ty: &str, uuid: &str, parent: &str, sid: &str, cwd: &Path, content: &str) -> String {
    let parent = if parent.is_empty() {
        "null".to_owned()
    } else {
        format!(r#""{parent}""#)
    };
    let msg = if ty == "user" {
        format!(r#"{{"role":"user","content":{content}}}"#)
    } else {
        format!(
            r#"{{"id":"m-{uuid}","role":"assistant","model":"claude-test","content":{content},"usage":{{"input_tokens":1,"output_tokens":1}}}}"#
        )
    };
    format!(
        r#"{{"type":"{ty}","uuid":"{uuid}","parentUuid":{parent},"timestamp":"2026-10-07T10:00:00.000Z","sessionId":"{sid}","cwd":"{cwd}","message":{msg}}}"#,
        cwd = cwd.display()
    ) + "\n"
}

fn edit_block(tool: &str, file: &Path) -> String {
    format!(
        r#"[{{"type":"tool_use","id":"tu-{tool}-{n}","name":"{tool}","input":{{"file_path":"{f}"}}}}]"#,
        n = file.file_name().unwrap().to_string_lossy(),
        f = file.display()
    )
}

/// A Session started in a workspace (not a repository) belongs to it and to every repository its
/// main file or Subagents edited; appended edits extend the set.
#[test]
fn session_belongs_to_its_start_project_and_the_projects_it_edited() {
    use codeck_core::model::SessionQuery;
    let work = tempfile::tempdir().unwrap();
    let ws = fs::canonicalize(work.path()).unwrap().join("ws");
    for repo in ["api", "web", "lib"] {
        fs::create_dir_all(ws.join(repo).join(".git")).unwrap();
    }
    let root = tempfile::tempdir().unwrap();
    let sid = "11111111-2222-3333-4444-555555555555";
    let dir = root.path().join("projects/-ws");
    fs::create_dir_all(dir.join(sid).join("subagents")).unwrap();
    let main = dir.join(format!("{sid}.jsonl"));
    fs::write(
        &main,
        entry_line("user", "u1", "", sid, &ws, r#""fix it""#)
            + &entry_line(
                "assistant",
                "a1",
                "u1",
                sid,
                &ws,
                &edit_block("Edit", &ws.join("api/src/a.rs")),
            )
            // A rejected write changes nothing.
            + &entry_line(
                "assistant",
                "f1",
                "a1",
                sid,
                &ws,
                &edit_block("Write", &ws.join("lib/fail.rs")),
            )
            + &entry_line(
                "user",
                "f2",
                "f1",
                sid,
                &ws,
                r#"[{"type":"tool_result","tool_use_id":"tu-Write-fail.rs","is_error":true,"content":"denied"}]"#,
            ),
    )
    .unwrap();
    fs::write(
        dir.join(sid).join("subagents/agent-x1.jsonl"),
        entry_line(
            "assistant",
            "x1",
            "",
            sid,
            &ws,
            &edit_block("Write", &ws.join("web/index.ts")),
        ),
    )
    .unwrap();
    let cache = tempfile::tempdir().unwrap();
    let engine = open(root.path(), cache.path());
    engine.scan_all(&|_| {}).unwrap();

    let id = |name: &str| ws.join(name).to_string_lossy().into_owned();
    let in_project = |p: &str| {
        let q = SessionQuery {
            project_ids: vec![p.to_owned()],
            ..SessionQuery::default()
        };
        engine
            .list_sessions(&q)
            .unwrap()
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>()
    };
    let all = engine.list_sessions(&SessionQuery::default()).unwrap();
    assert_eq!(all[0].project_id, ws.to_string_lossy());
    let mut edited = all[0].edited_project_ids.clone();
    edited.sort();
    assert_eq!(edited, vec![id("api"), id("web")]);
    for p in [ws.to_string_lossy().into_owned(), id("api"), id("web")] {
        assert_eq!(in_project(&p), vec![sid.to_owned()], "{p}");
    }
    assert!(in_project(&id("lib")).is_empty());
    let mut projects: Vec<(String, u32)> = engine
        .list_projects()
        .unwrap()
        .into_iter()
        .map(|p| (p.display_name, p.session_count))
        .collect();
    projects.sort();
    assert_eq!(
        projects,
        vec![("api".into(), 1), ("web".into(), 1), ("ws".into(), 1)]
    );

    append(
        &main,
        &entry_line(
            "assistant",
            "a2",
            "a1",
            sid,
            &ws,
            &edit_block("MultiEdit", &ws.join("lib/x.rs")),
        ),
    );
    engine.apply_changes(std::slice::from_ref(&main)).unwrap();
    assert_eq!(in_project(&id("lib")), vec![sid.to_owned()]);

    // Rewritten without the main-file edits: those Projects drop out and are pruned.
    fs::write(&main, entry_line("user", "u1", "", sid, &ws, r#""fix it""#)).unwrap();
    engine.apply_changes(std::slice::from_ref(&main)).unwrap();
    assert!(in_project(&id("api")).is_empty());
    assert!(in_project(&id("lib")).is_empty());
    let mut names: Vec<String> = engine
        .list_projects()
        .unwrap()
        .into_iter()
        .map(|p| p.display_name)
        .collect();
    names.sort();
    assert_eq!(names, vec!["web", "ws"]);
}
