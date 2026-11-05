//! Search: query parsing, FTS / LIKE / Mixed search over the phase-2 text index, filters,
//! hit addressing (subagents, abandoned Branches), the tool-output scan, ⌘F and jump resolution.
//! Fixture copies in temp dirs only.
mod common;

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use codeck_core::model::{
    AppError, DataRootSource, ErrorCode, FindLocation, FindRequest, JumpRequest, SearchMode,
    SearchRequest, SearchResponse, SearchRole, TranscriptRequest, TranscriptScope,
};
use codeck_core::search::query::{fts_phrase, is_fts_term};
use codeck_core::search::{fts, parse_query};
use codeck_core::{Engine, EngineConfig};
use common::engine::{Env, data_root, open_engine};
use common::{fixture_root, ids};
use rusqlite::Connection;
use serde_json::Value;

/// A data root holding the given fixture scenarios, scanned and fully text-indexed.
fn indexed(scenarios: &[&str]) -> Env {
    Env::indexed(data_root(scenarios))
}

impl Env {
    fn search(&self, query: &str) -> SearchResponse {
        self.engine.search(&req(query)).unwrap()
    }

    fn file(&self, rel: &str) -> PathBuf {
        self.root.path().join("projects").join(rel)
    }
}

fn req(query: &str) -> SearchRequest {
    SearchRequest {
        query: query.to_owned(),
        project_id: None,
        max_sessions: 50,
        hits_per_session: 50,
    }
}

/// (session id, hit node ids) per group, for mode-independent comparisons.
fn shape(r: &SearchResponse) -> Vec<(String, u32, Vec<String>)> {
    r.groups
        .iter()
        .map(|g| {
            let mut ids: Vec<String> = g.hits.iter().map(|h| h.node_id.clone()).collect();
            ids.sort();
            (g.session.id.clone(), g.hit_count, ids)
        })
        .collect()
}

fn snippet_text(r: &SearchResponse) -> Vec<String> {
    r.groups
        .iter()
        .flat_map(|g| &g.hits)
        .map(|h| h.snippet.iter().map(|p| p.text.as_str()).collect())
        .collect()
}

/// uuid of the first entry of `path` whose text content contains `needle`.
fn uuid_with(path: &Path, needle: &str) -> String {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| l.contains(needle))
        .find_map(|l| {
            let v: Value = serde_json::from_str(l).ok()?;
            v["uuid"].as_str().map(str::to_owned)
        })
        .unwrap_or_else(|| panic!("no entry with {needle:?} in {}", path.display()))
}

/// leafUuid of the last `last-prompt` entry.
fn last_leaf(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["type"] == "last-prompt")
        .filter_map(|v| v["leafUuid"].as_str().map(str::to_owned))
        .next_back()
        .unwrap()
}

fn append_prompt(path: &Path, uuid: &str, parent: &str, sid: &str, text: &str) {
    let line = serde_json::json!({
        "type": "user", "uuid": uuid, "parentUuid": parent, "sessionId": sid,
        "timestamp": "2026-10-07T10:00:00.000Z",
        "message": {"role": "user", "content": text},
    });
    let mut f = OpenOptions::new().append(true).open(path).unwrap();
    writeln!(f, "{line}").unwrap();
}

// ------------------------------------------------------------------ parser

