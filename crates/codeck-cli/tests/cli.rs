//! The read-only commands, through the binary, over a fixture's index.

#[path = "../../codeck-core/tests/common/mod.rs"]
mod common;

use std::fs;
use std::process::{Command, Output};

use common::engine::{Env, data_root};
use common::ids;
use serde_json::Value;

fn indexed(scenarios: &[&str]) -> Env {
    Env::indexed(data_root(scenarios))
}

/// Runs `codeck --cache-dir <cache> <args>` over `env`; `HOME` is a temp dir so the machine's
/// settings cannot move the data root.
fn run(env: &Env, args: &[&str]) -> Output {
    let home = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_codeck"))
        .env("HOME", home.path())
        .env("CLAUDE_CONFIG_DIR", env.root.path())
        .arg("--cache-dir")
        .arg(env.cache.path())
        .args(args)
        .output()
        .unwrap()
}

fn json(env: &Env, args: &[&str]) -> Value {
    let out = run(env, args);
    assert!(out.status.success(), "{args:?}: {out:?}");
    serde_json::from_slice(&out.stdout).unwrap()
}

fn text(env: &Env, args: &[&str]) -> String {
    let out = run(env, args);
    assert!(out.status.success(), "{args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap()
}

fn hits(v: &Value) -> Vec<&Value> {
    v["results"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["hits"].as_array().unwrap())
        .collect()
}

#[test]
fn a_search_hit_on_the_main_line_names_the_turn_show_prints() {
    let env = indexed(&["branch"]);
    let found = json(&env, &["search", "\"Add property tests instead\""]);
    assert!(found["indexedAt"].is_string());
    assert_eq!(found["results"][0]["sessionId"], ids::BRANCH);
    let hit = hits(&found)[0];
    assert_eq!(hit["location"], "mainLine");
    let turn = hit["turn"].as_u64().unwrap();
    assert_eq!(turn, 3);

    let page = text(&env, &["show", ids::BRANCH, "--turn", &turn.to_string()]);
    assert!(
        page.contains("[3] 用户：Add property tests instead"),
        "{page}"
    );
    assert!(page.contains("turns="));
    assert!(!page.contains("Handle timezone offsets too"));
}

#[test]
fn turn_numbers_follow_the_main_line_and_skip_a_rolled_back_branch() {
    let env = indexed(&["branch"]);
    let page = text(&env, &["show", ids::BRANCH]);
    assert!(
        page.contains("[1] 用户：Write a function that parses ISO dates"),
        "{page}"
    );
    assert!(page.contains("[2] 用户：Handle timezone offsets too"));
    assert!(page.contains("[3] 用户：Add property tests instead"));
    assert!(!page.contains("Now add unit tests"));

    let found = json(&env, &["search", "\"Now add unit tests\""]);
    let hit = hits(&found)[0];
    assert_eq!(hit["location"], "branch");
    assert!(hit["turn"].is_null());
}

#[test]
fn a_hit_inside_a_subagent_run_has_no_turn_and_names_the_run() {
    let env = indexed(&["subagents"]);
    let found = json(&env, &["search", "\"dolore et aliquip ut d\""]);
    let hit = hits(&found)
        .into_iter()
        .find(|h| h["agentId"] == "a11b19d522e5e3835")
        .expect("a hit in the Subagent Run");
    assert_eq!(hit["location"], "subagent");
    assert!(hit["turn"].is_null());
    let page = text(
        &env,
        &["show", ids::SUBAGENTS, "--agent", "a11b19d522e5e3835"],
    );
    assert!(page.contains("agent=a11b19d522e5e3835"), "{page}");
    assert!(page.contains("[1] 任务："), "{page}");
}

#[test]
fn long_text_is_cut_and_says_so() {
    let env = indexed(&["basic"]);
    let sid = "22222222-3333-4444-8555-666666666666";
    let dir = env.root.path().join("projects").join(ids::BASIC_DIR);
    let long = "长".repeat(5000);
    let lines = [
        serde_json::json!({"parentUuid":null,"type":"user","uuid":"u1","sessionId":sid,
            "timestamp":"2026-06-03T09:00:04.000Z","origin":{"kind":"human"},"cwd":"/w",
            "message":{"role":"user","content":long}}),
        serde_json::json!({"parentUuid":"u1","type":"assistant","uuid":"a1","sessionId":sid,
            "timestamp":"2026-06-03T09:00:05.000Z","cwd":"/w",
            "message":{"id":"m1","role":"assistant","content":[{"type":"text","text":long}]}}),
    ]
    .map(|v| v.to_string())
    .join("\n");
    fs::write(dir.join(format!("{sid}.jsonl")), format!("{lines}\n")).unwrap();

    // Not indexed yet: read from its file, and said to be partial.
    let page = text(&env, &["show", sid]);
    let first = page.lines().next().unwrap();
    assert!(first.ends_with("partial"), "{first}");
    assert!(!first.contains("project="), "{first}");
    assert!(page.contains("…（已截断，共 5000 字）"), "{page}");
    assert!(page.len() < long.len());
    // `--turn` asks for the whole turn: longer limit, nothing cut at this size.
    let page = text(&env, &["show", sid, "--turn", "1"]);
    assert!(!page.contains("已截断"), "cut at 5000 chars");
}

#[test]
fn an_unknown_project_or_turn_is_one_json_line_on_stderr() {
    let env = indexed(&["basic"]);
    for (args, code) in [
        (vec!["sessions", "--project", "/no/such"], "not_found"),
        (vec!["show", ids::BASIC, "--turn", "99"], "not_found"),
        (vec!["show", ids::BASIC, "--full"], "usage"),
        (vec!["nonsense"], "usage"),
    ] {
        let out = run(&env, &args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        let err: Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(err["code"], code, "{args:?}");
        assert!(err["message"].as_str().is_some_and(|m| !m.contains('\n')));
    }
}

#[test]
fn sessions_lists_the_index_newest_first_with_a_limit() {
    let env = indexed(&["basic", "branch"]);
    let all = json(&env, &["sessions"]);
    assert_eq!(all["total"], 2);
    assert!(all["indexedAt"].is_string());
    assert_eq!(all["sessions"].as_array().unwrap().len(), 2);
    assert_eq!(all["sessions"][0]["status"], "ended");
    let one = json(&env, &["sessions", "--limit", "1"]);
    assert_eq!(one["total"], 2);
    assert_eq!(one["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(
        one["sessions"][0]["sessionId"],
        all["sessions"][0]["sessionId"]
    );
}

#[test]
fn a_full_turn_includes_tool_input_and_output() {
    let env = indexed(&["basic"]);
    // The first prompt runs tools.
    let page = text(&env, &["show", ids::BASIC, "--turn", "1", "--full"]);
    assert!(page.contains("--- 工具 1："), "{page}");
    assert!(page.contains("输入：") && page.contains("输出："), "{page}");
}
