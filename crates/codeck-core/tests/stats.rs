//! Stats: totals, requests, local-day / hourly bucketing and drill-down on fixture copies
//! through the Engine. Expected values come from an independent oracle that reads the fixture
//! JSONL directly (README counting rules: assistant messages by `message.id`, max usage per id).
mod common;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{DateTime, FixedOffset, Local, NaiveDateTime, TimeZone};
use codeck_core::Engine;
use codeck_core::model::{Drill, SessionQuery, SessionSort, Stats, StatsRequest, TimeRange};
use common::engine::data_root;
use common::ids;
use serde_json::Value;
use tempfile::TempDir;

const MIXED: &[&str] = &[
    "basic",
    "branch",
    "subagents",
    "workflow",
    "persisted_and_images",
];

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

struct Env {
    _root: TempDir,
    _cache: TempDir,
    engine: Engine,
    oracle: Oracle,
}

fn scanned(scenarios: &[&str]) -> Env {
    let common::engine::Env {
        root,
        cache,
        engine,
    } = common::engine::Env::scanned(data_root(scenarios));
    let oracle = Oracle::read(root.path());
    Env {
        _root: root,
        _cache: cache,
        engine,
        oracle,
    }
}

fn all_time(env: &Env) -> Stats {
    env.engine.stats(&StatsRequest::default()).unwrap()
}

fn drill_ids(engine: &Engine, drill: Drill, time_range: Option<TimeRange>) -> BTreeSet<String> {
    engine
        .list_sessions(&SessionQuery {
            drill: Some(drill),
            time_range,
            ..SessionQuery::default()
        })
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect()
}

// ---------------------------------------------------------------- oracle

#[derive(Clone, Debug)]
struct Msg {
    ts: i64,
    main: bool,
    assistant: bool,
    model: Option<String>,
    tokens: [i64; 4],
}

#[derive(Default, Debug)]
struct Sess {
    msgs: Vec<Msg>,
    /// tool_use id → (name, ts)
    tools: BTreeMap<String, (String, i64)>,
    errors: HashSet<String>,
    agent_types: Vec<String>,
}

#[derive(Default, Debug)]
struct Oracle {
    sessions: BTreeMap<String, Sess>,
}

fn ts_of(e: &Value) -> Option<i64> {
    let t = e.get("timestamp")?.as_str()?;
    Some(DateTime::parse_from_rfc3339(t).ok()?.timestamp_millis())
}

fn is_human_prompt(e: &Value) -> bool {
    if e["type"] != "user" || e["isMeta"] == true || e["isCompactSummary"] == true {
        return false;
    }
    if let Some(k) = e["origin"]["kind"].as_str()
        && k != "human"
    {
        return false;
    }
    let content = &e["message"]["content"];
    let text = match content {
        Value::String(s) => Some(s.as_str()),
        Value::Array(blocks) => {
            if blocks.iter().any(|b| b["type"] == "tool_result") {
                return false;
            }
            if blocks.is_empty() {
                return false;
            }
            blocks
                .iter()
                .find(|b| b["type"] == "text")
                .and_then(|b| b["text"].as_str())
        }
        _ => return false,
    };
    text.is_none_or(|t| {
        let t = t.trim_start();
        !(t.starts_with("<local-command")
            || t.starts_with("<system-reminder")
            || t.starts_with("<teammate-message")
            || t.starts_with("Another Claude session sent a message:"))
    })
}

impl Oracle {
    fn read(root: &Path) -> Oracle {
        let mut o = Oracle::default();
        for dir in fs::read_dir(root.join("projects")).unwrap() {
            let dir = dir.unwrap().path();
            for f in fs::read_dir(&dir).unwrap() {
                let p = f.unwrap().path();
                if p.extension().is_some_and(|e| e == "jsonl") {
                    let sid = p.file_stem().unwrap().to_string_lossy().to_string();
                    let sess = o.sessions.entry(sid.clone()).or_default();
                    read_file(&p, true, sess);
                    let mut agents = Vec::new();
                    walk(&dir.join(&sid).join("subagents"), &mut agents);
                    for a in agents {
                        let name = a.file_name().unwrap().to_string_lossy().to_string();
                        if name.ends_with(".meta.json") {
                            let meta: Value =
                                serde_json::from_str(&fs::read_to_string(&a).unwrap()).unwrap();
                            sess.agent_types
                                .push(meta["agentType"].as_str().unwrap_or("unknown").to_owned());
                        } else if name.starts_with("agent-") && name.ends_with(".jsonl") {
                            read_file(&a, false, sess);
                        }
                    }
                }
            }
        }
        o
    }

    fn main_msgs(&self) -> impl Iterator<Item = (&String, &Msg)> {
        self.sessions
            .iter()
            .flat_map(|(id, s)| s.msgs.iter().map(move |m| (id, m)))
            .filter(|(_, m)| m.main)
    }

