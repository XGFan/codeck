//! Against a scripted local model (`mock_` tests: no key, no credits, deterministic timing).

use std::io::{BufRead, BufReader, Write};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::harness::*;

/// A scripted Anthropic Messages endpoint on loopback. A prompt containing `MOCK-SLOW` is answered
/// over ~8 s, `MOCK-FAIL` with HTTP 400, `MOCK-TOOL` with a Bash call (`touch mock-tool.txt`),
/// `MOCK-ASK` with an AskUserQuestion call (`MOCK-ASKS`: two questions, the second multi-select), `MOCK-BG` with a background `sleep 2`, then `sleep 1`,
/// then ~8 s of text (the background task ends meanwhile; its notification is answered as slowly);
/// anything else (tool results too) with text `ok`.
fn mock_model() -> String {
    use std::net::TcpListener;
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for conn in l.incoming() {
            let Ok(mut s) = conn else { continue };
            std::thread::spawn(move || {
                let mut r = BufReader::new(s.try_clone().unwrap());
                let (mut line, mut len) = (String::new(), 0usize);
                r.read_line(&mut line).unwrap_or(0);
                let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                        break;
                    }
                    if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; len];
                use std::io::Read;
                let _ = r.read_exact(&mut body);
                let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let reply = |s: &mut std::net::TcpStream, code: &str, v: Value| {
                    let b = v.to_string();
                    let _ = write!(
                        s,
                        "HTTP/1.1 {code}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{b}",
                        b.len()
                    );
                };
                if path.contains("count_tokens") {
                    return reply(&mut s, "200 OK", json!({"input_tokens": 10}));
                }
                if !path.starts_with("/v1/messages") {
                    return reply(
                        &mut s,
                        "404 Not Found",
                        json!({"type": "error", "error": {"type": "not_found_error", "message": "no"}}),
                    );
                }
                // The newest user text (after any tool results).
                let last = req["messages"]
                    .as_array()
                    .and_then(|m| m.iter().rev().find(|m| m["role"] == "user"))
                    .cloned();
                let text = match last.as_ref().map(|m| &m["content"]) {
                    Some(Value::String(t)) => t.clone(),
                    Some(Value::Array(a)) => a
                        .iter()
                        .filter_map(|b| b["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => String::new(),
                };
                if text.contains("MOCK-FAIL") {
                    return reply(
                        &mut s,
                        "400 Bad Request",
                        json!({"type": "error", "error": {"type": "invalid_request_error", "message": "mock failure"}}),
                    );
                }
                let has_tool = |n: &str| {
                    req["tools"]
                        .as_array()
                        .is_some_and(|t| t.iter().any(|t| t["name"] == n))
                };
                static TOOL_SEQ: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                // Claude Code merges and reorders consecutive messages, so which prompt a request
                // answers is not read off the last message: a tool is called while there are more
                // prompts asking for it than calls to it so far (a tool result gets text).
                let msgs = req["messages"].as_array().cloned().unwrap_or_default();
                let blocks = |role: &str| -> Vec<Value> {
                    msgs.iter()
                        .filter(|m| m["role"] == role)
                        .flat_map(|m| match &m["content"] {
                            Value::Array(a) => a.clone(),
                            Value::String(t) => vec![json!({"type": "text", "text": t})],
                            _ => vec![],
                        })
                        .collect()
                };
                let (user, assistant) = (blocks("user"), blocks("assistant"));
                let asked = |w: &str| {
                    user.iter()
                        .filter(|b| b["text"].as_str().is_some_and(|t| t.contains(w)))
                        .count()
                };
                let called = |n: &str| {
                    assistant
                        .iter()
                        .filter(|b| b["type"] == "tool_use" && b["name"] == n)
                        .count()
                };
                let tool = if asked("MOCK-TOOL") > called("Bash") && has_tool("Bash") {
                    Some((
                        "Bash",
                        json!({"command": "touch mock-tool.txt", "description": "Create mock-tool.txt"}),
                    ))
                } else if asked("MOCK-LONGTASK") > called("Bash") && has_tool("Bash") {
                    Some((
                        "Bash",
                        json!({"command": "sleep 300", "description": "Wait long in background", "run_in_background": true}),
                    ))
                } else if asked("MOCK-BG") > 0 && called("Bash") < 2 && has_tool("Bash") {
                    Some((
                        "Bash",
                        match called("Bash") {
                            0 => {
                                json!({"command": "sleep 2", "description": "Wait in background", "run_in_background": true})
                            }
                            _ => json!({"command": "sleep 1", "description": "Wait"}),
                        },
                    ))
                } else if asked("MOCK-ASK") > called("AskUserQuestion")
                    && has_tool("AskUserQuestion")
                {
                    let color = json!({"question": "Which color?", "header": "Color",
                        "options": [{"label": "Red", "description": "warm"}, {"label": "Blue", "description": "cool"}],
                        "multiSelect": false});
                    let sizes = json!({"question": "Which sizes?", "header": "Sizes",
                        "options": [{"label": "S", "description": "small"}, {"label": "M", "description": "medium"},
                            {"label": "L", "description": "large"}],
                        "multiSelect": true});
                    let questions = if text.contains("MOCK-ASKS") {
                        json!([color, sizes])
                    } else {
                        json!([color])
                    };
                    Some(("AskUserQuestion", json!({"questions": questions})))
                } else {
                    None
                };
                // MOCK-BG's last answer (to the second tool's result, not to the task notification).
                let bg_last = asked("MOCK-BG") > 0 && called("Bash") == 2 && text.is_empty();
                let (words, gap): (Vec<&str>, u64) = if text.contains("MOCK-SLOW")
                    || bg_last
                    || text.contains("<task-notification>")
                {
                    (vec!["slow "; 16], 500)
                } else if text.contains("MOCK-STEERED") {
                    (vec!["STEER-TURN"], 0)
                } else {
                    (vec!["ok"], 0)
                };
                let model = req["model"].as_str().unwrap_or("mock").to_string();
                let usage = json!({"input_tokens": 10, "output_tokens": 3});
                let msg = json!({"id": "msg_mock", "type": "message", "role": "assistant", "model": model,
                    "content": [], "stop_reason": null, "stop_sequence": null, "usage": usage});
                let tool = tool.map(|(name, input)| {
                    let n = TOOL_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    (format!("toolu_mock{n:04}"), name, input)
                });
                let stop = if tool.is_some() {
                    "tool_use"
                } else {
                    "end_turn"
                };
                if req["stream"] != true {
                    let mut m = msg.clone();
                    m["content"] = match &tool {
                        Some((id, name, input)) => {
                            json!([{"type": "tool_use", "id": id, "name": name, "input": input}])
                        }
                        None => json!([{"type": "text", "text": words.concat()}]),
                    };
                    m["stop_reason"] = json!(stop);
                    return reply(&mut s, "200 OK", m);
                }
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n"
                );
                let mut ev = |name: &str, v: Value| {
                    let _ = write!(s, "event: {name}\ndata: {v}\n\n");
                    let _ = s.flush();
                };
                ev(
                    "message_start",
                    json!({"type": "message_start", "message": msg}),
                );
                if let Some((id, name, input)) = &tool {
                    ev(
                        "content_block_start",
                        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": id, "name": name, "input": {}}}),
                    );
                    ev(
                        "content_block_delta",
                        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": input.to_string()}}),
                    );
                } else {
                    ev(
                        "content_block_start",
                        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
                    );
                    for w in words {
                        std::thread::sleep(Duration::from_millis(gap));
                        ev(
                            "content_block_delta",
                            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": w}}),
                        );
                    }
                }
                ev(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": 0}),
                );
                ev(
                    "message_delta",
                    json!({"type": "message_delta", "delta": {"stop_reason": stop, "stop_sequence": null}, "usage": {"output_tokens": 3}}),
                );
                ev("message_stop", json!({"type": "message_stop"}));
            });
        }
    });
    format!("http://{addr}")
}

