//! Stats: totals, local-day bucketing and drill-down on fixture copies through the Engine.
//! Expected values come from an independent oracle that reads the fixture JSONL directly
//! (README counting rules: human prompts + assistant messages by `message.id`, max usage per id).
mod common;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
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

fn drill_ids(env: &Env, drill: Drill, time_range: Option<TimeRange>) -> BTreeSet<String> {
    env.engine
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
    assert_eq!(st.overview.messages, 20);
    assert_eq!(st.overview.sessions, 1);
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

    // Messages and heatmaps: main files only; tokens: main + subagents.
    let main: Vec<_> = o.main_msgs().collect();
    assert_eq!(st.overview.messages as usize, main.len());
    assert_eq!(st.overview.sessions, 5);
    let out: i64 = o.all_msgs().map(|(_, m)| m.tokens[1]).sum();
    let main_out: i64 = main.iter().map(|(_, m)| m.tokens[1]).sum();
    assert!(out > main_out, "subagent tokens are included");
    assert_eq!(st.overview.output_tokens, out as f64);
    assert_eq!(st.daily.iter().map(|d| d.output).sum::<f64>(), out as f64);

    // Local-day heatmap from chrono::Local.
    let mut days: BTreeMap<String, u32> = BTreeMap::new();
    for (_, m) in &main {
        let t = local(m.ts);
        *days.entry(t.date().to_string()).or_default() += 1;
    }
    let got: BTreeMap<String, u32> = st
        .heat_daily
        .iter()
        .map(|d| (d.day.clone(), d.messages))
        .collect();
    assert_eq!(got, days);
    assert_eq!(st.overview.active_days as usize, days.len());

    // Daily tokens by (local day, model).
    let mut daily: BTreeMap<(String, String), [i64; 4]> = BTreeMap::new();
    for (_, m) in o.all_msgs().filter(|(_, m)| has_tokens(m)) {
        let key = (local(m.ts).date().to_string(), m.model.clone().unwrap());
        let e = daily.entry(key).or_default();
        for (a, b) in e.iter_mut().zip(m.tokens) {
            *a += b;
        }
    }
    let got: BTreeMap<(String, String), [i64; 4]> = st
        .daily
        .iter()
        .map(|d| {
            (
                (d.day.clone(), d.model.clone()),
                [d.input, d.output, d.cache_read, d.cache_creation].map(|v| v as i64),
            )
        })
        .collect();
    assert_eq!(got, daily);

    // Session ranking: most expensive first, at most TOP_SESSIONS, each with its own numbers.
    assert!(st.sessions.len() <= codeck_core::stats::TOP_SESSIONS);
    assert!(st.sessions.windows(2).all(|w| w[0].cost >= w[1].cost));
    let basic = st
        .sessions
        .iter()
        .find(|s| s.session_id == ids::BASIC)
        .expect("basic ranked");
    assert_eq!(basic.messages as usize, o.sessions[ids::BASIC].msgs.len());
    let basic_out: i64 = o.sessions[ids::BASIC]
        .msgs
        .iter()
        .map(|m| m.tokens[1])
        .sum();
    assert_eq!(basic.output_tokens, basic_out as f64);
    assert!(!basic.title.is_empty());
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
            time_range: Some(range.clone()),
        })
        .unwrap();
    let s = &env.oracle.sessions[ids::SUBAGENTS];
    let in_range = |m: &&Msg| m.ts >= from && m.ts <= to;
    assert_eq!(st.overview.sessions, 1);
    assert_eq!(
        st.overview.messages as usize,
        s.msgs.iter().filter(|m| m.main).filter(in_range).count()
    );
    let out: i64 = s.msgs.iter().filter(in_range).map(|m| m.tokens[1]).sum();
    assert_eq!(st.overview.output_tokens, out as f64);
    assert_eq!(st.sessions.len(), 1);
    assert_eq!(st.sessions[0].session_id, ids::SUBAGENTS);
}