    fn all_msgs(&self) -> impl Iterator<Item = (&String, &Msg)> {
        self.sessions
            .iter()
            .flat_map(|(id, s)| s.msgs.iter().map(move |m| (id, m)))
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd {
        let p = e.unwrap().path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// One file's messages (prompts by uuid, assistant messages by `message.id` with max usage),
/// tool calls and failed results.
fn read_file(path: &Path, main: bool, sess: &mut Sess) {
    let mut by_key: BTreeMap<String, usize> = BTreeMap::new();
    let mut msgs: Vec<Msg> = Vec::new();
    for line in fs::read_to_string(path).unwrap().lines() {
        let Ok(e) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let ts = ts_of(&e);
        if e["type"] == "assistant" {
            if let Some(blocks) = e["message"]["content"].as_array() {
                for b in blocks.iter().filter(|b| b["type"] == "tool_use") {
                    sess.tools
                        .entry(b["id"].as_str().unwrap().to_owned())
                        .or_insert((
                            b["name"].as_str().unwrap().to_owned(),
                            ts.unwrap_or_default(),
                        ));
                }
            }
            let Some(ts) = ts else { continue };
            let key = e["message"]["id"]
                .as_str()
                .or(e["uuid"].as_str())
                .unwrap()
                .to_owned();
            let u = &e["message"]["usage"];
            let tok = [
                "input_tokens",
                "output_tokens",
                "cache_read_input_tokens",
                "cache_creation_input_tokens",
            ]
            .map(|k| u[k].as_i64().unwrap_or(0));
            let model = e["message"]["model"].as_str().map(str::to_owned);
            match by_key.get(&key) {
                Some(&i) => {
                    for (a, b) in msgs[i].tokens.iter_mut().zip(tok) {
                        *a = (*a).max(b);
                    }
                    if msgs[i].model.is_none() {
                        msgs[i].model = model;
                    }
                }
                None => {
                    by_key.insert(key, msgs.len());
                    msgs.push(Msg {
                        ts,
                        main,
                        assistant: true,
                        model,
                        tokens: tok,
                    });
                }
            }
        } else if e["type"] == "user" {
            if let Some(blocks) = e["message"]["content"].as_array() {
                for b in blocks.iter().filter(|b| b["type"] == "tool_result") {
                    if b["is_error"] == true {
                        sess.errors
                            .insert(b["tool_use_id"].as_str().unwrap().to_owned());
                    }
                }
            }
            if let (Some(ts), Some(uuid)) = (ts, e["uuid"].as_str())
                && is_human_prompt(&e)
                && !by_key.contains_key(uuid)
            {
                by_key.insert(uuid.to_owned(), msgs.len());
                msgs.push(Msg {
                    ts,
                    main,
                    assistant: false,
                    model: None,
                    tokens: [0; 4],
                });
            }
        }
    }
    sess.msgs.extend(msgs);
}

fn local(ts: i64) -> NaiveDateTime {
    Local.timestamp_millis_opt(ts).unwrap().naive_local()
}

fn day_in(ts: i64, off: &FixedOffset) -> String {
    off.timestamp_millis_opt(ts)
        .unwrap()
        .date_naive()
        .to_string()
}

fn has_tokens(m: &Msg) -> bool {
    m.assistant && m.tokens.iter().any(|&t| t > 0)
}

// ---------------------------------------------------------------- totals

#[test]
fn oracle_matches_readme_counts() {
    let env = scanned(&["basic"]);
    let s = &env.oracle.sessions[ids::BASIC];
    assert_eq!(s.msgs.len(), 20, "4 human prompts + 16 assistant msg ids");
    let out: i64 = s.msgs.iter().map(|m| m.tokens[1]).sum();
    assert_eq!(out, 5151);
    assert_eq!(s.tools.len(), 18);
    assert_eq!(s.errors.len(), 1);
}

#[test]
fn tokens_counted_once_per_message_id() {
    let env = scanned(&["basic"]);
    let st = all_time(&env);
    // README: split fragments repeat the same usage; max per message.id gives 51/5151/1761742/33071.
    assert_eq!(st.overview.output_tokens, 5151.0);
    let expected_reqs = env.oracle.sessions[ids::BASIC]
        .msgs
        .iter()
        .filter(|m| has_tokens(m))
        .count() as u32;
    assert_eq!(st.overview.requests, expected_reqs);
    assert_eq!(
        st.daily.iter().map(|d| d.requests).sum::<u32>(),
        st.overview.requests
    );
    let sum =
        |f: fn(&codeck_core::model::DailyModelTokens) -> f64| st.daily.iter().map(f).sum::<f64>();
    assert_eq!(sum(|d| d.input), 51.0);
    assert_eq!(sum(|d| d.output), 5151.0);
    assert_eq!(sum(|d| d.cache_read), 1_761_742.0);
    assert_eq!(sum(|d| d.cache_creation), 33_071.0);
}

#[test]
fn totals_match_oracle() {
    let env = scanned(MIXED);
    let o = &env.oracle;
    let st = all_time(&env);

    // Tokens and requests: main + subagents.
    let all_req = o.all_msgs().filter(|(_, m)| has_tokens(m)).count();
    let main_req = o.main_msgs().filter(|(_, m)| has_tokens(m)).count();
    assert!(all_req > main_req, "subagent requests are included");
    assert_eq!(st.overview.requests as usize, all_req);
    assert_eq!(
        st.daily.iter().map(|d| d.requests).sum::<u32>(),
        st.overview.requests
    );
    assert!(close(
        st.daily.iter().map(|d| d.cost).sum::<f64>(),
        st.overview.cost
    ));
    assert!(st.hourly.is_empty(), "unbounded query emits no hourly");

    let out: i64 = o.all_msgs().map(|(_, m)| m.tokens[1]).sum();
    let main_out: i64 = o.main_msgs().map(|(_, m)| m.tokens[1]).sum();
    assert!(out > main_out, "subagent tokens are included");
    assert_eq!(st.overview.output_tokens, out as f64);
    assert_eq!(st.daily.iter().map(|d| d.output).sum::<f64>(), out as f64);

    // Daily tokens and requests by (local day, model).
    let mut daily: BTreeMap<(String, String), ([i64; 4], u32)> = BTreeMap::new();
    for (_, m) in o.all_msgs().filter(|(_, m)| has_tokens(m)) {
        let key = (local(m.ts).date().to_string(), m.model.clone().unwrap());
        let e = daily.entry(key).or_default();
        for (a, b) in e.0.iter_mut().zip(m.tokens) {
            *a += b;
        }
        e.1 += 1;
    }
    let got: BTreeMap<(String, String), ([i64; 4], u32)> = st
        .daily
        .iter()
        .map(|d| {
            (
                (d.day.clone(), d.model.clone()),
                (
                    [d.input, d.output, d.cache_read, d.cache_creation].map(|v| v as i64),
                    d.requests,
                ),
            )
        })
        .collect();
    assert_eq!(got, daily);
}

#[test]
fn time_range_filter() {
    let env = scanned(MIXED);
    // 2026-06-09 UTC: only the subagents session's main file and its orphan agent.
    let from = DateTime::parse_from_rfc3339("2026-06-09T00:00:00Z")
        .unwrap()
        .timestamp_millis();
    let to = from + 86_400_000 - 1;
    let range = TimeRange {
        from_ms: Some(from as f64),
        to_ms: Some(to as f64),
    };
    let st = env
        .engine
        .stats(&StatsRequest {
            time_range: Some(range),
        })
        .unwrap();
    let s = &env.oracle.sessions[ids::SUBAGENTS];
    let in_range = |m: &&Msg| m.ts >= from && m.ts <= to;
    let reqs = s
        .msgs
        .iter()
        .filter(|m| has_tokens(m))
        .filter(in_range)
        .count();
    assert_eq!(st.overview.requests as usize, reqs);
    assert_eq!(
        st.daily.iter().map(|d| d.requests).sum::<u32>(),
        st.overview.requests
    );
    assert_eq!(
        st.hourly.iter().map(|h| h.requests).sum::<u32>(),
        st.overview.requests
    );
    assert!(close(
        st.hourly.iter().map(|h| h.cost).sum::<f64>(),
        st.overview.cost
    ));
    let out: i64 = s.msgs.iter().filter(in_range).map(|m| m.tokens[1]).sum();
    assert_eq!(st.overview.output_tokens, out as f64);
}

/// Child half of `local_day_follows_tz`: prints per-day request counts under the inherited `TZ`.
#[test]
#[ignore = "spawned by local_day_follows_tz with TZ set"]
fn tz_probe() {
    if std::env::var_os("CV_TZ_PROBE").is_none() {
        return;
    }
    let env = scanned(MIXED);
    let st = all_time(&env);
    let mut by_day: BTreeMap<String, u32> = BTreeMap::new();
    for d in &st.daily {
        *by_day.entry(d.day.clone()).or_default() += d.requests;
    }
    let days: Vec<String> = by_day
        .into_iter()
        .map(|(day, reqs)| format!("{day}={reqs}"))
        .collect();
    println!("TZ_PROBE {}", days.join(","));
}

fn probe(tz: &str) -> BTreeMap<String, u32> {
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "tz_probe", "--ignored", "--nocapture"])
        .env("CV_TZ_PROBE", "1")
        .env("TZ", tz)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let line = text
        .lines()
        .find_map(|l| l.strip_prefix("TZ_PROBE "))
        .expect("probe output");
    line.split(',')
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap();
            (k.to_owned(), v.parse().unwrap())
        })
        .collect()
}

#[test]
fn local_day_follows_tz() {
    let env = scanned(MIXED);
    for (tz, secs) in [("Etc/GMT-14", 14 * 3600), ("Etc/GMT+10", -10 * 3600)] {
        let off = FixedOffset::east_opt(secs).unwrap();
        let mut expected: BTreeMap<String, u32> = BTreeMap::new();
        for (_, m) in env.oracle.all_msgs().filter(|(_, m)| has_tokens(m)) {
            *expected.entry(day_in(m.ts, &off)).or_default() += 1;
        }
        assert_eq!(probe(tz), expected, "{tz}");
    }
    // UTC+14 and UTC-10 are 24 h apart: every request moves to a different day.
    assert_ne!(probe("Etc/GMT-14"), probe("Etc/GMT+10"));
}