/// Turns the plugin delimits, on a scripted model (no credits): a steer that arrives after the
/// last tool runs as a turn of its own (no UserPromptSubmit of its own), and an API error ends a
/// turn through StopFailure.
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_turns() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let mut rpc = Rpc::start(&env);
    start_session(&mut rpc, &env);

    rpc.forget();
    let d = rpc.call("s1", json!({"type": "prompt", "text": "MOCK-SLOW please"}));
    let first = d["prompt_id"].clone();
    // The reply is streaming (the spinner counts tokens): a steer now waits in the queue.
    rpc.until("streaming", Duration::from_secs(10), |v| {
        v["type"] == "progress" && v["tokens"].as_u64().is_some_and(|n| n > 0)
    });
    let st = rpc.call("s2", json!({"type": "steer", "text": "MOCK-STEERED now"}));
    assert_eq!(st["delivered_as"], "queued", "{st}");
    let t1 = rpc.event("turn_end", Duration::from_secs(60));
    assert_eq!(t1["prompt_id"], first, "{t1}");
    // Each turn's blocks come before its turn_end (the steer's Stop arrives right after this one's).
    let text_before = |rpc: &Rpc, w: &str| {
        rpc.seen.iter().any(|v| {
            v["type"] == "block"
                && v["kind"] == "text"
                && v["text"].as_str().is_some_and(|t| t.contains(w))
        })
    };
    assert!(
        text_before(&rpc, "slow"),
        "第一轮的文字在它的 turn_end 之后才到"
    );
    assert!(
        !text_before(&rpc, "STEER-TURN"),
        "插话那一轮的文字跑到了第一轮的 turn_end 之前"
    );
    assert!(
        t1["last_text"].as_str().unwrap_or("").contains("slow"),
        "{t1}"
    );
    let t2 = rpc.event("turn_end", Duration::from_secs(60));
    assert!(
        t2["prompt_id"].is_string() && t2["prompt_id"] != first,
        "{t2}"
    );
    assert!(
        text_before(&rpc, "STEER-TURN"),
        "插话那一轮的文字在它的 turn_end 之后才到"
    );
    assert_eq!(t2["last_text"], "STEER-TURN", "{t2}");
    assert_eq!(t2["aborted"], false, "{t2}");

    rpc.forget();
    rpc.call("f1", json!({"type": "prompt", "text": "MOCK-FAIL"}));
    let t3 = rpc.event("turn_end", Duration::from_secs(60));
    assert!(t3["error"].is_object(), "{t3}");
    assert!(
        t3["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("400"),
        "{t3}"
    );
    assert_eq!(t3["aborted"], false);
}