/// Child half of `local_day_follows_tz`: prints the heat-map days under the inherited `TZ`.
#[test]
#[ignore = "spawned by local_day_follows_tz with TZ set"]
fn tz_probe() {
    if std::env::var_os("CV_TZ_PROBE").is_none() {
        return;
    }
    let env = scanned(MIXED);
    let st = all_time(&env);
    let days: Vec<String> = st
        .heat_daily
        .iter()
        .map(|d| format!("{}={}", d.day, d.messages))
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
        for (_, m) in env.oracle.main_msgs() {
            *expected.entry(day_in(m.ts, &off)).or_default() += 1;
        }
        assert_eq!(probe(tz), expected, "{tz}");
    }
    // UTC+14 and UTC-10 are 24 h apart: every message moves to a different day.
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

    assert!(st.heat_daily.len() >= 5);
    for d in &st.heat_daily {
        let want = set(&|s| {
            s.msgs
                .iter()
                .any(|m| m.main && local(m.ts).date().to_string() == d.day)
        });
        assert_eq!(
            drill_ids(&env, Drill::Day { day: d.day.clone() }, None),
            want,
            "day {}",
            d.day
        );
    }
    let models: BTreeSet<&String> = st.daily.iter().map(|d| &d.model).collect();
    assert!(models.len() >= 2);
    for model in models {
        let want = set(&|s| {
            s.msgs
                .iter()
                .any(|m| has_tokens(m) && m.model.as_ref() == Some(model))
        });
        let drill = Drill::Model {
            model: model.clone(),
        };
        assert_eq!(drill_ids(&env, drill, None), want, "model {model}");
    }
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
    assert_eq!(st.overview.messages, 0);
    assert!(st.overview.output_tokens > 0.0);
    for model in st.daily.iter().map(|d| d.model.clone()) {
        assert_eq!(
            drill_ids(&env, Drill::Model { model }, range.clone()),
            BTreeSet::from([ids::SUBAGENTS.to_owned()])
        );
    }
    // A day outside the range yields nothing.
    assert!(
        drill_ids(
            &env,
            Drill::Day {
                day: "2026-06-02".into()
            },
            range
        )
        .is_empty()
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

// ---------------------------------------------------------------- activity

#[test]
fn activity_bucket_is_the_finest_with_at_most_170_bars() {
    use codeck_core::model::ActivityBucket as B;
    use codeck_core::stats::choose_bucket;
    let at = |s: &str| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap();
    let to = at("2026-10-27 13:20");
    let back = |days: i64| to - chrono::Duration::days(days);
    assert_eq!(choose_bucket(back(7), to), B::Hour1);
    assert_eq!(choose_bucket(back(30), to), B::Hour6);
    assert_eq!(choose_bucket(back(90), to), B::Day);
    assert_eq!(choose_bucket(back(400), to), B::Week);
    assert_eq!(choose_bucket(back(3650), to), B::Month);
}

#[test]
fn activity_counts_every_main_message_and_bars_drill_to_their_sessions() {
    let env = scanned(MIXED);
    let o = &env.oracle;
    let st = all_time(&env);
    let a = &st.activity;
    assert!(!a.bars.is_empty());
    // Unbounded: from the earliest message's bucket, contiguous, empty bars included.
    assert!(a.bars.windows(2).all(|w| w[0].end_ms == w[1].start_ms));
    assert_eq!(
        a.bars.iter().map(|b| b.messages).sum::<u32>(),
        st.overview.messages
    );
    for b in a.bars.iter().filter(|b| b.messages > 0) {
        let want: BTreeSet<String> = o
            .sessions
            .iter()
            .filter(|(_, s)| {
                s.msgs
                    .iter()
                    .any(|m| m.main && (m.ts as f64) >= b.start_ms && (m.ts as f64) < b.end_ms)
            })
            .map(|(id, _)| id.clone())
            .collect();
        let drill = Drill::Span {
            from_ms: b.start_ms,
            to_ms: b.end_ms,
        };
        assert_eq!(drill_ids(&env, drill, None), want, "bar at {}", b.start_ms);
    }
}

#[test]
fn activity_over_seven_days_is_hourly() {
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
    assert_eq!(
        st.activity.bucket,
        codeck_core::model::ActivityBucket::Hour1
    );
    // 7 × 24 hours from a mid-hour start touch 169 hourly bars.
    assert_eq!(st.activity.bars.len(), 169);
    assert_eq!(
        st.activity.bars.iter().map(|b| b.messages).sum::<u32>(),
        st.overview.messages
    );
}

#[test]
fn span_drill_is_limited_to_the_time_range() {
    let env = scanned(MIXED);
    let o = &env.oracle;
    let st = all_time(&env);
    let bar = st
        .activity
        .bars
        .iter()
        .find(|b| b.messages > 1)
        .expect("a bar with messages");
    let main_in = |s: &Sess, from: f64, to: f64| {
        s.msgs
            .iter()
            .any(|m| m.main && (m.ts as f64) >= from && (m.ts as f64) < to)
    };
    // A range covering only the second half of the bar.
    let mid = (bar.start_ms + bar.end_ms) / 2.0;
    let range = TimeRange {
        from_ms: Some(mid),
        to_ms: None,
    };
    let want: BTreeSet<String> = o
        .sessions
        .iter()
        .filter(|(_, s)| main_in(s, mid, bar.end_ms))
        .map(|(id, _)| id.clone())
        .collect();
    let drill = Drill::Span {
        from_ms: bar.start_ms,
        to_ms: bar.end_ms,
    };
    assert_eq!(drill_ids(&env, drill, Some(range)), want);
}

/// Child half of `activity_bars_follow_dst_changes`: prints hourly bar widths (minutes) around
/// both 2026 US DST changes under the inherited `TZ`.
#[test]
#[ignore = "spawned by activity_bars_follow_dst_changes with TZ set"]
fn dst_probe() {
    if std::env::var_os("CV_DST_PROBE").is_none() {
        return;
    }
    let env = scanned(MIXED);
    let widths = |from_utc: &str| {
        let from = DateTime::parse_from_rfc3339(from_utc)
            .unwrap()
            .timestamp_millis() as f64;
        let st = env
            .engine
            .stats(&StatsRequest {
                time_range: Some(TimeRange {
                    from_ms: Some(from),
                    to_ms: Some(from + 5.0 * 3_600_000.0),
                }),
            })
            .unwrap();
        let bars = &st.activity.bars;
        assert!(bars.windows(2).all(|w| w[0].end_ms == w[1].start_ms));
        bars.iter()
            .map(|b| ((b.end_ms - b.start_ms) / 60_000.0).to_string())
            .collect::<Vec<_>>()
            .join(",")
    };
    // 00:00 local on each change day in New York (EDT / EST).
    println!(
        "DST_PROBE {}|{}",
        widths("2026-11-01T04:00:00Z"),
        widths("2026-03-08T05:00:00Z")
    );
}

#[test]
fn activity_bars_follow_dst_changes() {
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
    let (fall, spring) = line.split_once('|').unwrap();
    // Clocks go back at 02:00: the 01:00 bar holds both readings of that hour (00:00 to 04:00 local).
    assert_eq!(fall, "60,120,60,60,60");
    // Clocks go forward at 02:00: that hour does not exist and gets no bar (00:00 to 06:00 local).
    assert_eq!(spring, "60,60,60,60,60,60");
}
