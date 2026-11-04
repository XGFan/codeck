//! API cost: per-message pricing in the index, session / model / stats totals, sorting, and
//! re-pricing when a new price list arrives. Expected values are worked out by hand from the
//! LiteLLM rates of `claude-sonnet-4-6` ($3 / $15 / $0.30 read / $3.75 5m write / $6 1h write per
//! MTok).
mod common;

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use codeck_core::model::{DataRootSource, SessionQuery, SessionSort, StatsRequest};
use codeck_core::{Engine, EngineConfig};
use common::{fixture_root, ids};
use tempfile::TempDir;

fn open(root: &Path, cache: &Path) -> Engine {
    Engine::open(EngineConfig {
        data_root: root.to_path_buf(),
        data_root_source: DataRootSource::Settings,
        cache_dir: cache.to_path_buf(),
    })
    .expect("open engine")
}

fn assistant(uuid: &str, msg_id: &str, model: &str, usage: &str) -> String {
    format!(
        r#"{{"type":"assistant","uuid":"{uuid}","parentUuid":null,"timestamp":"2026-10-07T10:00:01.000Z","sessionId":"{sid}","message":{{"id":"{msg_id}","role":"assistant","model":"{model}","content":[{{"type":"text","text":"ok"}}],"usage":{usage}}}}}"#,
        sid = ids::BASIC
    ) + "\n"
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// Basic fixture plus known messages appended; returns the engine and the added cost.
fn with_known_messages() -> (TempDir, TempDir, Engine, f64) {
    let root = fixture_root("basic");
    let cache = tempfile::tempdir().unwrap();
    let engine = open(root.path(), cache.path());
    engine.scan_all(&|_| {}).unwrap();
    let before = engine.get_session(ids::BASIC).unwrap().summary.cost;

    let path = root
        .path()
        .join("projects")
        .join(ids::BASIC_DIR)
        .join(format!("{}.jsonl", ids::BASIC));
    let usage = r#"{"input_tokens":1000,"output_tokens":1000,"cache_read_input_tokens":10000,"cache_creation_input_tokens":2000,"cache_creation":{"ephemeral_5m_input_tokens":1000,"ephemeral_1h_input_tokens":1000}}"#;
    let mut f = OpenOptions::new().append(true).open(&path).unwrap();
    // Two fragments of one reply (same message id) count once; an unknown model is unpriced.
    f.write_all(assistant("c-1", "msg-cost", "claude-sonnet-4-6", usage).as_bytes())
        .unwrap();
    f.write_all(assistant("c-2", "msg-cost", "claude-sonnet-4-6", usage).as_bytes())
        .unwrap();
    f.write_all(assistant("c-3", "msg-nope", "claude-nope-1", r#"{"input_tokens":5}"#).as_bytes())
        .unwrap();
    drop(f);
    engine.apply_changes(&[path]).unwrap();
    // 1000×3 + 1000×15 + 10000×0.30 + 1000×3.75 + 1000×6, per MTok.
    (root, cache, engine, before + 0.030_75)
}

#[test]
fn session_cost_counts_each_reply_once_and_flags_unpriced_models() {
    let (_root, _cache, engine, expected) = with_known_messages();
    let d = engine.get_session(ids::BASIC).unwrap();
    assert!(
        close(d.summary.cost, expected),
        "{} vs {expected}",
        d.summary.cost
    );
    assert_eq!(d.summary.unpriced_messages, 1);

    let sonnet = d
        .model_costs
        .iter()
        .find(|m| m.model == "claude-sonnet-4-6")
        .unwrap();
    assert!(sonnet.cost >= 0.030_75 - 1e-9);
    let nope = d
        .model_costs
        .iter()
        .find(|m| m.model == "claude-nope-1")
        .unwrap();
    assert_eq!((nope.cost, nope.unpriced_messages), (0.0, 1));
    let total: f64 = d.model_costs.iter().map(|m| m.cost).sum();
    assert!(close(total, d.summary.cost));
}

#[test]
fn a_new_price_list_reprices_and_is_cached() {
    let (root, cache, engine, _) = with_known_messages();
    let cost_of = |e: &Engine, model: &str| {
        let d = e.get_session(ids::BASIC).unwrap();
        let m = d
            .model_costs
            .iter()
            .find(|m| m.model == model)
            .cloned()
            .unwrap();
        (m.cost, m.unpriced_messages, d.summary.unpriced_messages)
    };
    let (sonnet_before, _, unpriced_before) = cost_of(&engine, "claude-sonnet-4-6");
    let doubled = r#"{
        "claude-sonnet-4-6": {
            "input_cost_per_token": 6e-6, "output_cost_per_token": 3e-5,
            "cache_read_input_token_cost": 6e-7, "cache_creation_input_token_cost": 7.5e-6,
            "cache_creation_input_token_cost_above_1hr": 1.2e-5
        },
        "claude-nope-1": { "input_cost_per_token": 1e-3, "output_cost_per_token": 1e-3 }
    }"#;
    assert!(engine.update_prices(doubled).unwrap());
    // Every Sonnet rate doubled; `nope` is priced now (5 input tokens × $1e-3).
    let (sonnet_after, _, unpriced_after) = cost_of(&engine, "claude-sonnet-4-6");
    assert!(close(sonnet_after, 2.0 * sonnet_before));
    let (nope, nope_unpriced, _) = cost_of(&engine, "claude-nope-1");
    assert!(close(nope, 0.005));
    assert_eq!(nope_unpriced, 0);
    assert!(
        !engine.update_prices(doubled).unwrap(),
        "same table: nothing to do"
    );
    // The fixture's other models are missing from this list, so they become unpriced.
    let d = engine.get_session(ids::BASIC).unwrap();
    let others: Vec<_> = d
        .model_costs
        .iter()
        .filter(|m| m.model != "claude-sonnet-4-6" && m.model != "claude-nope-1")
        .collect();
    assert!(!others.is_empty());
    assert!(
        others
            .iter()
            .all(|m| m.cost == 0.0 && m.unpriced_messages > 0)
    );
    let unpriced: u32 = others.iter().map(|m| m.unpriced_messages).sum();
    assert_eq!((unpriced_before, unpriced_after), (1, unpriced));

    // A fresh engine starts from the cached list.
    drop(engine);
    let again = open(root.path(), cache.path());
    let d2 = again.get_session(ids::BASIC).unwrap();
    assert!(close(d2.summary.cost, d.summary.cost));
    assert!(again.update_prices("not json").is_err());
}

#[test]
fn list_sorts_by_cost_and_stats_sum_it() {
    let root = fixture_root("subagents");
    let cache = tempfile::tempdir().unwrap();
    let engine = open(root.path(), cache.path());
    engine.scan_all(&|_| {}).unwrap();

    let list = engine
        .list_sessions(&SessionQuery {
            sort: SessionSort::Cost,
            descending: true,
            ..SessionQuery::default()
        })
        .unwrap();
    assert!(!list.is_empty());
    assert!(list.windows(2).all(|w| w[0].cost >= w[1].cost));

    // The session total includes its Subagents' messages.
    let d = engine.get_session(ids::SUBAGENTS).unwrap();
    assert!(d.tokens_subagents.output > 0.0);
    let stats = engine.stats(&StatsRequest::default()).unwrap();
    let total: f64 = list.iter().map(|s| s.cost).sum();
    assert!(close(stats.overview.cost, total));
    // The ranking's per-session cost is the session's own (all time, so the whole session).
    assert!(stats.sessions.windows(2).all(|w| w[0].cost >= w[1].cost));
    for r in &stats.sessions {
        let s = list.iter().find(|s| s.id == r.session_id).unwrap();
        assert!(close(r.cost, s.cost), "{}", r.session_id);
    }
    assert!(d.summary.cost > 0.0);
}

/// Distinct assistant messages of the given session files, as (key, output tokens): fragments of
/// one reply share `message.id` and keep the max, mirroring `messages`.
fn distinct_output(files: &[&Path]) -> std::collections::HashMap<String, f64> {
    let mut out = std::collections::HashMap::new();
    for f in files {
        for line in std::fs::read_to_string(f).unwrap().lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap_or_default();
            if v["type"] != "assistant" {
                continue;
            }
            let key = v["message"]["id"]
                .as_str()
                .or(v["uuid"].as_str())
                .unwrap()
                .to_owned();
            let o = v["message"]["usage"]["output_tokens"]
                .as_f64()
                .unwrap_or(0.0);
            let e = out.entry(key).or_insert(0.0f64);
            *e = e.max(o);
        }
    }
    out
}

#[test]
fn a_fork_is_billed_only_for_its_own_messages_until_the_origin_is_gone() {
    let root = fixture_root("fork");
    let cache = tempfile::tempdir().unwrap();
    let engine = open(root.path(), cache.path());
    engine.scan_all(&|_| {}).unwrap();
    let dir = root.path().join("projects").join(ids::FORK_DIR);
    let origin_file = dir.join(format!("{}.jsonl", ids::FORK_ORIGIN));
    let child_file = dir.join(format!("{}.jsonl", ids::FORK_CHILD));
    let origin_msgs = distinct_output(&[&origin_file]);
    let child_msgs = distinct_output(&[&child_file]);
    let own: f64 = child_msgs
        .iter()
        .filter(|(k, _)| !origin_msgs.contains_key(*k))
        .map(|(_, o)| o)
        .sum();
    assert!(
        child_msgs.keys().any(|k| origin_msgs.contains_key(k)),
        "the child inherits messages"
    );

    let out = |id: &str| engine.get_session(id).unwrap().summary.tokens.output;
    assert_eq!(out(ids::FORK_ORIGIN), origin_msgs.values().sum::<f64>());
    assert_eq!(out(ids::FORK_CHILD), own);
    let stats = engine.stats(&StatsRequest::default()).unwrap();
    assert_eq!(stats.overview.output_tokens, out(ids::FORK_ORIGIN) + own);

    // Without the origin, the child is the earliest holder of the inherited messages.
    std::fs::remove_file(&origin_file).unwrap();
    let cs = engine.apply_changes(&[origin_file]).unwrap();
    assert!(cs.changed.contains(&ids::FORK_CHILD.to_owned()), "{cs:?}");
    assert_eq!(out(ids::FORK_CHILD), child_msgs.values().sum::<f64>());
}