/// Claude Code's own queue, as in the TUI: a steer that cannot be absorbed waits in it (a `queue`
/// event lists it); `send_now` (`ctrl+enter`) interrupts the turn and runs it at once.
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_send_now() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let mut rpc = Rpc::start(&env);
    start_session(&mut rpc, &env);

    rpc.forget();
    let d = rpc.call("s1", json!({"type": "prompt", "text": "MOCK-SLOW please"}));
    let first = d["prompt_id"].clone();
    // The reply is streaming (the spinner counts tokens): a steer now waits in the queue.
    rpc.until("streaming", Duration::from_secs(10), |v| {
        v["type"] == "progress" && v["tokens"].as_u64().is_some_and(|n| n > 0)
    });
    rpc.call("s2", json!({"type": "steer", "text": "MOCK-STEERED now"}));
    let q = rpc.until("queue", Duration::from_secs(10), |v| {
        v["type"] == "queue" && v["items"].as_array().is_some_and(|a| !a.is_empty())
    });
    assert_eq!(q["items"], json!(["MOCK-STEERED now"]), "{q}");
    let st = rpc.call("g1", json!({"type": "get_state"}));
    assert_eq!(st["queue"], json!(["MOCK-STEERED now"]), "{st}");

    rpc.call("n1", json!({"type": "send_now"}));
    rpc.until("queue", Duration::from_secs(5), |v| {
        v["type"] == "queue" && v["items"] == json!([])
    });
    let t1 = rpc.event("turn_end", Duration::from_secs(30));
    assert_eq!(t1["prompt_id"], first, "{t1}");
    assert_eq!(t1["aborted"], true, "{t1}");
    let t2 = rpc.event("turn_end", Duration::from_secs(30));
    assert_eq!(t2["last_text"], "STEER-TURN", "{t2}");
    assert_eq!(t2["aborted"], false, "{t2}");
}

/// A steer with an image while busy: pasted by path like a prompt's, it waits in Claude Code's
/// queue with its `[Image #1]` placeholder, on the scripted model (no credits).
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_steer_image() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let mut rpc = Rpc::start(&env);
    start_session(&mut rpc, &env);
    let img = env.work.join("dot.png");
    std::fs::write(&img, red_dot_png()).unwrap();

    rpc.forget();
    rpc.call("s1", json!({"type": "prompt", "text": "MOCK-SLOW please"}));
    // The reply is streaming (the spinner counts tokens): a steer now waits in the queue.
    rpc.until("streaming", Duration::from_secs(10), |v| {
        v["type"] == "progress" && v["tokens"].as_u64().is_some_and(|n| n > 0)
    });
    let d = rpc.call(
        "s2",
        json!({"type": "steer", "text": "MOCK-STEERED with image", "images": [img]}),
    );
    assert_eq!(d["delivered_as"], "queued", "{d}");
    let q = rpc.until("queue", Duration::from_secs(10), |v| {
        v["type"] == "queue" && v["items"].as_array().is_some_and(|a| !a.is_empty())
    });
    let item = q["items"][0].as_str().unwrap_or_default();
    assert!(
        item.contains("[Image #1]") && item.ends_with("MOCK-STEERED with image"),
        "{q}"
    );
    rpc.event("turn_end", Duration::from_secs(30));
    rpc.until("queue", Duration::from_secs(30), |v| {
        v["type"] == "queue" && v["items"] == json!([])
    });
}