// ---------------------------------------------------------------- drill-down

#[test]
fn drill_down_returns_exactly_the_contributing_sessions() {
    let env = scanned(MIXED);
    let o = &env.oracle;
    let st = all_time(&env);
    let set = |f: &dyn Fn(&Sess) -> bool| -> BTreeSet<String> {
        o.sessions
            .iter()
            .filter(|(_, s)| f(s))
            .map(|(id, _)| id.clone())
            .collect()
    };

    let days: BTreeSet<&String> = st.daily.iter().map(|d| &d.day).collect();
    assert!(days.len() >= 5);
    for day in days {
        let want = set(&|s| {
            s.msgs
                .iter()
                .any(|m| has_tokens(m) && local(m.ts).date().to_string() == *day)
        });
        assert_eq!(
            drill_ids(&env.engine, Drill::Day { day: day.clone() }, None),
            want,
            "day {day}"
        );
    }

    let models: Vec<String> = st
        .daily
        .iter()
        .map(|d| d.model.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    assert!(models.len() >= 2);
    for model in &models {
        let want = set(&|s| {
            s.msgs
                .iter()
                .any(|m| has_tokens(m) && m.model.as_ref() == Some(model))
        });
        let drill = Drill::Model {
            models: vec![model.clone()],
        };
        assert_eq!(drill_ids(&env.engine, drill, None), want, "model {model}");
    }
    // Multi-model drill returns the union; empty model list matches nothing.
    let pair = vec![models[0].clone(), models[1].clone()];
    let want_pair = set(&|s| {
        s.msgs
            .iter()
            .any(|m| has_tokens(m) && m.model.as_ref().is_some_and(|name| pair.contains(name)))
    });
    assert_eq!(
        drill_ids(&env.engine, Drill::Model { models: pair }, None),
        want_pair
    );
    assert!(
        drill_ids(&env.engine, Drill::Model { models: Vec::new() }, None).is_empty(),
        "empty model list matches no session"
    );
    // Blank items are ignored; duplicates and >64 unique models are deduplicated and capped at 64.
    assert!(
        drill_ids(
            &env.engine,
            Drill::Model {
                models: vec![String::new(), String::new()]
            },
            None
        )
        .is_empty(),
        "blank model items match no session"
    );
    let mut noisy = vec![String::new(), models[0].clone(), models[0].clone()];
    for i in 0..80 {
        noisy.push(format!("nonexistent-model-{i}"));
    }
    // `models[1]` appears past the 64th unique model so it is dropped by the cap.
    noisy.push(models[1].clone());
    assert_eq!(
        drill_ids(&env.engine, Drill::Model { models: noisy }, None),
        drill_ids(
            &env.engine,
            Drill::Model {
                models: vec![models[0].clone()]
            },
            None
        )
    );
    let dedup_pair = vec![
        String::new(),
        models[0].clone(),
        models[1].clone(),
        models[0].clone(),
        models[1].clone(),
    ];
    assert_eq!(
        drill_ids(&env.engine, Drill::Model { models: dedup_pair }, None),
        want_pair
    );
}

#[test]
fn drill_down_respects_the_time_range() {
    let env = scanned(MIXED);
    // Subagent rows of the subagents session run on 2026-07-12 while its main file is on 06-09.
    let from = DateTime::parse_from_rfc3339("2026-07-12T00:00:00Z")
        .unwrap()
        .timestamp_millis() as f64;
    let range = Some(TimeRange {
        from_ms: Some(from),
        to_ms: Some(from + 86_400_000.0),
    });
    let st = env
        .engine
        .stats(&StatsRequest {
            time_range: range.clone(),
        })
        .unwrap();
    assert!(st.overview.requests > 0);
    assert!(st.overview.output_tokens > 0.0);
    // Subagent-only requests still drill down on both Day and Model.
    for day in st
        .daily
        .iter()
        .map(|d| d.day.clone())
        .collect::<BTreeSet<_>>()
    {
        assert_eq!(
            drill_ids(&env.engine, Drill::Day { day }, range.clone()),
            BTreeSet::from([ids::SUBAGENTS.to_owned()])
        );
    }
    for model in st
        .daily
        .iter()
        .map(|d| d.model.clone())
        .collect::<BTreeSet<_>>()
    {
        assert_eq!(
            drill_ids(
                &env.engine,
                Drill::Model {
                    models: vec![model]
                },
                range.clone()
            ),
            BTreeSet::from([ids::SUBAGENTS.to_owned()])
        );
    }
    // A day outside the range yields nothing.
    assert!(
        drill_ids(
            &env.engine,
            Drill::Day {
                day: "2026-06-02".into()
            },
            range
        )
        .is_empty()
    );
}

/// Asserts that `Drill::Day` and `Drill::Model` return the exact set of visible sessions derived
/// from contributing billed rows, covering:
/// 1. Multiple requests in the same session,
/// 2. Unpriced models carrying tokens,
/// 3. Subagent-only requests on a day/model,
/// 4. Cross-session duplicate `message.id` (billed only on the earliest session, not the fork),
/// 5. Superseded sessions' leftover billed rows (single-hop and multi-hop continuation chains).
#[test]
fn drill_down_matches_contributing_rows_mapped_to_visible_sessions() {
    let root = data_root(&["basic", "subagents", "fork", "continued"]);

    // 1 & 2. Append multiple requests to BASIC, including an unpriced model with tokens and a
    // zero-token assistant message (which must NOT count as a request).
    let basic_path = root
        .path()
        .join("projects")
        .join(ids::BASIC_DIR)
        .join(format!("{}.jsonl", ids::BASIC));
    {
        let mut f = OpenOptions::new().append(true).open(&basic_path).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","uuid":"u-unp-1","parentUuid":null,"timestamp":"2026-08-15T12:00:00.000Z","sessionId":"{sid}","message":{{"id":"msg-unp-1","role":"assistant","model":"claude-unpriced-test","content":[{{"type":"text","text":"a"}}],"usage":{{"input_tokens":5}}}}}}"#,
            sid = ids::BASIC
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","uuid":"u-unp-2","parentUuid":null,"timestamp":"2026-08-15T12:05:00.000Z","sessionId":"{sid}","message":{{"id":"msg-unp-2","role":"assistant","model":"claude-unpriced-test","content":[{{"type":"text","text":"b"}}],"usage":{{"input_tokens":7}}}}}}"#,
            sid = ids::BASIC
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","uuid":"u-zero","parentUuid":null,"timestamp":"2026-08-16T12:00:00.000Z","sessionId":"{sid}","message":{{"id":"msg-zero","role":"assistant","model":"claude-zero-only","content":[{{"type":"text","text":"z"}}],"usage":{{"input_tokens":0,"output_tokens":0}}}}}}"#,
            sid = ids::BASIC
        )
        .unwrap();
    }

    // 5a. Single-hop superseded leftover billed row on CONTINUED_OLD -> CONTINUED_NEW.
    let cont_old_path = root
        .path()
        .join("projects")
        .join(ids::CONTINUED_DIR)
        .join(format!("{}.jsonl", ids::CONTINUED_OLD));
    {
        let mut f = OpenOptions::new()
            .append(true)
            .open(&cont_old_path)
            .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","uuid":"5f3a9d27-0c81-4b6e-a2d4-8e17c3b90f52","parentUuid":null,"sessionId":"{old}","timestamp":"2026-11-06T01:30:00.000Z","message":{{"id":"msg_leftover_1hop","role":"assistant","model":"claude-leftover-1hop","content":[{{"type":"thinking","thinking":""}}],"usage":{{"input_tokens":1,"output_tokens":3}}}}}}"#,
            old = ids::CONTINUED_OLD
        )
        .unwrap();
    }

    // 5b. Two-hop continuation chain A -> B -> C where A has a leftover billed row.
    const CHAIN_A: &str = "11111111-2222-3333-4444-555555555501";
    const CHAIN_B: &str = "11111111-2222-3333-4444-555555555502";
    const CHAIN_C: &str = "11111111-2222-3333-4444-555555555503";
    let chain_dir = root.path().join("projects").join(ids::CONTINUED_DIR);
    let chain_prompt = |sid: &str| {
        format!(
            r#"{{"type":"user","uuid":"p-chain-root","parentUuid":null,"sessionId":"{sid}","timestamp":"2026-11-07T08:00:00.000Z","message":{{"role":"user","content":"start chain"}}}}"#
        )
    };
    fs::write(
        chain_dir.join(format!("{CHAIN_A}.jsonl")),
        format!(
            "{prompt}\n\
{{\"type\":\"assistant\",\"uuid\":\"u-chain-a\",\"parentUuid\":\"p-chain-root\",\"sessionId\":\"{CHAIN_A}\",\"timestamp\":\"2026-11-07T09:00:00.000Z\",\"message\":{{\"id\":\"msg_chain_leftover\",\"role\":\"assistant\",\"model\":\"claude-leftover-2hop\",\"content\":[{{\"type\":\"text\",\"text\":\"a\"}}],\"usage\":{{\"input_tokens\":2,\"output_tokens\":4}}}}}}\n\
{{\"type\":\"continued-in\",\"sessionId\":\"{CHAIN_A}\",\"continuedInSessionId\":\"{CHAIN_B}\"}}\n",
            prompt = chain_prompt(CHAIN_A),
        ),
    )
    .unwrap();
    fs::write(
        chain_dir.join(format!("{CHAIN_B}.jsonl")),
        format!(
            "{prompt}\n\
{{\"type\":\"continued-in\",\"sessionId\":\"{CHAIN_B}\",\"continuedInSessionId\":\"{CHAIN_C}\"}}\n",
            prompt = chain_prompt(CHAIN_B),
        ),
    )
    .unwrap();
    fs::write(
        chain_dir.join(format!("{CHAIN_C}.jsonl")),
        format!(
            "{prompt}\n\
{{\"type\":\"assistant\",\"uuid\":\"u-chain-c\",\"parentUuid\":\"p-chain-root\",\"sessionId\":\"{CHAIN_C}\",\"timestamp\":\"2026-11-08T10:00:00.000Z\",\"message\":{{\"id\":\"msg_chain_c\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-6\",\"content\":[{{\"type\":\"text\",\"text\":\"c\"}}],\"usage\":{{\"input_tokens\":2,\"output_tokens\":4}}}}}}\n",
            prompt = chain_prompt(CHAIN_C),
        ),
    )
    .unwrap();

    let env = common::engine::Env::scanned(root);
    let db = env.db();

    // Verify our preconditions directly against the index:
    // - FORK_CHILD has unbilled rows duplicated from FORK_ORIGIN.
    let unbilled_fork_rows: i64 = db
        .query_row(
            "SELECT count(*) FROM messages WHERE session_id = ?1 AND role = 1 AND billed = 0",
            [ids::FORK_CHILD],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        unbilled_fork_rows > 0,
        "fork child has duplicate unbilled rows"
    );
    // - CONTINUED_OLD, CHAIN_A and CHAIN_B are superseded; CONTINUED_NEW and CHAIN_C are visible.
    let is_superseded = |sid: &str| -> bool {
        db.query_row(
            "SELECT superseded FROM sessions WHERE id = ?1",
            [sid],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert!(is_superseded(ids::CONTINUED_OLD));
    assert!(!is_superseded(ids::CONTINUED_NEW));
    assert!(is_superseded(CHAIN_A));
    assert!(is_superseded(CHAIN_B));
    assert!(!is_superseded(CHAIN_C));

    // Build the ground-truth visible owner map directly from `sessions`.
    let mut continued_in: BTreeMap<String, (bool, Option<String>)> = BTreeMap::new();
    {
        let mut st = db
            .prepare("SELECT id, superseded, continued_in_id FROM sessions WHERE is_empty = 0")
            .unwrap();
        let rows = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        for row in rows {
            let (id, sup, next): (String, bool, Option<String>) = row.unwrap();
            continued_in.insert(id, (sup, next));
        }
    }
    let visible_owner = |start: &str| -> Option<String> {
        let mut cur = start.to_owned();
        let mut seen = HashSet::from([cur.clone()]);
        loop {
            let &(sup, ref next) = continued_in.get(&cur)?;
            if !sup {
                return Some(cur);
            }
            let next = next.as_ref()?.clone();
            if !seen.insert(next.clone()) {
                return None;
            }
            cur = next;
        }
    };

    // Derive the exact expected session set per day and per model from contributing billed rows.
    let mut expected_by_day: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut expected_by_model: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    {
        let mut st = db
            .prepare(
                "SELECT m.session_id, m.ts_ms, coalesce(m.model, 'unknown')
                 FROM messages m
                 JOIN sessions s ON s.id = m.session_id AND s.is_empty = 0
                 WHERE m.role = 1 AND m.billed = 1
                   AND (m.in_tok > 0 OR m.out_tok > 0 OR m.cr_tok > 0 OR m.cc_tok > 0)",
            )
            .unwrap();
        let rows = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .unwrap();
        for row in rows {
            let (sid, ts, model) = row.unwrap();
            let owner =
                visible_owner(&sid).expect("every contributing row maps to a visible session");
            let day = local(ts).date().to_string();
            expected_by_day
                .entry(day)
                .or_default()
                .insert(owner.clone());
            expected_by_model.entry(model).or_default().insert(owner);
        }
    }

    // Check EVERY day and EVERY model against the derived sets.
    for (day, want) in &expected_by_day {
        assert_eq!(
            drill_ids(&env.engine, Drill::Day { day: day.clone() }, None),
            *want,
            "day {day}"
        );
    }
    for (model, want) in &expected_by_model {
        assert_eq!(
            drill_ids(
                &env.engine,
                Drill::Model {
                    models: vec![model.clone()]
                },
                None
            ),
            *want,
            "model {model}"
        );
    }

    // Spot-check each specific edge case explicitly:
    // - Unpriced model with tokens:
    assert_eq!(
        drill_ids(
            &env.engine,
            Drill::Model {
                models: vec!["claude-unpriced-test".into()]
            },
            None
        ),
        BTreeSet::from([ids::BASIC.to_owned()])
    );
    // - Zero-token assistant message is NOT a request:
    assert!(
        drill_ids(
            &env.engine,
            Drill::Model {
                models: vec!["claude-zero-only".into()]
            },
            None
        )
        .is_empty()
    );
    // - Single-hop and 2-hop superseded leftover rows map to CONTINUED_NEW and CHAIN_C:
    assert_eq!(
        drill_ids(
            &env.engine,
            Drill::Model {
                models: vec!["claude-leftover-1hop".into()]
            },
            None
        ),
        BTreeSet::from([ids::CONTINUED_NEW.to_owned()])
    );
    assert_eq!(
        drill_ids(
            &env.engine,
            Drill::Model {
                models: vec!["claude-leftover-2hop".into()]
            },
            None
        ),
        BTreeSet::from([CHAIN_C.to_owned()])
    );
    let leftover_2hop_day = local(
        DateTime::parse_from_rfc3339("2026-11-07T09:00:00.000Z")
            .unwrap()
            .timestamp_millis(),
    )
    .date()
    .to_string();
    assert_eq!(
        drill_ids(
            &env.engine,
            Drill::Day {
                day: leftover_2hop_day
            },
            None
        ),
        BTreeSet::from([CHAIN_C.to_owned()])
    );
    // - Cross-session duplicate: narrow the range to an inherited message's exact timestamp so
    //   FORK_CHILD only has an unbilled duplicate in range -> only FORK_ORIGIN is returned.
    let (dup_ts, dup_model): (i64, String) = db
        .query_row(
            "SELECT m.ts_ms, coalesce(m.model, 'unknown')
             FROM messages m
             WHERE m.session_id = ?1 AND m.role = 1 AND m.billed = 0
               AND (m.in_tok > 0 OR m.out_tok > 0 OR m.cr_tok > 0 OR m.cc_tok > 0)
             LIMIT 1",
            [ids::FORK_CHILD],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let dup_range = Some(TimeRange {
        from_ms: Some(dup_ts as f64),
        to_ms: Some(dup_ts as f64),
    });
    assert_eq!(
        drill_ids(
            &env.engine,
            Drill::Model {
                models: vec![dup_model]
            },
            dup_range.clone()
        ),
        BTreeSet::from([ids::FORK_ORIGIN.to_owned()])
    );
    assert_eq!(
        drill_ids(
            &env.engine,
            Drill::Day {
                day: local(dup_ts).date().to_string()
            },
            dup_range
        ),
        BTreeSet::from([ids::FORK_ORIGIN.to_owned()])
    );
}

#[test]
fn tokens_sort_orders_by_output_tokens() {
    let env = scanned(MIXED);
    let list = env
        .engine
        .list_sessions(&SessionQuery {
            sort: SessionSort::Tokens,
            descending: true,
            ..SessionQuery::default()
        })
        .unwrap();
    assert_eq!(list.len(), 5);
    let outs: Vec<f64> = list.iter().map(|s| s.tokens.output).collect();
    assert!(outs.windows(2).all(|w| w[0] >= w[1]), "{outs:?}");
    let o = &env.oracle;
    let top: i64 = o.sessions[&list[0].id]
        .msgs
        .iter()
        .map(|m| m.tokens[1])
        .sum();
    assert_eq!(list[0].tokens.output, top as f64, "main + subagents");
}

// ---------------------------------------------------------------- hourly

#[test]
fn hourly_over_seven_days_includes_empty_hours_and_conserves_totals() {
    let env = scanned(MIXED);
    let from = DateTime::parse_from_rfc3339("2026-06-06T10:30:00Z")
        .unwrap()
        .timestamp_millis() as f64;
    let st = env
        .engine
        .stats(&StatsRequest {
            time_range: Some(TimeRange {
                from_ms: Some(from),
                to_ms: Some(from + 7.0 * 86_400_000.0),
            }),
        })
        .unwrap();
    // 7 × 24 hours from a mid-hour start touch 169 hourly buckets.
    assert_eq!(st.hourly.len(), 169);
    assert!(st.hourly.windows(2).all(|w| w[0].end_ms == w[1].start_ms));
    assert!(
        st.hourly.iter().any(|h| h.requests == 0 && h.cost == 0.0),
        "empty hours are emitted"
    );
    assert!(st.overview.requests > 0);
    assert_eq!(
        st.hourly.iter().map(|h| h.requests).sum::<u32>(),
        st.overview.requests
    );
    assert!(close(
        st.hourly.iter().map(|h| h.cost).sum::<f64>(),
        st.overview.cost
    ));
    assert_eq!(
        st.daily.iter().map(|d| d.requests).sum::<u32>(),
        st.overview.requests
    );
}

#[test]
fn hourly_range_boundaries() {
    let env = scanned(MIXED);
    let from = DateTime::parse_from_rfc3339("2026-06-06T00:00:00Z")
        .unwrap()
        .timestamp_millis() as f64;
    let eight_days = 8.0 * 86_400_000.0;

    let query =
        |time_range: Option<TimeRange>| env.engine.stats(&StatsRequest { time_range }).unwrap();

    // Exact 8-day boundary: non-empty and conserves requests/cost.
    let at_limit = query(Some(TimeRange {
        from_ms: Some(from),
        to_ms: Some(from + eight_days),
    }));
    assert!(!at_limit.hourly.is_empty());
    assert_eq!(
        at_limit.hourly.iter().map(|h| h.requests).sum::<u32>(),
        at_limit.overview.requests
    );
    assert!(close(
        at_limit.hourly.iter().map(|h| h.cost).sum::<f64>(),
        at_limit.overview.cost
    ));

    // 1 ms past 8 days: empty.
    assert!(
        query(Some(TimeRange {
            from_ms: Some(from),
            to_ms: Some(from + eight_days + 1.0),
        }))
        .hourly
        .is_empty()
    );
    // Fractional boundaries evaluated on raw f64 ms (not truncated to i64):
    // 8 days + 0.5 ms exceeds the 8-day limit -> empty.
    assert!(
        query(Some(TimeRange {
            from_ms: Some(from),
            to_ms: Some(from + eight_days + 0.5),
        }))
        .hourly
        .is_empty()
    );
    // Reversed within the same integer millisecond (T+0.8 > T+0.2) -> empty.
    assert!(
        query(Some(TimeRange {
            from_ms: Some(from + 0.8),
            to_ms: Some(from + 0.2),
        }))
        .hourly
        .is_empty()
    );
    // Reversed bounds: empty.
    assert!(
        query(Some(TimeRange {
            from_ms: Some(from + 1000.0),
            to_ms: Some(from),
        }))
        .hourly
        .is_empty()
    );
    // Missing lower bound, upper bound, or whole range: empty.
    assert!(
        query(Some(TimeRange {
            from_ms: None,
            to_ms: Some(from + 86_400_000.0),
        }))
        .hourly
        .is_empty()
    );
    assert!(
        query(Some(TimeRange {
            from_ms: Some(from),
            to_ms: None,
        }))
        .hourly
        .is_empty()
    );
    assert!(query(None).hourly.is_empty());
}

/// Child half of `hourly_follows_dst_changes_with_conservation`: writes real requests across both
/// 2026 US DST transitions under the inherited `TZ` and verifies widths and request/cost
/// conservation.
#[test]
#[ignore = "spawned by hourly_follows_dst_changes_with_conservation with TZ set"]
fn dst_probe() {
    if std::env::var_os("CV_DST_PROBE").is_none() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("projects").join("-tmp-dst");
    fs::create_dir_all(&dir).unwrap();
    let sid = "22222222-3333-4444-5555-666666666601";
    let usage = r#"{"input_tokens":1000,"output_tokens":500}"#;
    let mk = |uuid: &str, id: &str, ts: &str| {
        format!(
            r#"{{"type":"assistant","uuid":"{uuid}","parentUuid":null,"sessionId":"{sid}","timestamp":"{ts}","message":{{"id":"{id}","role":"assistant","model":"claude-sonnet-4-6","content":[{{"type":"text","text":"ok"}}],"usage":{usage}}}}}"#
        )
    };
    fs::write(
        dir.join(format!("{sid}.jsonl")),
        [
            format!(
                r#"{{"type":"user","uuid":"u0","parentUuid":null,"sessionId":"{sid}","timestamp":"2026-03-08T06:00:00.000Z","message":{{"role":"user","content":"dst"}}}}"#
            ),
            // Spring forward (America/New_York): 01:30 EST and 03:30 EDT (02:00 does not exist).
            mk("s1", "m-s1", "2026-03-08T06:30:00.000Z"),
            mk("s2", "m-s2", "2026-03-08T07:30:00.000Z"),
            // Fall back (America/New_York): 01:20 EDT (first 01:00) and 01:40 EST (second 01:00).
            mk("f1", "m-f1", "2026-11-01T05:20:00.000Z"),
            mk("f2", "m-f2", "2026-11-01T06:40:00.000Z"),
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();
    let env = common::engine::Env::scanned(root);
    let parse_ms = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().timestamp_millis() as f64;
    let check = |from_utc: &str, to_utc: &str, want_reqs: u32| {
        let st = env
            .engine
            .stats(&StatsRequest {
                time_range: Some(TimeRange {
                    from_ms: Some(parse_ms(from_utc)),
                    to_ms: Some(parse_ms(to_utc)),
                }),
            })
            .unwrap();
        let h = &st.hourly;
        assert!(h.windows(2).all(|w| w[0].end_ms == w[1].start_ms));
        assert_eq!(st.overview.requests, want_reqs);
        assert!(st.overview.cost > 0.0);
        assert_eq!(
            h.iter().map(|b| b.requests).sum::<u32>(),
            st.overview.requests
        );
        assert!(close(
            h.iter().map(|b| b.cost).sum::<f64>(),
            st.overview.cost
        ));
        // Single-day DST probes also emit `hourly_models` conserving per-hour requests and cost.
        let mut hm_by_hour: BTreeMap<i64, (u32, f64)> = BTreeMap::new();
        for hm in &st.hourly_models {
            assert!(hm.requests > 0);
            let e = hm_by_hour.entry(hm.start_ms as i64).or_default();
            e.0 += hm.requests;
            e.1 += hm.cost;
        }
        for b in h {
            let (reqs, cost) = hm_by_hour.remove(&(b.start_ms as i64)).unwrap_or((0, 0.0));
            assert_eq!(reqs, b.requests);
            assert!(close(cost, b.cost));
        }
        assert!(
            hm_by_hour.is_empty(),
            "every hourly_models bucket matches an hourly bucket"
        );
        h.iter()
            .map(|b| format!("{}:{}", ((b.end_ms - b.start_ms) / 60_000.0), b.requests))
            .collect::<Vec<_>>()
            .join(",")
    };
    // 00:00 local to +5h on fall-back (04:00 EST) and spring-forward (06:00 EDT).
    let fall = check("2026-11-01T04:00:00Z", "2026-11-01T09:00:00Z", 2);
    // Inside the repeated hour where wall clock goes backwards (01:50 EDT -> 01:45 EST) while
    // unix ms goes forward: still produces the merged 120-minute bucket with the 01:40 EST request.
    let fold_sub = check("2026-11-01T05:50:00Z", "2026-11-01T06:45:00Z", 1);
    let spring = check("2026-03-08T05:00:00Z", "2026-03-08T10:00:00Z", 2);
    println!("DST_PROBE {fall}|{fold_sub}|{spring}");
}

#[test]
fn hourly_follows_dst_changes_with_conservation() {
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "dst_probe", "--ignored", "--nocapture"])
        .env("CV_DST_PROBE", "1")
        .env("TZ", "America/New_York")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let line = text
        .lines()
        .find_map(|l| l.strip_prefix("DST_PROBE "))
        .expect("probe output");
    let parts: Vec<&str> = line.split('|').collect();
    assert_eq!(parts.len(), 3, "{line}");
    // Clocks go back at 02:00: the 01:00 bucket holds both readings of that hour (120 min, 2 reqs).
    assert_eq!(parts[0], "60:0,120:2,60:0,60:0,60:0");
    // 01:50 EDT -> 01:45 EST: ms order holds even though wall clock went backwards (120 min, 1 req).
    assert_eq!(parts[1], "120:1");
    // Clocks go forward at 02:00: that hour gets no bucket (00:00, 01:00, 03:00, 04:00, 05:00, 06:00).
    assert_eq!(parts[2], "60:0,60:1,60:1,60:0,60:0,60:0");
}

// ---------------------------------------------------------------- single-day breakdown

#[test]
fn single_day_hourly_models_and_sessions_conserve_totals() {
    // Fixture with two visible sessions on the same day, multiple models (including a missing
    // model that must map to `stats::UNKNOWN`), and a tie-break on cost by `session_id`.
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("projects").join("-tmp-single-day");
    fs::create_dir_all(&dir).unwrap();

    let sid_high = "33333333-0000-4000-8000-000000000002";
    let sid_tie_a = "33333333-0000-4000-8000-000000000001";
    let sid_tie_b = "33333333-0000-4000-8000-000000000003";

    let prompt = |sid: &str, ts: &str, text: &str| {
        format!(
            r#"{{"type":"user","uuid":"p-{sid}","parentUuid":null,"sessionId":"{sid}","timestamp":"{ts}","message":{{"role":"user","content":"{text}"}}}}"#
        )
    };
    let asst = |sid: &str, uuid: &str, id: &str, ts: &str, model: Option<&str>, out_tok: i64| {
        let model_field = match model {
            Some(m) => format!(r#""model":"{m}","#),
            None => String::new(),
        };
        format!(
            r#"{{"type":"assistant","uuid":"{uuid}","parentUuid":"p-{sid}","sessionId":"{sid}","timestamp":"{ts}","message":{{"id":"{id}","role":"assistant",{model_field}"content":[{{"type":"text","text":"ok"}}],"usage":{{"input_tokens":1000,"output_tokens":{out_tok},"cache_read_input_tokens":200,"cache_creation_input_tokens":50}}}}}}"#
        )
    };

    fs::write(
        dir.join(format!("{sid_high}.jsonl")),
        [
            prompt(sid_high, "2026-06-10T09:00:00.000Z", "High cost session"),
            asst(
                sid_high,
                "h1",
                "m-h1",
                "2026-06-10T09:05:00.000Z",
                Some("claude-sonnet-4-6"),
                2000,
            ),
            asst(
                sid_high,
                "h2",
                "m-h2",
                "2026-06-10T09:10:00.000Z",
                Some("claude-haiku-4-5"),
                1000,
            ),
            asst(
                sid_high,
                "h3",
                "m-h3",
                "2026-06-10T10:15:00.000Z",
                None,
                500,
            ),
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();
    for (sid, title) in [(sid_tie_b, "Tie B"), (sid_tie_a, "Tie A")] {
        fs::write(
            dir.join(format!("{sid}.jsonl")),
            [
                prompt(sid, "2026-06-10T11:00:00.000Z", title),
                asst(
                    sid,
                    &format!("t-{sid}"),
                    &format!("m-t-{sid}"),
                    "2026-06-10T11:02:00.000Z",
                    Some("claude-haiku-4-5"),
                    400,
                ),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
    }

    let env = common::engine::Env::scanned(root);
    let from = DateTime::parse_from_rfc3339("2026-06-10T00:00:00Z")
        .unwrap()
        .timestamp_millis() as f64;
    let st = env
        .engine
        .stats(&StatsRequest {
            time_range: Some(TimeRange {
                from_ms: Some(from),
                to_ms: Some(from + 86_400_000.0 - 1.0),
            }),
        })
        .unwrap();

    assert_eq!(st.overview.requests, 5);
    assert!(!st.hourly_models.is_empty());
    assert!(
        st.hourly_models
            .iter()
            .any(|hm| hm.model == codeck_core::stats::UNKNOWN),
        "missing model maps to stats::UNKNOWN"
    );

    // Every hourly_models row has requests > 0 and sums per-hour to match `hourly`.
    let mut hm_by_hour: BTreeMap<i64, (u32, f64)> = BTreeMap::new();
    for hm in &st.hourly_models {
        assert!(hm.requests > 0);
        let e = hm_by_hour.entry(hm.start_ms as i64).or_default();
        e.0 += hm.requests;
        e.1 += hm.cost;
    }
    for h in &st.hourly {
        let (reqs, cost) = hm_by_hour.remove(&(h.start_ms as i64)).unwrap_or((0, 0.0));
        assert_eq!(reqs, h.requests);
        assert!(close(cost, h.cost));
    }
    assert!(hm_by_hour.is_empty());
    assert_eq!(
        st.hourly_models.iter().map(|m| m.output).sum::<f64>(),
        st.overview.output_tokens
    );

    // `sessions` conserves against `overview` (all rows belong to visible sessions), sorted by
    // cost desc then session_id asc, with titles from `sessions`.
    assert_eq!(st.sessions.len(), 3);
    assert_eq!(
        st.sessions.iter().map(|s| s.requests).sum::<u32>(),
        st.overview.requests
    );
    assert!(close(
        st.sessions.iter().map(|s| s.cost).sum::<f64>(),
        st.overview.cost
    ));
    let ids_in_order: Vec<&str> = st.sessions.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(ids_in_order, vec![sid_high, sid_tie_a, sid_tie_b]);
    assert_eq!(st.sessions[0].title, "High cost session");
    assert_eq!(st.sessions[1].title, "Tie A");
    assert_eq!(st.sessions[2].title, "Tie B");
    assert!(st.sessions[0].cost > st.sessions[1].cost);
    assert!(close(st.sessions[1].cost, st.sessions[2].cost));
}

#[test]
fn single_day_range_boundaries() {
    let env = scanned(MIXED);
    let from = DateTime::parse_from_rfc3339("2026-06-09T00:00:00Z")
        .unwrap()
        .timestamp_millis() as f64;
    let limit = 26.0 * 3_600_000.0;
    let query =
        |time_range: Option<TimeRange>| env.engine.stats(&StatsRequest { time_range }).unwrap();

    // Exact 26-hour boundary: both single-day fields emitted and conserve totals.
    let at_limit = query(Some(TimeRange {
        from_ms: Some(from),
        to_ms: Some(from + limit),
    }));
    assert!(at_limit.overview.requests > 0);
    assert!(!at_limit.hourly_models.is_empty());
    assert!(!at_limit.sessions.is_empty());
    assert_eq!(
        at_limit
            .hourly_models
            .iter()
            .map(|m| m.requests)
            .sum::<u32>(),
        at_limit.overview.requests
    );
    assert_eq!(
        at_limit.sessions.iter().map(|s| s.requests).sum::<u32>(),
        at_limit.overview.requests
    );
    assert!(close(
        at_limit.sessions.iter().map(|s| s.cost).sum::<f64>(),
        at_limit.overview.cost
    ));

    // 1 ms and 0.5 ms past 26 hours: single-day fields empty, while `hourly` (<= 8d) still populated.
    for extra in [1.0, 0.5] {
        let over = query(Some(TimeRange {
            from_ms: Some(from),
            to_ms: Some(from + limit + extra),
        }));
        assert!(!over.hourly.is_empty());
        assert!(over.hourly_models.is_empty());
        assert!(over.sessions.is_empty());
    }

    // Reversed bounds (whole ms and fractional ms) and missing bounds -> empty.
    for range in [
        Some(TimeRange {
            from_ms: Some(from + 1000.0),
            to_ms: Some(from),
        }),
        Some(TimeRange {
            from_ms: Some(from + 0.8),
            to_ms: Some(from + 0.2),
        }),
        Some(TimeRange {
            from_ms: None,
            to_ms: Some(from + 86_400_000.0),
        }),
        Some(TimeRange {
            from_ms: Some(from),
            to_ms: None,
        }),
        None,
    ] {
        let st = query(range);
        assert!(st.hourly_models.is_empty());
        assert!(st.sessions.is_empty());
    }
}

#[test]
fn single_day_session_spans_merge_within_ten_minutes() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("projects").join("-tmp-spans");
    fs::create_dir_all(&dir).unwrap();

    let sid_multi = "44444444-0000-4000-8000-000000000001";
    let sid_single = "44444444-0000-4000-8000-000000000002";
    let parse_ms = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().timestamp_millis() as f64;
    let mk = |sid: &str, uuid: &str, id: &str, ts: &str| {
        format!(
            r#"{{"type":"assistant","uuid":"{uuid}","parentUuid":null,"sessionId":"{sid}","timestamp":"{ts}","message":{{"id":"{id}","role":"assistant","model":"claude-sonnet-4-6","content":[{{"type":"text","text":"ok"}}],"usage":{{"input_tokens":100,"output_tokens":50}}}}}}"#
        )
    };

    // Multi-span session:
    // - 10:00 -> 10:09 (9 min gap: merged) -> 10:19 (10 min exact gap: merged) => [10:00, 10:19]
    // - 10:30 (11 min gap: split, single request) => [10:30, 10:30]
    let t0 = "2026-06-11T10:00:00.000Z";
    let t1 = "2026-06-11T10:09:00.000Z";
    let t2 = "2026-06-11T10:19:00.000Z";
    let t3 = "2026-06-11T10:30:00.000Z";
    let t_solo = "2026-06-11T12:00:00.000Z";

    fs::write(
        dir.join(format!("{sid_multi}.jsonl")),
        [
            format!(
                r#"{{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"{sid_multi}","timestamp":"{t0}","message":{{"role":"user","content":"multi"}}}}"#
            ),
            // Write out of timestamp order to verify sorting before span merging.
            mk(sid_multi, "a3", "m3", t3),
            mk(sid_multi, "a1", "m1", t1),
            mk(sid_multi, "a0", "m0", t0),
            mk(sid_multi, "a2", "m2", t2),
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();
    fs::write(
        dir.join(format!("{sid_single}.jsonl")),
        [
            format!(
                r#"{{"type":"user","uuid":"u2","parentUuid":null,"sessionId":"{sid_single}","timestamp":"{t_solo}","message":{{"role":"user","content":"solo"}}}}"#
            ),
            mk(sid_single, "s0", "ms0", t_solo),
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();

    let env = common::engine::Env::scanned(root);
    let from = parse_ms("2026-06-11T00:00:00.000Z");
    let st = env
        .engine
        .stats(&StatsRequest {
            time_range: Some(TimeRange {
                from_ms: Some(from),
                to_ms: Some(from + 86_400_000.0 - 1.0),
            }),
        })
        .unwrap();

    let multi = st
        .sessions
        .iter()
        .find(|s| s.session_id == sid_multi)
        .expect("multi session");
    assert_eq!(multi.requests, 4);
    assert_eq!(
        multi.spans,
        vec![
            codeck_core::model::TimeSpan {
                start_ms: parse_ms(t0),
                end_ms: parse_ms(t2),
            },
            codeck_core::model::TimeSpan {
                start_ms: parse_ms(t3),
                end_ms: parse_ms(t3),
            },
        ]
    );

    let solo = st
        .sessions
        .iter()
        .find(|s| s.session_id == sid_single)
        .expect("single session");
    assert_eq!(solo.requests, 1);
    assert_eq!(
        solo.spans,
        vec![codeck_core::model::TimeSpan {
            start_ms: parse_ms(t_solo),
            end_ms: parse_ms(t_solo),
        }]
    );
}

#[test]
fn single_day_sessions_attributes_superseded_rows_to_visible_successor() {
    let root = data_root(&["continued"]);
    let cont_dir = root.path().join("projects").join(ids::CONTINUED_DIR);
    let parse_ms = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().timestamp_millis() as f64;

    // 1-hop superseded leftover billed row on CONTINUED_OLD -> CONTINUED_NEW, plus a request on
    // CONTINUED_NEW 5 minutes later so their timestamps merge into one span under CONTINUED_NEW.
    // Pick a date with no pre-existing messages in the `continued` fixture.
    let ts_old = "2026-11-10T01:30:00.000Z";
    let ts_new = "2026-11-10T01:35:00.000Z";
    {
        let mut f = OpenOptions::new()
            .append(true)
            .open(cont_dir.join(format!("{}.jsonl", ids::CONTINUED_OLD)))
            .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","uuid":"u-cont-old-1","parentUuid":null,"sessionId":"{old}","timestamp":"{ts_old}","message":{{"id":"msg_sd_1hop_old","role":"assistant","model":"claude-sonnet-4-6","content":[{{"type":"text","text":"old"}}],"usage":{{"input_tokens":1000,"output_tokens":200}}}}}}"#,
            old = ids::CONTINUED_OLD
        )
        .unwrap();
    }
    {
        let mut f = OpenOptions::new()
            .append(true)
            .open(cont_dir.join(format!("{}.jsonl", ids::CONTINUED_NEW)))
            .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","uuid":"u-cont-new-1","parentUuid":null,"sessionId":"{new}","timestamp":"{ts_new}","message":{{"id":"msg_sd_1hop_new","role":"assistant","model":"claude-sonnet-4-6","content":[{{"type":"text","text":"new"}}],"usage":{{"input_tokens":1000,"output_tokens":300}}}}}}"#,
            new = ids::CONTINUED_NEW
        )
        .unwrap();
    }

    // 2-hop continuation chain A -> B -> C with a leftover billed row on A.
    const CHAIN_A: &str = "55555555-2222-3333-4444-555555555501";
    const CHAIN_B: &str = "55555555-2222-3333-4444-555555555502";
    const CHAIN_C: &str = "55555555-2222-3333-4444-555555555503";
    let ts_chain = "2026-11-10T02:00:00.000Z";
    let chain_prompt = |sid: &str, text: &str| {
        format!(
            r#"{{"type":"user","uuid":"p-sd-chain","parentUuid":null,"sessionId":"{sid}","timestamp":"2026-11-10T01:59:00.000Z","message":{{"role":"user","content":"{text}"}}}}"#
        )
    };
    fs::write(
        cont_dir.join(format!("{CHAIN_A}.jsonl")),
        format!(
            "{prompt}\n\
{{\"type\":\"assistant\",\"uuid\":\"u-sd-a\",\"parentUuid\":\"p-sd-chain\",\"sessionId\":\"{CHAIN_A}\",\"timestamp\":\"{ts_chain}\",\"message\":{{\"id\":\"msg_sd_2hop\",\"role\":\"assistant\",\"model\":\"claude-haiku-4-5\",\"content\":[{{\"type\":\"text\",\"text\":\"a\"}}],\"usage\":{{\"input_tokens\":500,\"output_tokens\":100}}}}}}\n\
{{\"type\":\"continued-in\",\"sessionId\":\"{CHAIN_A}\",\"continuedInSessionId\":\"{CHAIN_B}\"}}\n",
            prompt = chain_prompt(CHAIN_A, "Chain A"),
        ),
    )
    .unwrap();
    fs::write(
        cont_dir.join(format!("{CHAIN_B}.jsonl")),
        format!(
            "{prompt}\n\
{{\"type\":\"continued-in\",\"sessionId\":\"{CHAIN_B}\",\"continuedInSessionId\":\"{CHAIN_C}\"}}\n",
            prompt = chain_prompt(CHAIN_B, "Chain B"),
        ),
    )
    .unwrap();
    fs::write(
        cont_dir.join(format!("{CHAIN_C}.jsonl")),
        format!(
            "{prompt}\n\
{{\"type\":\"assistant\",\"uuid\":\"u-sd-c\",\"parentUuid\":\"p-sd-chain\",\"sessionId\":\"{CHAIN_C}\",\"timestamp\":\"2026-11-12T10:00:00.000Z\",\"message\":{{\"id\":\"msg_sd_c\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-6\",\"content\":[{{\"type\":\"text\",\"text\":\"c\"}}],\"usage\":{{\"input_tokens\":100,\"output_tokens\":100}}}}}}\n",
            prompt = chain_prompt(CHAIN_C, "Chain C visible"),
        ),
    )
    .unwrap();

    let env = common::engine::Env::scanned(root);
    let from = parse_ms("2026-11-10T00:00:00.000Z");
    let req = StatsRequest {
        time_range: Some(TimeRange {
            from_ms: Some(from),
            to_ms: Some(from + 86_400_000.0 - 1.0),
        }),
    };
    let st = env.engine.stats(&req).unwrap();

    // Superseded sessions never appear; their rows belong to CONTINUED_NEW and CHAIN_C.
    let ids: Vec<&str> = st.sessions.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(ids, vec![ids::CONTINUED_NEW, CHAIN_C]);
    assert_eq!(st.sessions[0].requests, 2);
    assert_eq!(
        st.sessions[0].spans,
        vec![codeck_core::model::TimeSpan {
            start_ms: parse_ms(ts_old),
            end_ms: parse_ms(ts_new),
        }]
    );
    assert_eq!(st.sessions[1].title, "Chain C visible");
    assert_eq!(st.sessions[1].requests, 1);
    assert_eq!(
        st.sessions[1].spans,
        vec![codeck_core::model::TimeSpan {
            start_ms: parse_ms(ts_chain),
            end_ms: parse_ms(ts_chain),
        }]
    );
    assert_eq!(
        st.sessions.iter().map(|s| s.requests).sum::<u32>(),
        st.overview.requests
    );
    assert!(close(
        st.sessions.iter().map(|s| s.cost).sum::<f64>(),
        st.overview.cost
    ));

    // A superseded row that cannot reach a visible session (e.g. broken continued_in_id) is
    // omitted from `sessions` while still counted in `overview`.
    let db = env.db();
    db.execute(
        "UPDATE sessions SET continued_in_id = 'missing-session' WHERE id = ?1",
        [CHAIN_B],
    )
    .unwrap();
    let st_broken = codeck_core::stats::compute(&db, &req).unwrap();
    let broken_ids: Vec<&str> = st_broken
        .sessions
        .iter()
        .map(|s| s.session_id.as_str())
        .collect();
    assert_eq!(broken_ids, vec![ids::CONTINUED_NEW]);
    assert_eq!(st_broken.overview.requests, 3);
    assert_eq!(st_broken.sessions[0].requests, 2);
}