#[test]
fn parser_phrases_exclusions_and_errors() {
    let q = parse_query(r#"retry "exponential backoff" -flaky -"dead code""#).unwrap();
    assert_eq!(q.include, ["retry", "exponential backoff"]);
    assert_eq!(q.exclude, ["flaky", "dead code"]);
    // Inner quotes of a word are literal; an unterminated phrase runs to the end; "-" alone is a term.
    let q = parse_query(r#"key="v" - "open end"#).unwrap();
    assert_eq!(q.include, [r#"key="v""#, "-", "open end"]);

    let err: AppError = parse_query("-foo -\"bar baz\"").unwrap_err().into();
    assert_eq!(err.code, ErrorCode::InvalidQuery);
    assert_eq!(err.message, "至少需要一个包含词");
    assert!(parse_query("  \"\" ").is_err());

    // Quote doubling and the 3-character trigram threshold (characters, not bytes).
    assert_eq!(fts_phrase(r#"key="v""#), r#""key=""v""""#);
    assert!(is_fts_term("abc") && is_fts_term("重试吧"));
    assert!(!is_fts_term("ab") && !is_fts_term("重试"));
}

// ------------------------------------------------------------------ FTS / LIKE

#[test]
fn fts_and_like_agree_on_ascii() {
    let env = indexed(&["basic", "branch", "subagents", "persisted_and_images"]);
    let db = env.db();
    for q in [
        "retry",
        "Summary",
        "property tests",
        "\"exponential backoff\"",
        "lorem -ipsum",
        "tests -unit",
    ] {
        let a = fts::run(&db, &req(q), false).unwrap();
        let b = fts::run(&db, &req(q), true).unwrap();
        assert_eq!(b.mode, SearchMode::Like);
        assert_ne!(a.mode, SearchMode::Like, "{q}");
        assert!(a.total_hits > 0, "{q} finds something");
        assert_eq!(shape(&a), shape(&b), "{q}");
        assert_eq!(
            (a.total_hits, a.total_sessions),
            (b.total_hits, b.total_sessions)
        );
    }
}

#[test]
fn search_hits_snippets_and_case() {
    let env = indexed(&["basic"]);
    let r = env.search("EXPONENTIAL backoff");
    assert_eq!(r.mode, SearchMode::Fts);
    assert!(r.index_complete);
    assert_eq!(r.groups.len(), 1);
    let g = &r.groups[0];
    assert_eq!(g.session.id, ids::BASIC);
    let roles: Vec<SearchRole> = g.hits.iter().map(|h| h.role).collect();
    assert!(roles.contains(&SearchRole::Assistant), "{roles:?}");
    let hit = g.hits.iter().find(|h| h.role == SearchRole::User).unwrap();
    assert!(hit.on_main_line && hit.agent_id.is_none());
    let marked: Vec<&str> = hit
        .snippet
        .iter()
        .filter(|p| p.hit)
        .map(|p| p.text.as_str())
        .collect();
    assert!(
        marked.iter().any(|t| t.eq_ignore_ascii_case("exponential")),
        "{marked:?}"
    );

    // Short term → LIKE window around the hit.
    let r = env.search("ok");
    assert_eq!(r.mode, SearchMode::Like);
    // Mixed: FTS term + short term.
    let r = env.search("retry ok");
    assert_eq!(r.mode, SearchMode::Mixed);
    // All-short positive + FTS-able exclusion (`Mixed`).
    let only_short = env.search("it");
    let excluded = env.search("it -retry");
    assert_eq!(excluded.mode, SearchMode::Mixed);
    assert!(excluded.total_hits < only_short.total_hits);
    for s in snippet_text(&excluded) {
        assert!(!s.to_lowercase().contains("retry"), "{s}");
    }
    // Invalid query surfaces as an error.
    assert!(env.engine.search(&req("-retry")).is_err());
}

#[test]
fn project_filter_tool_inputs_and_grouping_limits() {
    let env = indexed(&["basic", "branch"]);
    let db = env.db();
    let project: String = db
        .query_row(
            "SELECT project_id FROM sessions WHERE id=?1",
            [ids::BASIC],
            |r| r.get(0),
        )
        .unwrap();
    // "test" appears in both sessions.
    let all = env.search("test");
    assert_eq!(all.groups.len(), 2);
    let r = env
        .engine
        .search(&SearchRequest {
            project_id: Some(project),
            ..req("test")
        })
        .unwrap();
    assert_eq!(r.groups.len(), 1);
    assert_eq!(r.groups[0].session.id, ids::BASIC);

    // The Bash command only exists as a tool input, which is searched too.
    let r = env.search("\"cargo test --lib\"");
    assert_eq!(r.total_hits, 1);
    let hit = &r.groups[0].hits[0];
    assert_eq!(hit.role, SearchRole::ToolInput);
    assert!(
        hit.tool_use_id
            .as_deref()
            .is_some_and(|t| t.starts_with("toolu_"))
    );

    // Grouping limits: one session, one hit, totals still complete.
    let r = env
        .engine
        .search(&SearchRequest {
            max_sessions: 1,
            hits_per_session: 1,
            ..req("test")
        })
        .unwrap();
    assert_eq!(r.groups.len(), 1);
    assert_eq!(r.groups[0].hits.len(), 1);
    assert_eq!(
        (r.total_sessions, r.total_hits),
        (all.total_sessions, all.total_hits)
    );
    assert!(r.groups[0].hit_count > 1 || all.groups.iter().all(|g| g.hit_count == 1));
}

#[test]
fn subagent_hit_carries_agent_and_jumps_into_scope() {
    let env = indexed(&["subagents"]);
    let agent_file = env.file(&format!(
        "{}/{}/subagents/agent-a11b19d522e5e3835.jsonl",
        ids::SUBAGENTS_DIR,
        ids::SUBAGENTS
    ));
    let r = env.search("\"dolore et aliquip ut d\"");
    let hit = r
        .groups
        .iter()
        .flat_map(|g| &g.hits)
        .find(|h| h.agent_id.as_deref() == Some("a11b19d522e5e3835"))
        .expect("hit inside the subagent file");
    assert!(hit.on_main_line);
    assert_eq!(hit.agent_type.as_deref(), Some("codex:codex-rescue"));
    let t = env
        .engine
        .resolve_jump(&JumpRequest {
            session_id: ids::SUBAGENTS.to_owned(),
            node_id: hit.node_id.clone(),
            agent_id: hit.agent_id.clone(),
            tool_use_id: None,
        })
        .unwrap();
    assert!(t.in_subagent);
    assert_eq!(
        t.scope,
        TranscriptScope::Subagent {
            agent_id: "a11b19d522e5e3835".into()
        }
    );
    assert_eq!(t.agent_path, ["a11b19d522e5e3835"]);
    assert_eq!(t.node_id, uuid_with(&agent_file, "dolore et aliquip ut d"));

    // Nested run: agent path runs outermost → innermost.
    let nested_file = env.file(&format!(
        "{}/{}/subagents/agent-a1e2a06b87a0850ff.jsonl",
        ids::SUBAGENTS_DIR,
        ids::SUBAGENTS
    ));
    let first_uuid = uuid_with(&nested_file, "\"uuid\"");
    let t = env
        .engine
        .resolve_jump(&JumpRequest {
            session_id: ids::SUBAGENTS.to_owned(),
            node_id: first_uuid,
            agent_id: Some("a1e2a06b87a0850ff".into()),
            tool_use_id: None,
        })
        .unwrap();
    assert_eq!(
        t.agent_path,
        ["agui-impl-16f075d476ac2e52", "a1e2a06b87a0850ff"]
    );
}

#[test]
fn abandoned_branch_hit_is_off_main_line_and_jump_selects_it() {
    let env = indexed(&["branch"]);
    let main = env.file(&format!("{}/{}.jsonl", ids::BRANCH_DIR, ids::BRANCH));
    for (query, text) in [
        ("\"Now add unit tests\"", "Now add unit tests"),
        ("\"draft one\"", "Summary, draft one."),
    ] {
        let r = env.search(query);
        assert_eq!(r.total_hits, 1, "{query}");
        let hit = &r.groups[0].hits[0];
        assert!(!hit.on_main_line, "{query}");
        assert_eq!(hit.node_id, uuid_with(&main, text));
        let t = env
            .engine
            .resolve_jump(&JumpRequest {
                session_id: ids::BRANCH.to_owned(),
                node_id: hit.node_id.clone(),
                agent_id: None,
                tool_use_id: None,
            })
            .unwrap();
        assert!(t.in_abandoned_branch && !t.in_subagent && !t.needs_hidden);
        assert_eq!(t.branch_choices.len(), 1, "{query}");
        // The choices really select the target.
        let tr = env
            .engine
            .get_transcript(&TranscriptRequest {
                session_id: ids::BRANCH.to_owned(),
                scope: TranscriptScope::Main,
                branch_choices: t.branch_choices.clone(),
                include_hidden: false,
            })
            .unwrap();
        assert!(tr.nodes.iter().any(|n| n.id == t.node_id), "{query}");
    }
    // A Main Line hit needs no choice.
    let r = env.search("\"draft two\"");
    let hit = &r.groups[0].hits[0];
    assert!(hit.on_main_line);
    let t = env
        .engine
        .resolve_jump(&JumpRequest {
            session_id: ids::BRANCH.to_owned(),
            node_id: hit.node_id.clone(),
            agent_id: None,
            tool_use_id: None,
        })
        .unwrap();
    assert!(t.branch_choices.is_empty() && !t.in_abandoned_branch);
}

#[test]
fn jump_maps_absorbed_results_and_flags_hidden() {
    let env = indexed(&["basic"]);
    let main = env.file(&format!("{}/{}.jsonl", ids::BASIC_DIR, ids::BASIC));
    let jump = |node_id: String, tool: Option<&str>| {
        env.engine
            .resolve_jump(&JumpRequest {
                session_id: ids::BASIC.to_owned(),
                node_id,
                agent_id: None,
                tool_use_id: tool.map(str::to_owned),
            })
            .unwrap()
    };
    // The failed Bash result is absorbed by its tool call's assistant node.
    let result_uuid = uuid_with(&main, "cannot find value");
    let t = jump(result_uuid, None);
    assert_eq!(
        t.tool_use_id.as_deref(),
        Some("toolu_e2ccd9f8e29e53a88b3896")
    );
    assert_eq!(t.node_id, uuid_with(&main, "Running the tests."));
    assert!(!t.needs_hidden);
    // A meta prompt is hidden by default.
    let t = jump(
        uuid_with(&main, "The task tools have not been used recently."),
        None,
    );
    assert!(t.needs_hidden);
    // Unknown ids are NotFound.
    assert!(
        env.engine
            .resolve_jump(&JumpRequest {
                session_id: ids::BASIC.to_owned(),
                node_id: "nope".into(),
                agent_id: None,
                tool_use_id: None,
            })
            .is_err()
    );
}

#[test]
fn ask_answers_are_user_hits_that_jump_to_the_call() {
    let env = indexed(&["basic"]);
    let ask = "toolu_0118zFb2snzn5S8uBwkUiEUn";
    let r = env.search("consectetur adipis");
    let hits: Vec<_> = r
        .groups
        .iter()
        .flat_map(|g| &g.hits)
        .filter(|h| h.tool_use_id.as_deref() == Some(ask) && h.role == SearchRole::User)
        .collect();
    assert_eq!(hits.len(), 1);
    let t = env
        .engine
        .resolve_jump(&JumpRequest {
            session_id: ids::BASIC.to_owned(),
            node_id: hits[0].node_id.clone(),
            agent_id: None,
            tool_use_id: hits[0].tool_use_id.clone(),
        })
        .unwrap();
    assert_eq!(t.tool_use_id.as_deref(), Some(ask));
    // The call's message is displayed under its first fragment.
    assert_eq!(t.node_id, "e690b132-a297-4571-8770-7a7fe24e9456");
}

#[test]
fn append_makes_new_text_searchable_including_cjk_and_quotes() {
    let env = indexed(&["basic"]);
    let main =
        fs::canonicalize(env.file(&format!("{}/{}.jsonl", ids::BASIC_DIR, ids::BASIC))).unwrap();
    assert_eq!(env.search("重试").total_hits, 0);
    append_prompt(
        &main,
        "u-appended-1",
        &last_leaf(&main),
        ids::BASIC,
        r#"给重试逻辑加上 jitter，配置 key="retry" 保持不变"#,
    );
    env.engine
        .apply_changes(std::slice::from_ref(&main))
        .unwrap();
    env.engine
        .index_text_backlog(&|_| {}, &AtomicBool::new(false))
        .unwrap();

    let two = env.search("重试");
    assert_eq!(two.mode, SearchMode::Like);
    assert_eq!(two.total_hits, 1);
    let hit = &two.groups[0].hits[0];
    assert_eq!(hit.node_id, "u-appended-1");
    assert!(
        hit.on_main_line,
        "the appended prompt extends the Main Line"
    );
    assert!(hit.snippet.iter().any(|p| p.hit && p.text == "重试"));
    // Short terms with and without ASCII letters together, and as an exclusion.
    assert_eq!(env.search("JI 重试").total_hits, 1);
    assert_eq!(env.search("jitter -重试").total_hits, 0);
    assert_eq!(env.search("jitter -试重").total_hits, 1);

    let three = env.search("重试逻");
    assert_eq!(three.mode, SearchMode::Fts);
    assert_eq!(three.total_hits, 1);
    // Inner quotes are doubled in the FTS phrase.
    assert_eq!(env.search(r#"key="retry""#).total_hits, 1);
    assert_eq!(env.search(r#"key="retryx""#).total_hits, 0);
}

#[test]
fn backlog_commits_a_transaction_even_when_asked_to_yield() {
    let root = fixture_root("basic");
    let cache = tempfile::tempdir().unwrap();
    let engine = open_engine(root.path(), cache.path());
    engine.scan_all(&|_| {}).unwrap();
    let r = engine.search(&req("retry")).unwrap();
    assert!(!r.index_complete && r.total_hits == 0);
    // A yield request is honoured only after a commit, so the backlog never starves; the
    // fixture fits in one transaction, so it completes.
    engine
        .index_text_backlog(&|_| {}, &AtomicBool::new(true))
        .unwrap();
    let r = engine.search(&req("retry")).unwrap();
    assert!(r.index_complete && r.total_hits > 0);
    assert!(engine.index_status().text_ready);
}

// ------------------------------------------------------------------ ⌘F

#[test]
fn find_in_session_covers_text_inputs_and_full_outputs() {
    let env = indexed(&["persisted_and_images", "basic"]);
    let find = |sid: &str, query: &str| {
        env.engine
            .find_in_session(&FindRequest {
                session_id: sid.to_owned(),
                scope: TranscriptScope::Main,
                branch_choices: Vec::new(),
                include_hidden: false,
                query: query.to_owned(),
            })
            .unwrap()
    };
    // Only in the persisted file (beyond the inline preview).
    let r = find(ids::PERSISTED, "08:00:59.463");
    assert_eq!(r.total, 1);
    assert_eq!(
        r.matches[0].location,
        FindLocation::ToolOutput {
            tool_use_id: "toolu_01YSyvoRkw4Y1Uz3WMi4zHxr".into()
        }
    );
    // Prompt text, assistant text and a tool input (TodoWrite "Add retry loop"), case-insensitive.
    let r = find(ids::BASIC, "RETRY");
    let locs: Vec<&FindLocation> = r.matches.iter().map(|m| &m.location).collect();
    assert!(locs.contains(&&FindLocation::Text));
    assert!(
        locs.iter()
            .any(|l| matches!(l, FindLocation::ToolInput { .. }))
    );
    assert_eq!(r.total, r.matches.iter().map(|m| m.count).sum::<u32>());
    assert_eq!(find(ids::BASIC, "  ").total, 0);
}

// ------------------------------------------------------------------ real-root measurement

/// Full text index time, DB size and query latencies on a real data root (read-only, temp
/// cache). `CODECK_BENCH_ROOT=~/.claude cargo test -p codeck-core --release --test search -- --ignored --nocapture`
#[test]
#[ignore]
fn measure_real_root() {
    let root = std::env::var("CODECK_BENCH_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs_home().join(".claude"));
    // `CV_BENCH_CACHE` keeps the index for repeated query runs; default is a fresh temp dir.
    let tmp = tempfile::tempdir().unwrap();
    let cache_dir =
        std::env::var("CV_BENCH_CACHE").map_or_else(|_| tmp.path().to_path_buf(), PathBuf::from);
    let engine = Engine::open(EngineConfig {
        data_root: root.clone(),
        data_root_source: DataRootSource::Settings,
        cache_dir: cache_dir.clone(),
    })
    .unwrap();
    let t = std::time::Instant::now();
    engine.scan_all(&|_| {}).unwrap();
    println!("metadata scan: {:.2}s", t.elapsed().as_secs_f64());
    let db_path = cache_dir.join("index.sqlite");
    let size = |p: &Path| {
        ["", "-wal"]
            .iter()
            .map(|s| fs::metadata(format!("{}{s}", p.display())).map_or(0, |m| m.len()))
            .sum::<u64>() as f64
            / 1e6
    };
    println!("db after metadata: {:.1} MB", size(&db_path));
    let t = std::time::Instant::now();
    engine
        .index_text_backlog(&|_| {}, &AtomicBool::new(false))
        .unwrap();
    println!("text index: {:.2}s", t.elapsed().as_secs_f64());
    let db = Connection::open(&db_path).unwrap();
    db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    let (rows, bytes): (i64, i64) = db
        .query_row(
            "SELECT count(*), coalesce(sum(length(CAST(body AS BLOB))), 0) FROM msg_text",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    println!(
        "msg_text: {rows} rows, {:.1} MB text; db: {:.1} MB",
        bytes as f64 / 1e6,
        size(&db_path)
    );
    for q in [
        "error",
        "retry backoff",
        "\"cargo test\"",
        "重试",
        "搜索",
        "ok -error",
        "数据库连接",
    ] {
        let mut best = f64::MAX;
        let mut r = None;
        for _ in 0..3 {
            let resp = engine
                .search(&SearchRequest {
                    hits_per_session: 5,
                    ..req(q)
                })
                .unwrap();
            best = best.min(resp.elapsed_ms);
            r = Some(resp);
        }
        let r = r.unwrap();
        println!(
            "query {q:<16} mode {:?}: {:.1} ms, {} hits in {} sessions",
            r.mode, best, r.total_hits, r.total_sessions
        );
    }
}

fn dirs_home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap())
}