/// Model and effort from here: `set_effort` / `set_model` mid-conversation ask first (built-in
/// choice layouts), and `session_info` says what was set at once — from the command's output and
/// the TUI's effort mark — on the scripted model (no credits).
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_model_and_effort() {
    let env = setup_with("mock-key", &mock_model(), "claude-opus-4-7");
    let mut rpc = Rpc::start(&env);
    start_session(&mut rpc, &env);
    rpc.call("p1", json!({"type": "prompt", "text": "hello"}));
    rpc.event("turn_end", Duration::from_secs(30));
    let st = rpc.call("g0", json!({"type": "get_state"}));
    assert_eq!(st["effort"], "xhigh", "{st}");
    // Mid-conversation the TUI asks first (the history is re-read); answered as a choice.
    let confirm = |rpc: &mut Rpc, id: &str, title: &str, yes: &str| {
        let c = rpc.until(title, Duration::from_secs(10), |v| v["type"] == "choice");
        assert_eq!(c["title"], title, "{c}");
        assert_eq!(c["layout_source"], "builtin", "{c}");
        assert_eq!(c["options"][0]["label"], yes, "{c}");
        rpc.call(
            id,
            json!({"type": "answer_choice", "request_id": c["request_id"], "option": yes}),
        );
    };

    // Model: then its name at once; Sonnet 4.5 has no effort levels.
    rpc.forget();
    rpc.call(
        "m1",
        json!({"type": "set_model", "model": "claude-sonnet-4-5"}),
    );
    confirm(&mut rpc, "c1", "Switch model?", "Yes, switch to Sonnet 4.5");
    let i = rpc.until("sonnet", Duration::from_secs(10), |v| {
        v["type"] == "session_info" && v["model"] == "Sonnet 4.5"
    });
    assert!(i["effort"].is_null(), "{i}");

    // Nothing said to Sonnet yet: set at once, the effort from the TUI's mark.
    rpc.forget();
    rpc.call("m2", json!({"type": "set_model", "model": "fable"}));
    let i = rpc.until("fable", Duration::from_secs(10), |v| {
        v["type"] == "session_info" && v["model"] == "Fable 5.1" && v["effort"].is_string()
    });
    assert_ne!(i["effort"], "low", "{i}");

    // Effort, after a reply on Fable. Claude Code asks only the first time in a Session (the
    // effort question's layout was seen live in an earlier run and is checked on its screen).
    rpc.call("p2", json!({"type": "prompt", "text": "again"}));
    rpc.event("turn_end", Duration::from_secs(30));
    rpc.forget();
    rpc.call("e1", json!({"type": "set_effort", "level": "low"}));
    let v = rpc.until("effort low", Duration::from_secs(10), |v| {
        v["type"] == "choice" || (v["type"] == "session_info" && v["effort"] == "low")
    });
    if v["type"] == "choice" {
        assert_eq!(v["title"], "Change effort level?", "{v}");
        assert_eq!(v["layout_source"], "builtin", "{v}");
        rpc.call(
            "c2",
            json!({"type": "answer_choice", "request_id": v["request_id"], "option": v["options"][0]["label"]}),
        );
        rpc.until("effort low", Duration::from_secs(10), |v| {
            v["type"] == "session_info" && v["effort"] == "low"
        });
    }
    let st = rpc.call("g1", json!({"type": "get_state"}));
    assert_eq!(st["state"], "idle", "{st}");
    assert_eq!(st["model"], "Fable 5.1", "{st}");
}

/// Starting in a folder not trusted yet, the status line, progress, paging, the log and exit, on
/// the scripted model (no credits).
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_session() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    // Outside the trusted work folder (a trusted folder's subfolders are trusted too).
    let fresh_dir = tempfile::tempdir().unwrap();
    let fresh = fresh_dir.path().canonicalize().unwrap();
    let log = env.state_dir.join("ccrpc.log");
    let mut rpc = Rpc::start_in(&env, &fresh, &["--log", log.to_str().unwrap()]);
    let ready = rpc.event("ready", Duration::from_secs(20));
    rpc.holder_pid = holder_pid(&env.toh_dir, ready["holder_id"].as_str().unwrap());

    // The folder trust dialog is a choice.
    let c = rpc.until("trust", Duration::from_secs(30), |v| v["type"] == "choice");
    assert_eq!(c["layout"], "trust_folder", "{c}");
    let d = rpc.call("t1", json!({"type": "answer_choice", "request_id": c["request_id"], "option": "Yes, I trust this folder"}));
    assert!(d["state"].is_string(), "{d}");
    wait_state(&mut rpc, "idle", 30);
    // `ready` came before the plugin checked in: the sessionId is announced once known.
    assert!(ready["session_id"].is_null(), "{ready}");
    let ch = rpc.event("session_changed", Duration::from_secs(15));
    assert!(ch["previous"].is_null(), "{ch}");
    assert!(
        ch["session_id"].as_str().is_some_and(|s| s.len() == 36),
        "{ch}"
    );

    // A few turns: the status line, then progress while one runs slowly.
    for i in 0..3 {
        rpc.call(
            &format!("p{i}"),
            json!({"type": "prompt", "text": format!("hello {i}")}),
        );
        rpc.event("turn_end", Duration::from_secs(30));
    }
    let st = rpc.call("g1", json!({"type": "get_state"}));
    assert_eq!(st["mode"], "manual", "{st}");
    assert_eq!(st["model"], "claude-sonnet-4-5", "{st}");
    assert!(st["context_tokens"].as_u64().unwrap_or(0) > 0, "{st}");
    rpc.forget();
    rpc.call("s1", json!({"type": "prompt", "text": "MOCK-SLOW please"}));
    let p = rpc.event("progress", Duration::from_secs(15));
    assert!(
        p["verb"].is_string() && p["text"].as_str().unwrap().contains('…'),
        "{p}"
    );
    // A follow-up queued while busy, with an image: sent as a prompt once idle.
    let img = env.work.join("dot.png");
    std::fs::write(&img, red_dot_png()).unwrap();
    let q = rpc.call(
        "f1",
        json!({"type": "follow_up", "text": "after-image", "images": [img]}),
    );
    assert_eq!(q["queued"], 1, "{q}");
    rpc.event("turn_end", Duration::from_secs(30));
    let u = rpc.until("follow-up", Duration::from_secs(30), |v| {
        v["type"] == "follow_up_failed"
            || (v["type"] == "block"
                && v["kind"] == "user"
                && v["text"]
                    .as_str()
                    .is_some_and(|t| t.contains("after-image")))
    });
    assert_eq!(u["type"], "block", "follow_up 没发出去：{u}");
    rpc.event("turn_end", Duration::from_secs(30));
    // Its block names the image, which comes back as sent; Claude Code's note of where it came
    // from is not a block.
    assert_eq!(u["images"], json!(["image/png"]), "{u}");
    let im = rpc.call(
        "i1",
        json!({"type": "get_image", "uuid": u["uuid"], "index": 0}),
    );
    assert_eq!(im["media_type"], "image/png");
    use base64::Engine as _;
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(im["data"].as_str().unwrap())
            .unwrap(),
        red_dot_png()
    );

    // Paging back from the newest.
    let all = rpc.call("m0", json!({"type": "get_messages"}));
    let n = all["blocks"].as_array().unwrap().len();
    assert!(n >= 8, "{all}");
    let notes = all["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| {
            b["text"]
                .as_str()
                .is_some_and(|t| t.starts_with("[Image: source:"))
        })
        .count();
    assert_eq!(notes, 0, "{all}");
    let last = rpc.call("m1", json!({"type": "get_messages", "limit": 3}));
    assert_eq!(last["blocks"].as_array().unwrap().len(), 3);
    assert_eq!(last["more"], true);
    assert_eq!(
        last["blocks"],
        json!(all["blocks"].as_array().unwrap()[n - 3..])
    );
    let older = rpc.call(
        "m2",
        json!({"type": "get_messages", "before": last["first"], "limit": 100}),
    );
    assert_eq!(older["blocks"].as_array().unwrap().len(), n - 3);
    assert_eq!(older["more"], false);

    // Exit, with the Pane.
    let holder_pid = rpc.holder_pid.unwrap();
    rpc.call("x1", json!({"type": "exit", "close_pane": true}));
    let deadline = Instant::now() + Duration::from_secs(10);
    while transcript::live::pid_exists(holder_pid) {
        assert!(Instant::now() < deadline, "Pane 没有随 exit 结束");
        std::thread::sleep(Duration::from_millis(200));
    }
    rpc.holder_pid = None;
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(
        text.contains("hook UserPromptSubmit") && text.contains("keys "),
        "日志里没有 hook 与按键"
    );
    assert!(
        text.contains(" holder=") && text.contains(" claude="),
        "日志开头没有版本行"
    );
    assert!(
        text.contains("recognize choice") && text.contains("  | "),
        "日志里没有识别决策与整屏"
    );
}

/// Tool calls on the scripted model (no credits): a Bash approval carries the dialog's options
/// (read off the screen) and where its layout comes from; AskUserQuestion is answered through the
/// plugin; a denial with a message keeps the turn going.
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_tools() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let mut rpc = Rpc::start(&env);
    start_session(&mut rpc, &env);

    rpc.forget();
    rpc.call("t1", json!({"type": "prompt", "text": "MOCK-TOOL please"}));
    let pr = rpc.event("permission_request", Duration::from_secs(30));
    assert_eq!(pr["tool_name"], "Bash", "{pr}");
    assert_eq!(pr["layout_source"], "builtin", "{pr}");
    let opts = pr["options"]
        .as_array()
        .unwrap_or_else(|| panic!("permission_request 没带 options：{pr}"));
    let meanings: Vec<&str> = opts.iter().filter_map(|o| o["meaning"].as_str()).collect();
    assert_eq!(meanings.first(), Some(&"allow"), "{pr}");
    assert_eq!(meanings.last(), Some(&"deny"), "{pr}");
    assert!(meanings.contains(&"allow_always"), "{pr}");
    assert!(
        opts.iter()
            .all(|o| o["label"].as_str().is_some_and(|l| !l.is_empty())),
        "{pr}"
    );
    rpc.call(
        "t2",
        json!({"type": "answer_permission", "request_id": pr["request_id"], "decision": "allow"}),
    );
    let t = rpc.event("turn_end", Duration::from_secs(30));
    assert_eq!(t["aborted"], false, "{t}");
    assert!(
        env.work.join("mock-tool.txt").exists(),
        "允许之后 touch 没有执行"
    );

    rpc.forget();
    rpc.call("t3", json!({"type": "prompt", "text": "MOCK-TOOL again"}));
    let pr = rpc.event("permission_request", Duration::from_secs(30));
    rpc.call("t4", json!({"type": "answer_permission", "request_id": pr["request_id"], "decision": "deny", "message": "not now"}));
    let t = rpc.event("turn_end", Duration::from_secs(30));
    assert_eq!(t["aborted"], false, "带 message 的拒绝不应中断本轮：{t}");

    rpc.forget();
    rpc.call("q1", json!({"type": "prompt", "text": "MOCK-ASK which"}));
    let q = rpc.event("question", Duration::from_secs(30));
    assert_eq!(q["questions"][0]["question"], "Which color?", "{q}");
    rpc.call("q2", json!({"type": "answer_question", "request_id": q["request_id"], "answers": [{"options": ["Blue"]}]}));
    rpc.event("turn_end", Duration::from_secs(30));
    let m = rpc.call("m1", json!({"type": "get_messages"}));
    let answered = m["blocks"].as_array().unwrap().iter().any(|b| {
        b["kind"] == "tool_result" && b["content"].as_str().is_some_and(|c| c.contains("Blue"))
    });
    assert!(answered, "AskUserQuestion 的结果里没有答案：{m}");
}

/// A hidden Pane (`new --hidden`, ToH ADR-0050), co-displayed as CoDeck starts it: nobody shows it, so its PTY
/// keeps the size it starts with, ccrpc's 160×48, and an approval dialog is recognized there; `ls --json` reports
/// `hidden`, which `hide` / `show` toggle (on the scripted model, no credits).
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_hidden() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let mut rpc = Rpc::start_in(&env, &env.work, &["--hidden", "--co-display"]);
    let ready = start_session(&mut rpc, &env);
    assert_eq!(ready["size"], json!([160, 48]), "{ready}");
    let holder = ready["holder_id"].as_str().unwrap().to_string();
    let hidden = || {
        let out = ccrpc_cmd(&env).args(["ls", "--json"]).output().unwrap();
        assert!(out.status.success(), "{out:?}");
        let rows: Value = serde_json::from_slice(&out.stdout).unwrap();
        rows.as_array()
            .unwrap()
            .iter()
            .find(|r| r["holder_id"] == holder.as_str())
            .map(|r| r["hidden"].clone())
    };
    assert_eq!(hidden(), Some(json!(true)));

    rpc.forget();
    rpc.call("t1", json!({"type": "prompt", "text": "MOCK-TOOL please"}));
    let pr = rpc.event("permission_request", Duration::from_secs(30));
    assert_eq!(pr["tool_name"], "Bash", "{pr}");
    rpc.call(
        "t2",
        json!({"type": "answer_permission", "request_id": pr["request_id"], "decision": "allow"}),
    );
    let t = rpc.event("turn_end", Duration::from_secs(30));
    assert_eq!(t["aborted"], false, "{t}");
    assert!(
        env.work.join("mock-tool.txt").exists(),
        "允许之后 touch 没有执行"
    );

    let place = |verb: &str| {
        let out = ccrpc_cmd(&env)
            .args([verb, holder.as_str()])
            .output()
            .unwrap();
        assert!(out.status.success(), "{verb}: {out:?}");
    };
    place("show");
    assert_eq!(hidden(), Some(json!(false)));
    place("hide");
    assert_eq!(hidden(), Some(json!(true)));
}

/// An AskUserQuestion asked while no ccrpc holds its hook (the ccrpc that sent the prompt left):
/// the next ccrpc reads it from the transcript and answers it with keys on the dialog, for one
/// question and for two (tabbed, the second multi-select, submitted on the Review tab).
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_ask_before_attach() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let mut rpc = Rpc::start(&env);
    let ready = start_session(&mut rpc, &env);
    let holder = ready["holder_id"].as_str().unwrap().to_string();

    // Asks, then leaves before answering: the hook is let go and the dialog stays for a person.
    let ask_and_leave = |mut rpc: Rpc, id: &str, text: &str| -> (Value, Rpc) {
        rpc.forget();
        rpc.call(id, json!({"type": "prompt", "text": text}));
        let held = rpc.event("question", Duration::from_secs(30));
        let holder_pid = rpc.holder_pid.take();
        drop(rpc);
        let mut next = Rpc::attach(&env, &[&holder]);
        next.holder_pid = holder_pid;
        next.event("ready", Duration::from_secs(20));
        let q = next.event("question", Duration::from_secs(10));
        assert_eq!(q["request_id"], held["request_id"], "{q}");
        assert_eq!(q["questions"], held["questions"], "{q}");
        (q, next)
    };

    let (q, mut rpc) = ask_and_leave(rpc, "q1", "MOCK-ASK which");
    rpc.call("a1", json!({"type": "answer_question", "request_id": q["request_id"], "answers": [{"options": ["Blue"]}]}));
    rpc.event("turn_end", Duration::from_secs(30));

    let (q, mut rpc) = ask_and_leave(rpc, "q2", "MOCK-ASKS both");
    assert_eq!(q["questions"].as_array().map(Vec::len), Some(2), "{q}");
    rpc.call(
        "a2",
        json!({"type": "answer_question", "request_id": q["request_id"],
        "answers": [{"options": ["Red"]}, {"options": ["S", "L"]}]}),
    );
    rpc.event("turn_end", Duration::from_secs(30));

    let m = rpc.call("m1", json!({"type": "get_messages"}));
    let results: Vec<&str> = m["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["kind"] == "tool_result")
        .filter_map(|b| b["content"].as_str())
        .collect();
    assert!(
        results.iter().any(|c| c.contains("Blue")),
        "单题的结果里没有答案：{results:?}"
    );
    assert!(
        results.iter().any(|c| c.contains("Red")
            && c.contains('S')
            && c.contains('L')
            && !c.contains("\"M\"")),
        "两题的结果里没有答案：{results:?}"
    );
}

/// A follow-up queued while a background task ends during the turn's last answer: right after
/// Stop, Claude Code runs the task's notification as a turn of its own; the follow-up goes out
/// after that turn instead of racing it into Claude Code's queue (and being reported failed).
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_follow_up() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let mut rpc = Rpc::start(&env);
    start_session(&mut rpc, &env);

    rpc.forget();
    rpc.call("b1", json!({"type": "prompt", "text": "MOCK-BG go"}));
    // `sleep` runs without asking.
    rpc.until("tool_use", Duration::from_secs(30), |v| {
        v["kind"] == "tool_use"
    });
    let q = rpc.call("f1", json!({"type": "follow_up", "text": "AFTER-BG"}));
    assert_eq!(q["queued"], 1, "{q}");
    // The prompt's turn, the notification's, then the follow-up's.
    let mut ends = vec![];
    while ends.len() < 3 {
        let v = rpc.until("turn_end", Duration::from_secs(90), |v| {
            v["type"] == "turn_end" || v["type"] == "follow_up_failed"
        });
        assert_ne!(v["type"], "follow_up_failed", "{v}");
        ends.push(v);
    }
    let users: Vec<String> = rpc.call("m1", json!({"type": "get_messages"}))["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["kind"] == "user")
        .map(|b| b["text"].as_str().unwrap_or("").to_string())
        .collect();
    let notified = users
        .iter()
        .position(|t| t.contains("<task-notification>"))
        .unwrap_or_else(|| panic!("没有后台任务的通知：{users:?}"));
    let after = users
        .iter()
        .position(|t| t == "AFTER-BG")
        .unwrap_or_else(|| panic!("follow_up 没发出：{users:?}"));
    assert!(
        notified < after,
        "follow_up 应在通知那一轮之后发出：{users:?}"
    );
    // Sent as a prompt of its own, not typed into the notification's turn (Claude Code's queue).
    let transcript = std::fs::read_dir(env.cc_dir.join("projects"))
        .unwrap()
        .flatten()
        .flat_map(|d| std::fs::read_dir(d.path()).unwrap().flatten())
        .find(|f| f.path().extension().is_some_and(|e| e == "jsonl"))
        .map(|f| std::fs::read_to_string(f.path()).unwrap())
        .unwrap();
    let enqueues: Vec<&str> = transcript
        .lines()
        .filter(|l| l.contains(r#""operation":"enqueue""#) && !l.contains("task-notification"))
        .collect();
    assert!(
        enqueues.is_empty(),
        "follow_up 进了 Claude Code 的队列：{enqueues:?}"
    );
}

/// `exit` while a background task runs: Claude Code asks first; without `stop_tasks` the question is
/// dismissed and `exit` fails, with it the tasks are stopped and claude exits (on the scripted model).
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_exit_with_tasks() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let mut rpc = Rpc::start(&env);
    start_session(&mut rpc, &env);
    run_turn_allowing(&mut rpc, "bg", "MOCK-LONGTASK please");
    settle_idle(&mut rpc);
    rpc.send(json!({"id": "x1", "type": "exit"}));
    let r = rpc.until("x1", Duration::from_secs(30), |v| v["id"] == "x1");
    assert_eq!(r["error"]["code"], "not_idle", "{r}");
    // The question is dismissed: back at the input box, claude still running.
    wait_screen(&mut rpc, "x1s", |lines| {
        !lines
            .iter()
            .any(|l| l.contains("Background work is running"))
    });
    assert_eq!(
        rpc.call("x1g", json!({"type": "get_state"}))["state"],
        "idle"
    );
    rpc.call("x2", json!({"type": "exit", "stop_tasks": true}));
    rpc.event("session_exit", Duration::from_secs(20));
}

/// SIGTERM (CoDeck's Disconnect fallback) while idle ends ccrpc as a closed stdin does: it exits 0,
/// detached, and the Session can be attached again at once (on the scripted model).
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_sigterm_detaches() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let mut rpc = Rpc::start(&env);
    let ready = start_session(&mut rpc, &env);
    let holder = ready["holder_id"].as_str().unwrap().to_string();
    let pid = rpc.child.id();
    // SAFETY: plain kill(2) on this test's own ccrpc child, not reaped yet.
    unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(s) = rpc.child.try_wait().unwrap() {
            break s;
        }
        assert!(Instant::now() < deadline, "SIGTERM 后 ccrpc 没有退出");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "{status:?}");
    let holder_pid = rpc.holder_pid.take();
    drop(rpc);
    let mut next = Rpc::attach(&env, &["--co-display", &holder]);
    next.holder_pid = holder_pid;
    let r = next.event("ready", Duration::from_secs(20));
    assert_eq!(r["state"], "idle", "{r}");
}
