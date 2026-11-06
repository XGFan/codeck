//! Against a scripted local model (`mock_` tests: no key, no credits, deterministic timing).

use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::harness::*;

/// A scripted Anthropic Messages endpoint on loopback. A prompt containing `MOCK-SLOW` is answered
/// over ~8 s, `MOCK-FAIL` with HTTP 400, `MOCK-TOOL` with a Bash call (`touch mock-tool.txt`),
/// `MOCK-WRITE <path>` with a Write call,
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
                let tool = if asked("MOCK-WRITE") > called("Write") && has_tool("Write") {
                    // `MOCK-WRITE <path>`: writes `v<n>` to that file (n = Write calls so far).
                    let path = text
                        .split("MOCK-WRITE ")
                        .nth(1)
                        .and_then(|r| r.split_whitespace().next())
                        .unwrap_or_default()
                        .to_string();
                    let content = format!("v{}\n", called("Write"));
                    Some(("Write", json!({"file_path": path, "content": content})))
                } else if asked("MOCK-TOOL") > called("Bash") && has_tool("Bash") {
                    Some((
                        "Bash",
                        json!({"command": "touch mock-tool.txt", "description": "Create mock-tool.txt"}),
                    ))
                } else if asked("MOCK-LONGTASK") > called("Bash") && has_tool("Bash") {
                    Some((
                        "Bash",
                        json!({"command": "sleep 300", "description": "Wait long in background", "run_in_background": true}),
                    ))
                } else if asked("MOCK-STOPTASK") > called("TaskStop") {
                    // Stops the background shell a tool result above announced. TaskStop is a
                    // deferred tool (not in the request's tool list), still callable by name.
                    let id = user.iter().rev().find_map(|b| {
                        let t = match &b["content"] {
                            Value::String(t) => t.clone(),
                            Value::Array(a) => a
                                .iter()
                                .filter_map(|x| x["text"].as_str())
                                .collect::<Vec<_>>()
                                .join("\n"),
                            _ => return None,
                        };
                        let rest = &t[t.find("with ID: ")? + 9..];
                        let id: String = rest
                            .chars()
                            .take_while(|c| c.is_ascii_alphanumeric())
                            .collect();
                        (!id.is_empty()).then_some(id)
                    });
                    id.map(|id| ("TaskStop", json!({"task_id": id})))
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
                // Each reply its own id, as from the API (usage is counted once per id).
                static MSG_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                let id = format!(
                    "msg_mock{:04}",
                    MSG_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                );
                let msg = json!({"id": id, "type": "message", "role": "assistant", "model": model,
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

/// `edit_queue` (`↑` on the empty input box): what waits in Claude Code's queue comes back in the
/// response, oldest first, with an image's `[Image #n]`; the queue and the input box are empty
/// after it, and nothing of it runs after the turn.
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_edit_queue() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let mut rpc = Rpc::start(&env);
    start_session(&mut rpc, &env);
    let img = env.work.join("dot.png");
    std::fs::write(&img, red_dot_png()).unwrap();

    rpc.forget();
    rpc.call("s1", json!({"type": "prompt", "text": "MOCK-SLOW please"}));
    rpc.until("streaming", Duration::from_secs(10), |v| {
        v["type"] == "progress" && v["tokens"].as_u64().is_some_and(|n| n > 0)
    });
    rpc.call("s2", json!({"type": "steer", "text": "MOCK-STEERED first"}));
    rpc.call(
        "s3",
        json!({"type": "steer", "text": "MOCK-STEERED second", "images": [img]}),
    );
    rpc.until("queue", Duration::from_secs(10), |v| {
        v["type"] == "queue" && v["items"].as_array().is_some_and(|a| a.len() == 2)
    });

    let d = rpc.call("e1", json!({"type": "edit_queue"}));
    let items: Vec<String> = d["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| image_tags_as_hash(v.as_str().unwrap()))
        .collect();
    assert_eq!(items.len(), 2, "{d}");
    assert_eq!(items[0], "MOCK-STEERED first", "{d}");
    assert!(
        items[1].contains('#') && items[1].ends_with("MOCK-STEERED second"),
        "{d}"
    );
    rpc.until("queue", Duration::from_secs(5), |v| {
        v["type"] == "queue" && v["items"] == json!([])
    });
    let screen = rpc.call("g1", json!({"type": "get_screen"})).to_string();
    assert!(!screen.contains("MOCK-STEERED"), "{screen}");

    let t1 = rpc.event("turn_end", Duration::from_secs(30));
    assert_eq!(t1["aborted"], false, "{t1}");
    // No turn of the steers' own after it.
    std::thread::sleep(Duration::from_secs(4));
    while let Some(v) = rpc.recv(Duration::ZERO) {
        assert_ne!(v["type"], "turn_end", "{v}");
        assert!(!(v["type"] == "block" && v["text"] == "STEER-TURN"), "{v}");
    }
    let st = rpc.call("g2", json!({"type": "get_state"}));
    assert_eq!(st["state"], "idle", "{st}");
    // Nothing left to take back: refused without a key.
    rpc.send(json!({"type": "edit_queue", "id": "e2"}));
    let r = rpc.until("响应 e2", Duration::from_secs(5), |v| {
        v["type"] == "response" && v["id"] == "e2"
    });
    assert_eq!(r["error"]["code"], "not_busy", "{r}");
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

/// `[Image #n]` tags in `s` replaced by `#`, so the text around them can be compared (Claude Code
/// picks the numbers).
fn image_tags_as_hash(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(at) = rest.find("[Image #") {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 8..];
        let digits = tail.chars().take_while(char::is_ascii_digit).count();
        assert!(digits > 0 && tail[digits..].starts_with(']'), "{s}");
        out.push('#');
        rest = &tail[digits + 1..];
    }
    out.push_str(rest);
    out
}

/// `parts`: text and images pasted one by one land in that order, as a prompt and as a steer
/// while busy, on the scripted model (no credits).
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_interleaved_parts() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let mut rpc = Rpc::start(&env);
    start_session(&mut rpc, &env);
    let (a, b) = (env.work.join("a.png"), env.work.join("b.png"));
    std::fs::write(&a, red_dot_png()).unwrap();
    std::fs::write(&b, red_dot_png()).unwrap();

    rpc.forget();
    let d = rpc.call(
        "p1",
        json!({"type": "prompt", "parts": [
            {"text": "AAA "}, {"image": a}, {"text": " BBB "}, {"image": b}, {"text": " CCC"},
        ]}),
    );
    assert_eq!(d["delivered_as"], "prompt", "{d}");
    let uuid = d["uuid"].as_str().expect("没有 uuid").to_string();
    assert!(d["prompt_id"].is_string(), "{d}");
    let u = rpc.until("user block", Duration::from_secs(10), |v| {
        v["type"] == "block" && v["kind"] == "user" && v["uuid"] == uuid.as_str()
    });
    assert_eq!(
        image_tags_as_hash(u["text"].as_str().unwrap()),
        "AAA # BBB # CCC",
        "{u}"
    );
    assert_eq!(u["images"], json!(["image/png", "image/png"]), "{u}");
    rpc.event("turn_end", Duration::from_secs(30));

    // While busy: Claude Code's queue shows the text with the images in place.
    rpc.forget();
    rpc.call("s1", json!({"type": "prompt", "text": "MOCK-SLOW please"}));
    rpc.until("streaming", Duration::from_secs(10), |v| {
        v["type"] == "progress" && v["tokens"].as_u64().is_some_and(|n| n > 0)
    });
    let st = rpc.call(
        "s2",
        json!({"type": "steer", "parts": [
            {"text": "MOCK-STEERED see "}, {"image": a}, {"text": " and "}, {"image": b}, {"text": " now"},
        ]}),
    );
    assert_eq!(st["delivered_as"], "queued", "{st}");
    let q = rpc.until("queue", Duration::from_secs(10), |v| {
        v["type"] == "queue" && v["items"].as_array().is_some_and(|a| !a.is_empty())
    });
    assert_eq!(
        image_tags_as_hash(q["items"][0].as_str().unwrap_or_default()),
        "MOCK-STEERED see # and # now",
        "{q}"
    );
    // A follow-up with parts shows `[Image]` where its image goes while it waits.
    let f = rpc.call(
        "f1",
        json!({"type": "follow_up", "parts": [{"text": "later "}, {"image": a}]}),
    );
    assert_eq!(f["queued"], 1, "{f}");
    let g = rpc.call("g1", json!({"type": "get_state"}));
    assert_eq!(g["follow_ups"], json!(["later [Image]"]), "{g}");
    rpc.event("turn_end", Duration::from_secs(30));
    let t2 = rpc.event("turn_end", Duration::from_secs(30));
    assert_eq!(t2["last_text"], "STEER-TURN", "{t2}");
    let u = rpc.until("follow-up", Duration::from_secs(30), |v| {
        v["type"] == "follow_up_failed"
            || (v["type"] == "block"
                && v["kind"] == "user"
                && v["text"].as_str().is_some_and(|t| t.starts_with("later")))
    });
    assert_eq!(
        image_tags_as_hash(u["text"].as_str().unwrap_or_default()),
        "later #",
        "{u}"
    );
    rpc.event("turn_end", Duration::from_secs(30));
    settle_idle(&mut rpc);

    // After an image the box is a prompt, whatever the text starts with.
    let d = rpc.call(
        "p2",
        json!({"type": "prompt", "parts": [{"image": a}, {"text": "!!! 为什么"}]}),
    );
    assert!(d["uuid"].is_string(), "{d}");
    rpc.event("turn_end", Duration::from_secs(30));
    settle_idle(&mut rpc);

    // An image alone, no text.
    let d = rpc.call("p3", json!({"type": "prompt", "parts": [{"image": a}]}));
    assert!(d["uuid"].is_string(), "{d}");
    rpc.event("turn_end", Duration::from_secs(30));
    settle_idle(&mut rpc);

    // `!` or `/` first cannot go with an image (a `!` command; a slash command, or a prompt
    // Claude Code would send without the image): refused before anything is typed.
    for (id, first) in [("b1", "!ls "), ("b2", "/review 看这个截图 ")] {
        rpc.send(json!({"id": id, "type": "prompt", "parts": [{"text": first}, {"image": a}]}));
        let r = rpc.until(&format!("响应 {id}"), Duration::from_secs(10), |v| {
            v["type"] == "response" && v["id"] == id
        });
        assert_eq!(r["error"]["code"], "bad_request", "{r}");
    }
    let box_text = rpc.call("g2", json!({"type": "get_state"}));
    assert_eq!(box_text["state"], "idle", "{box_text}");
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
    // The reply on Fable named its model by id.
    assert_eq!(st["model"], "claude-fable-5-1", "{st}");
}

/// `/add-dir` asks before adding a directory: a choice from the built-in layout (no tool call is
/// pending, so it is not a tool approval), answered with `answer_choice`.
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_add_dir() {
    let env = setup_with("mock-key", &mock_model(), "claude-opus-4-7");
    let extra = tempfile::tempdir().unwrap();
    let extra = extra.path().canonicalize().unwrap();
    let mut rpc = Rpc::start(&env);
    start_session(&mut rpc, &env);
    rpc.forget();
    rpc.call(
        "s1",
        json!({"type": "slash", "command": format!("add-dir {}", extra.display())}),
    );
    let c = rpc.until("add-dir choice", Duration::from_secs(10), |v| {
        v["type"] == "choice" || v["type"] == "dialog"
    });
    assert_eq!(c["type"], "choice", "{c}");
    assert_eq!(c["title"], "Add directory to workspace", "{c}");
    assert_eq!(c["layout_source"], "builtin", "{c}");
    assert!(c["tool_use_id"].is_null(), "{c}");
    let labels: Vec<&str> = c["options"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|o| o["label"].as_str())
        .collect();
    assert_eq!(
        labels,
        [
            "Yes, for this session",
            "Yes, and remember this directory",
            "No"
        ],
        "{c}"
    );
    rpc.call(
        "c1",
        json!({"type": "answer_choice", "request_id": c["request_id"], "option": "Yes, for this session"}),
    );
    rpc.until("idle", Duration::from_secs(10), |v| {
        v["type"] == "state" && v["state"] == "idle"
    });
    let st = rpc.call("g1", json!({"type": "get_state"}));
    assert_eq!(st["state"], "idle", "{st}");
    assert!(st["request"].is_null(), "{st}");
    let screen = rpc.call("g2", json!({"type": "get_screen"})).to_string();
    assert!(screen.contains("Added "), "{screen}");
}

/// Rewind through the TUI's picker: `rewind` stops at the confirm page with what it offers,
/// `answer_rewind` restores (the file Write created goes away, the prompt returns to the input box)
/// or cancels; among prompts that look the same the one asked for is picked; no `dialog` goes out
/// for the picker meanwhile. On the scripted model (no credits).
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_rewind() {
    let env = setup_with("mock-key", &mock_model(), "claude-opus-4-7");
    // A project command that expands into a prompt (as a skill does).
    std::fs::create_dir_all(env.work.join(".claude/commands")).unwrap();
    std::fs::write(
        env.work.join(".claude/commands/greet.md"),
        "Say hello to $ARGUMENTS\n",
    )
    .unwrap();
    let mut rpc = Rpc::start(&env);
    start_session(&mut rpc, &env);
    let w = env.work.join("w.txt");
    let write = format!("MOCK-WRITE {} now", w.display());
    run_turn_allowing(&mut rpc, "p1", "first plain");
    run_turn_allowing(&mut rpc, "p2", &write);
    run_turn_allowing(&mut rpc, "p3", "continue");
    run_turn_allowing(&mut rpc, "p4", "continue");
    settle_idle(&mut rpc);
    assert_eq!(std::fs::read_to_string(&w).unwrap(), "v0\n");
    let msgs = rpc.call("m1", json!({"type": "get_messages"}));
    let prompts: Vec<(String, String)> = msgs["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["kind"] == "user")
        .map(|b| {
            (
                b["uuid"].as_str().unwrap().to_string(),
                b["text"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let uuid_of = |i: usize| prompts[i].0.clone();
    assert_eq!(prompts.len(), 4, "{prompts:?}");

    // Not a prompt of the Main Line.
    rpc.forget();
    rpc.send(json!({"type": "rewind", "uuid": "no-such-uuid", "id": "r0"}));
    let r = rpc.until("r0", Duration::from_secs(30), |v| v["id"] == "r0");
    assert_eq!(r["error"]["code"], "not_found", "{r}");

    // The older of two `continue`: nothing written after it, so only the conversation.
    rpc.forget();
    let c = rpc.call("r1", json!({"type": "rewind", "uuid": uuid_of(2)}));
    assert_eq!(c["text"], "continue", "{c}");
    assert_eq!(c["code"], "The code will be unchanged.", "{c}");
    assert_eq!(
        c["options"],
        json!([{"option": "conversation", "label": "Restore conversation"}])
    );
    let st = rpc.call("g1", json!({"type": "get_state"}));
    assert_eq!(st["request"]["request_id"], c["request_id"], "{st}");
    assert_eq!(st["request"]["type"], "rewind", "{st}");
    // Cancelled: back to idle, nothing changed.
    let a = rpc.call(
        "a1",
        json!({"type": "answer_rewind", "request_id": c["request_id"], "cancel": true}),
    );
    assert_eq!(a["state"], "idle", "{a}");
    assert!(
        !rpc.seen
            .iter()
            .any(|v| v["type"] == "dialog" || v["type"] == "unrecognized"),
        "rewind 期间不该上报对话框：{:?}",
        rpc.seen
    );

    // The prompt that wrote the file: code and conversation can go back.
    rpc.forget();
    let c = rpc.call("r2", json!({"type": "rewind", "uuid": uuid_of(1)}));
    assert_eq!(c["text"], write.as_str(), "{c}");
    let options: Vec<&str> = c["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["option"].as_str().unwrap())
        .collect();
    assert_eq!(options, ["both", "conversation", "code"], "{c}");
    assert!(c["code"].as_str().unwrap().contains("w.txt"), "{c}");
    assert!(c["warning"].is_string(), "{c}");
    let a = rpc.call(
        "a2",
        json!({"type": "answer_rewind", "request_id": c["request_id"], "option": "both"}),
    );
    assert_eq!(a["state"], "idle", "{a}");
    assert_eq!(a["text"], write.as_str(), "{a}");
    assert!(!w.exists(), "Write 建的文件应当被恢复掉");
    // The input box (its last `❯` row) holds the prompt again.
    wait_screen(&mut rpc, "s1", |ls| {
        ls.iter()
            .rfind(|l| l.starts_with('❯'))
            .is_some_and(|l| l.contains("MOCK-WRITE") && l.contains("w.txt now"))
    });
    assert!(
        !rpc.seen
            .iter()
            .any(|v| v["type"] == "dialog" || v["type"] == "unrecognized"),
        "{:?}",
        rpc.seen
    );
    // The answer was answered: no request left.
    let st = rpc.call("g2", json!({"type": "get_state"}));
    assert!(st["request"].is_null(), "{st}");

    // A prompt from here starts another Branch.
    rpc.forget();
    rpc.call("p5", json!({"type": "prompt", "text": "after rewind"}));
    let b = rpc.event("branch", Duration::from_secs(30));
    assert!(b["at_uuid"].is_string(), "{b}");
    rpc.event("turn_end", Duration::from_secs(30));

    // Code only: the file goes back, the conversation and the input box stay.
    let w2 = env.work.join("w2.txt");
    let write2 = format!("MOCK-WRITE {} code-only", w2.display());
    run_turn_allowing(&mut rpc, "p6", &write2);
    settle_idle(&mut rpc);
    let msgs = rpc.call("m2", json!({"type": "get_messages"}));
    let last_prompt = msgs["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .rfind(|b| b["kind"] == "user")
        .unwrap()["uuid"]
        .clone();
    let c = rpc.call("r3", json!({"type": "rewind", "uuid": last_prompt}));
    let a = rpc.call(
        "a3",
        json!({"type": "answer_rewind", "request_id": c["request_id"], "option": "code"}),
    );
    assert!(a["text"].is_null(), "{a}");
    assert!(!w2.exists(), "只恢复代码也应当去掉 w2.txt");

    // A newer prompt whose row looks like the target's (its first line, cut with `…`): its page
    // quotes something else, so the walk goes back and on to the target.
    run_turn_allowing(&mut rpc, "p7", "Please review the PR");
    run_turn_allowing(&mut rpc, "p8", "Please review\nthe diff");
    settle_idle(&mut rpc);
    let msgs = rpc.call("m3", json!({"type": "get_messages"}));
    let review = msgs["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["kind"] == "user" && b["text"] == "Please review the PR")
        .unwrap()["uuid"]
        .clone();
    let c = rpc.call("r5", json!({"type": "rewind", "uuid": review}));
    assert_eq!(c["text"], "Please review the PR", "{c}");
    let page = wait_screen(&mut rpc, "s2", |ls| {
        ls.iter().any(|l| l.contains("│ Please"))
    });
    assert!(
        page.iter()
            .any(|l| l.trim_end().ends_with("│ Please review the PR")),
        "{page:#?}"
    );
    let a = rpc.call(
        "a5",
        json!({"type": "answer_rewind", "request_id": c["request_id"], "cancel": true}),
    );
    assert_eq!(a["state"], "idle", "{a}");

    // Esc in the TUI on the confirm page: the rewind is over, the picker is a dialog as usual.
    rpc.forget();
    let c = rpc.call("r4", json!({"type": "rewind", "uuid": last_prompt}));
    rpc.call("k1", json!({"type": "send_keys", "keys": ["esc"]}));
    rpc.until("picker dialog", Duration::from_secs(5), |v| {
        v["type"] == "dialog"
    });
    let st = rpc.call("g3", json!({"type": "get_state"}));
    assert!(st["request"]["type"] != "rewind", "{st}");
    rpc.send(
        json!({"type": "answer_rewind", "request_id": c["request_id"], "cancel": true, "id": "a4"}),
    );
    let r = rpc.until("a4", Duration::from_secs(10), |v| v["id"] == "a4");
    assert_eq!(r["error"]["code"], "unknown_request", "{r}");
    rpc.call("k2", json!({"type": "send_keys", "keys": ["esc"]}));
    wait_state(&mut rpc, "idle", 10);
    settle_idle(&mut rpc);

    // Commands rewind too, and return to the input box as typed. A `!` command sent with
    // `prompt` runs in the TUI's shell mode, as a turn of its own.
    rpc.forget();
    rpc.call("c1", json!({"type": "slash", "command": "greet world"}));
    rpc.event("turn_end", Duration::from_secs(30));
    settle_idle(&mut rpc);
    rpc.forget();
    let r = rpc.call("c2", json!({"type": "prompt", "text": "! echo bang"}));
    assert!(r["uuid"].is_string(), "{r}");
    assert!(r["prompt_id"].is_null(), "{r}");
    rpc.event("turn_end", Duration::from_secs(30));
    settle_idle(&mut rpc);
    let entry_uuid = |marker: &str| -> String {
        let dir = std::fs::read_dir(env.cc_dir.join("projects")).unwrap();
        for d in dir.flatten() {
            for f in std::fs::read_dir(d.path()).unwrap().flatten() {
                if f.path().extension().is_none_or(|x| x != "jsonl") {
                    continue;
                }
                for line in std::fs::read_to_string(f.path()).unwrap().lines() {
                    if !line.contains(marker) {
                        continue;
                    }
                    let v: Value = serde_json::from_str(line).unwrap();
                    if let (Some(u), Some("user" | "system")) =
                        (v["uuid"].as_str(), v["type"].as_str())
                    {
                        return u.to_string();
                    }
                }
            }
        }
        panic!("transcript 里没有 {marker}");
    };
    let bang = entry_uuid("<bash-input>echo bang</bash-input>");
    let greet = entry_uuid("<command-name>/greet</command-name>");
    let c = rpc.call("r6", json!({"type": "rewind", "uuid": bang}));
    assert_eq!(c["text"], "! echo bang", "{c}");
    let a = rpc.call(
        "a6",
        json!({"type": "answer_rewind", "request_id": c["request_id"], "option": "conversation"}),
    );
    assert_eq!(a["text"], "! echo bang", "{a}");
    // Back in the TUI's input box in shell mode; a prompt leaves it first.
    wait_screen(&mut rpc, "s3", |ls| {
        ls.iter()
            .rfind(|l| l.starts_with('!'))
            .is_some_and(|l| l.contains("echo bang"))
    });
    run_turn_allowing(&mut rpc, "p10", "after bang");
    settle_idle(&mut rpc);
    let c = rpc.call("r7", json!({"type": "rewind", "uuid": greet}));
    assert_eq!(c["text"], "/greet world", "{c}");
    let a = rpc.call(
        "a7",
        json!({"type": "answer_rewind", "request_id": c["request_id"], "option": "conversation"}),
    );
    assert_eq!(a["text"], "/greet world", "{a}");
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
    rpc.shell_pid = shell_pid(&env, ready["holder_id"].as_str().unwrap());

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
    let shell_pid = rpc.shell_pid.unwrap();
    rpc.call("x1", json!({"type": "exit", "close_pane": true}));
    let deadline = Instant::now() + Duration::from_secs(10);
    while transcript::live::pid_exists(shell_pid) {
        assert!(Instant::now() < deadline, "Pane 没有随 exit 结束");
        std::thread::sleep(Duration::from_millis(200));
    }
    rpc.shell_pid = None;
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

/// A plugin's skill typed by its bare name (`/probe x`) is recorded with the plugin's name
/// (`/<plugin>:probe x`); `prompt` still finds it (on the scripted model, no credits).
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_plugin_command_by_bare_name() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let skill = env.plugin_dir.join("skills").join("probe");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: probe\ndescription: A probe skill for tests.\n---\nReply ok.\n",
    )
    .unwrap();
    let mut rpc = Rpc::start(&env);
    start_session(&mut rpc, &env);

    rpc.forget();
    let r = rpc.call(
        "p1",
        json!({"type": "prompt", "text": "/probe 第一行\n第二行"}),
    );
    assert!(r["uuid"].as_str().is_some(), "{r}");
    assert!(r["prompt_id"].as_str().is_some(), "{r}");
    rpc.event("turn_end", Duration::from_secs(30));
}

/// A hidden Pane (`new --hidden`, ToH ADR-0050), as CoDeck starts it: nobody shows it, so its PTY keeps the size
/// it starts with, ccrpc's 120×40, and an approval dialog is recognized there; `ls --json` reports `hidden`, which
/// `hide` / `show` toggle (on the scripted model, no credits).
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_hidden() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let mut rpc = Rpc::start_in(&env, &env.work, &["--hidden"]);
    let ready = start_session(&mut rpc, &env);
    assert_eq!(ready["size"], json!([120, 40]), "{ready}");
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
        let shell_pid = rpc.shell_pid.take();
        drop(rpc);
        let mut next = Rpc::attach(&env, &[&holder]);
        next.shell_pid = shell_pid;
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

/// `Chat about this` (`answer_question` with `chat`): the hook is let go and the dialog answered
/// with keys, the answers given so far kept; Claude Code records the call as rejected, listing
/// them, and the turn goes on to the model's reply. Two questions with only the second answered
/// (the first passed with `right`), with only the first answered, and a single question.
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_ask_chat() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let mut rpc = Rpc::start(&env);
    start_session(&mut rpc, &env);
    let chat = |rpc: &mut Rpc, n: usize, text: &str, answers: Value| -> String {
        rpc.forget();
        rpc.call(&format!("p{n}"), json!({"type": "prompt", "text": text}));
        let q = rpc.event("question", Duration::from_secs(30));
        rpc.call(
            &format!("c{n}"),
            json!({"type": "answer_question", "request_id": q["request_id"], "chat": true, "answers": answers}),
        );
        let t = rpc.event("turn_end", Duration::from_secs(30));
        assert_eq!(t["aborted"], false, "{t}");
        let m = rpc.call(&format!("m{n}"), json!({"type": "get_messages"}));
        m["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .rfind(|b| b["kind"] == "tool_result")
            .and_then(|b| b["content"].as_str())
            .unwrap_or_default()
            .to_string()
    };

    // Every question answered: refused before any key, the dialog still up.
    rpc.forget();
    rpc.call("p0", json!({"type": "prompt", "text": "MOCK-ASK which"}));
    let q = rpc.event("question", Duration::from_secs(30));
    rpc.send(
        json!({"id": "c0", "type": "answer_question", "request_id": q["request_id"],
        "chat": true, "answers": [{"options": ["Red"]}]}),
    );
    let r = rpc.until("c0", Duration::from_secs(10), |v| v["id"] == "c0");
    assert_eq!(r["error"]["code"], "bad_request", "{r}");
    // None answered: the call may leave `answers` out.
    rpc.call(
        "a0",
        json!({"type": "answer_question", "request_id": q["request_id"], "chat": true}),
    );
    rpc.event("turn_end", Duration::from_secs(30));

    let r = chat(
        &mut rpc,
        1,
        "MOCK-ASKS both",
        json!([null, {"options": ["S", "L"]}]),
    );
    assert!(
        r.contains("The user wants to clarify these questions."),
        "{r}"
    );
    assert!(
        r.contains("- \"Which color?\"\n  (No answer provided)"),
        "{r}"
    );
    assert!(r.contains("- \"Which sizes?\"\n  Answer: S, L"), "{r}");

    let r = chat(
        &mut rpc,
        2,
        "MOCK-ASKS both",
        json!([{"options": ["Blue"]}, null]),
    );
    assert!(r.contains("- \"Which color?\"\n  Answer: Blue"), "{r}");
    assert!(
        r.contains("- \"Which sizes?\"\n  (No answer provided)"),
        "{r}"
    );

    let r = chat(&mut rpc, 3, "MOCK-ASK which", json!([null]));
    assert!(
        r.contains("The user wants to clarify these questions."),
        "{r}"
    );
    assert!(
        r.contains("- \"Which color?\"\n  (No answer provided)"),
        "{r}"
    );
}

/// Esc before any reply: Claude Code takes the prompt out of the conversation and puts it back in
/// its input box; the aborted `turn_end` says so (`restored_text`). Once the reply has started the
/// prompt stays (`Interrupted`), the box is empty: null.
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_abort_restores_the_prompt() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let mut rpc = Rpc::start(&env);
    start_session(&mut rpc, &env);

    rpc.forget();
    rpc.call(
        "p1",
        json!({"type": "prompt", "text": "MOCK-SLOW taken back"}),
    );
    rpc.call("a1", json!({"type": "abort"}));
    let t = rpc.event("turn_end", Duration::from_secs(15));
    assert_eq!(t["aborted"], true, "{t}");
    assert_eq!(t["restored_text"], "MOCK-SLOW taken back", "{t}");

    // The next prompt clears the box first.
    rpc.forget();
    rpc.call("p2", json!({"type": "prompt", "text": "MOCK-SLOW kept"}));
    // MOCK-SLOW streams for ~8 s (its block comes once written): well into it.
    std::thread::sleep(Duration::from_secs(3));
    rpc.call("a2", json!({"type": "abort"}));
    let t = rpc.event("turn_end", Duration::from_secs(15));
    assert_eq!(t["aborted"], true, "{t}");
    assert_eq!(t["restored_text"], Value::Null, "{t}");
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

/// While a command waits (here `prompt` of a slash command that opens a panel and leaves no record,
/// ~30 s), commands that neither write nor wait are answered at once, in between, and `abort` runs
/// at once; anything after a command that has to wait waits too (on the scripted model).
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_reads_while_waiting() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let mut rpc = Rpc::start(&env);
    start_session(&mut rpc, &env);

    rpc.forget();
    rpc.send(json!({"id": "w1", "type": "prompt", "text": "/status"}));
    // The panel is up (a dialog): the prompt now waits for the transcript.
    wait_state(&mut rpc, "waiting", 10);
    let answered = |rpc: &Rpc, id: &str| {
        rpc.seen
            .iter()
            .any(|v| v["type"] == "response" && v["id"] == id)
    };
    let t = Instant::now();
    let st = rpc.call("g1", json!({"type": "get_state"}));
    assert!(st["state"].is_string(), "{st}");
    let q = rpc.call("f1", json!({"type": "follow_up", "text": "LATER"}));
    assert_eq!(q["queued"], 1, "{q}");
    let c = rpc.call("c1", json!({"type": "cancel_follow_up"}));
    assert_eq!(c["removed"], 1, "{c}");
    assert!(
        t.elapsed() < Duration::from_secs(3),
        "等待中的命令期间只读命令没有立即作答：{:?}",
        t.elapsed()
    );
    assert!(!answered(&rpc, "w1"), "prompt 没在等待：{:?}", rpc.seen);
    // The prompt pressed its last key and only waits: `abort` runs now (Esc closes the panel),
    // and the prompt still ends on its own afterwards.
    let t = Instant::now();
    let a = rpc.call("a1", json!({"type": "abort"}));
    assert!(
        t.elapsed() < Duration::from_secs(3),
        "等待中的命令期间 abort 没有立即执行：{:?}",
        t.elapsed()
    );
    assert_eq!(a["state"], "idle", "{a}");
    assert!(!answered(&rpc, "w1"), "prompt 在 abort 之前就结束了");
    // A command that writes waits for the prompt (already in manual mode: set_mode presses no
    // key), and so does a read sent after it (never ahead of a command received earlier).
    rpc.send(json!({"id": "k2", "type": "set_mode", "mode": "manual"}));
    rpc.send(json!({"id": "g2", "type": "get_state"}));
    let r = rpc.until("w1", Duration::from_secs(40), |v| v["id"] == "w1");
    assert_eq!(r["error"]["code"], "timeout", "{r}");
    assert!(
        !answered(&rpc, "k2") && !answered(&rpc, "g2"),
        "排在后面的命令先于 prompt 作答：{:?}",
        rpc.seen
    );
    let k = rpc.until("k2", Duration::from_secs(5), |v| v["id"] == "k2");
    assert_eq!(k["success"], true, "{k}");
    assert!(
        !answered(&rpc, "g2"),
        "get_state 先于更早收到的 set_mode 作答"
    );
    let g = rpc.until("g2", Duration::from_secs(5), |v| v["id"] == "g2");
    assert_eq!(g["data"]["state"], "idle", "{g}");
    settle_idle(&mut rpc);
}

/// `abort` sent while a follow-up goes out ends the follow-up's turn (it runs once the follow-up
/// has been typed, during the wait for the transcript or right after it), and the follow-up is not
/// reported failed (on the scripted model).
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_abort_follow_up() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let mut rpc = Rpc::start(&env);
    start_session(&mut rpc, &env);

    rpc.forget();
    rpc.call("s1", json!({"type": "prompt", "text": "MOCK-SLOW please"}));
    rpc.call(
        "f1",
        json!({"type": "follow_up", "text": "MOCK-SLOW again"}),
    );
    let t1 = rpc.event("turn_end", Duration::from_secs(60));
    assert_eq!(t1["aborted"], false, "{t1}");
    // The follow-up is being sent: its prompt shows up in the transcript.
    rpc.until("follow-up", Duration::from_secs(20), |v| {
        v["type"] == "block" && v["kind"] == "user" && v["text"] == "MOCK-SLOW again"
    });
    rpc.send(json!({"id": "a1", "type": "abort"}));
    let a = rpc.until("a1", Duration::from_secs(15), |v| v["id"] == "a1");
    assert_eq!(a["success"], true, "{a}");
    let t2 = rpc.until("turn_end", Duration::from_secs(15), |v| {
        v["type"] == "turn_end" || v["type"] == "follow_up_failed"
    });
    assert_eq!(t2["type"], "turn_end", "{t2}");
    assert_eq!(t2["aborted"], true, "{t2}");
    assert!(
        !t2["last_text"].as_str().is_some_and(|t| t.contains("slow")),
        "{t2}"
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

/// A background shell stopped with TaskStop ends in get_tasks: Claude Code sends no notification for
/// it, so ccrpc takes the tool's result as its end.
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_task_stop_ends_the_task() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let mut rpc = Rpc::start(&env);
    start_session(&mut rpc, &env);
    run_turn_allowing(&mut rpc, "bg", "MOCK-LONGTASK please");
    settle_idle(&mut rpc);
    let shell = |rpc: &mut Rpc, id: &str| {
        let tasks = rpc.call(id, json!({"type": "get_tasks"}));
        tasks["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["kind"] == "shell" && t["command"] == "sleep 300")
            .unwrap_or_else(|| panic!("没有那个后台 shell：{tasks}"))
            .clone()
    };
    assert_eq!(shell(&mut rpc, "t1")["status"], "running");
    run_turn_allowing(&mut rpc, "stop", "MOCK-STOPTASK please");
    settle_idle(&mut rpc);
    assert_eq!(shell(&mut rpc, "t2")["status"], "stopped");
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
    let shell_pid = rpc.shell_pid.take();
    drop(rpc);
    let mut next = Rpc::attach(&env, &[&holder]);
    next.shell_pid = shell_pid;
    let r = next.event("ready", Duration::from_secs(20));
    assert_eq!(r["state"], "idle", "{r}");
}

/// The Pane is shared with a person (ADR-0001): their Client sets the PTY size and ccrpc follows it;
/// writing goes on meanwhile. Too small a Pane to read the approval dialog on makes ccrpc switch
/// it to 120×40 and read the dialog there. Only one ccrpc per Session; the old options are gone
/// (on the scripted model).
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_shared_pane() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let mut rpc = Rpc::start(&env);
    let ready = start_session(&mut rpc, &env);
    assert_eq!(
        ready["size"],
        json!([120, 40]),
        "new starts at the work size: {ready}"
    );
    let holder = ready["holder_id"].as_str().unwrap().to_string();

    // A person shows the Pane at a size far too small for the dialog: the VT follows.
    let _person = person_resizes(&env, &holder, 40, 12);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let st = rpc.call("s0", json!({"type": "get_state"}));
        if st["size"] == json!([40, 12]) {
            let scr = rpc.call("s1", json!({"type": "get_screen"}));
            assert_eq!(scr["lines"].as_array().unwrap().len(), 12);
            break;
        }
        assert!(Instant::now() < deadline, "没有跟随人设的尺寸：{st}");
        std::thread::sleep(Duration::from_millis(200));
    }

    // Still writes; the approval's options are read at 120×40.
    rpc.forget();
    rpc.call("t1", json!({"type": "prompt", "text": "MOCK-TOOL please"}));
    let pr = rpc.event("permission_request", Duration::from_secs(30));
    assert_eq!(pr["tool_name"], "Bash", "{pr}");
    let meanings: Vec<&str> = pr["options"]
        .as_array()
        .unwrap_or_else(|| panic!("permission_request 没带 options：{pr}"))
        .iter()
        .filter_map(|o| o["meaning"].as_str())
        .collect();
    assert!(meanings.contains(&"allow_always"), "{pr}");
    let st = rpc.call("t2", json!({"type": "get_state"}));
    assert_eq!(st["size"], json!([120, 40]), "{st}");
    rpc.call(
        "t3",
        json!({"type": "answer_permission", "request_id": pr["request_id"], "decision": "allow"}),
    );
    let t = rpc.event("turn_end", Duration::from_secs(30));
    assert_eq!(t["aborted"], false, "{t}");

    // One ccrpc per Session; the removed options are refused.
    for args in [vec![holder.as_str()], vec!["--co-display", holder.as_str()]] {
        let out = ccrpc_cmd(&env)
            .arg("attach")
            .args(&args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(!out.status.success(), "{args:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("已有 ccrpc 连着") || err.contains("用法：ccrpc attach"),
            "{args:?}: {err}"
        );
    }
}

/// The background claude processes the Pane's claude `pane_claude` started (the transient daemon,
/// keyed to this test's own config dir, with its PTY hosts and claude processes): stopped when the
/// test is done, once the Pane's claude is gone (it starts the daemon again while it runs).
struct Daemons {
    pane_claude: Option<u32>,
}

impl Drop for Daemons {
    fn drop(&mut self) {
        let Some(pane_claude) = self.pane_claude else {
            return;
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while transcript::live::pid_exists(pane_claude) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
        }
        let out = std::process::Command::new("/bin/ps")
            .args(["-A", "-o", "pid=,ppid=,command="])
            .output()
            .unwrap();
        let table: Vec<(u32, u32, String)> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                let pid = it.next()?.parse().ok()?;
                let ppid = it.next()?.parse().ok()?;
                Some((pid, ppid, it.collect::<Vec<_>>().join(" ")))
            })
            .collect();
        let spawned = format!("\"pid\":{pane_claude}}}");
        let mut ours: Vec<u32> = table
            .iter()
            .filter(|(_, _, c)| c.contains("daemon run") && c.contains(&spawned))
            .map(|(p, _, _)| *p)
            .collect();
        loop {
            let more: Vec<u32> = table
                .iter()
                .filter(|(p, pp, _)| ours.contains(pp) && !ours.contains(p))
                .map(|(p, _, _)| *p)
                .collect();
            if more.is_empty() {
                break;
            }
            ours.extend(more);
        }
        let signal = |sig| {
            for p in &ours {
                // SAFETY: only processes descended from this test's own Pane's claude.
                unsafe { libc::kill(*p as i32, sig) };
            }
        };
        signal(libc::SIGTERM);
        let deadline = Instant::now() + Duration::from_secs(10);
        while ours.iter().any(|p| transcript::live::pid_exists(*p)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
        }
        signal(libc::SIGKILL);
        // The daemon's sockets: `/tmp/cc-daemon-<uid>/<hash of this test's config dir>`, named
        // on its processes' command lines.
        // SAFETY: getuid has no preconditions.
        let base = format!("/tmp/cc-daemon-{}/", unsafe { libc::getuid() });
        let dir = table
            .iter()
            .filter(|(p, _, _)| ours.contains(p))
            .find_map(|(_, _, c)| {
                let hash: String = c[c.find(&base)? + base.len()..]
                    .chars()
                    .take_while(char::is_ascii_hexdigit)
                    .collect();
                (hash.len() == 8).then(|| format!("{base}{hash}"))
            });
        if let Some(d) = dir {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

/// `←` on an empty input box moves the conversation to the background (agents page): the Pane's
/// claude stays as the front end of a background claude that runs it as another Session. ccrpc
/// follows it (`session_changed`); the agents page has no input box; opened again, a prompt runs
/// there to its end. `ls` reports the background Session for the Holder; a second ccrpc is still
/// refused.
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_follows_the_background_session() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    // Dropped after `rpc`, which ends the Pane.
    let mut daemons = Daemons { pane_claude: None };
    let mut rpc = Rpc::start(&env);
    let ready = start_session(&mut rpc, &env);
    let holder = ready["holder_id"].as_str().unwrap().to_string();
    rpc.call("p1", json!({"type": "prompt", "text": "hello one"}));
    let t1 = rpc.event("turn_end", Duration::from_secs(30));
    let old = rpc.call("g1", json!({"type": "get_state"}))["session_id"].clone();
    assert!(old.is_string(), "{old}");
    let ls = || -> Value {
        let out = ccrpc_cmd(&env)
            .args(["ls", "--json"])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let rows: Value = serde_json::from_slice(&out.stdout).unwrap();
        rows.as_array()
            .unwrap()
            .iter()
            .find(|r| r["holder_id"] == holder)
            .cloned()
            .unwrap()
    };
    daemons.pane_claude = ls()["claude_pid"].as_u64().map(|p| p as u32);

    rpc.forget();
    rpc.call("k1", json!({"type": "send_keys", "keys": ["left"]}));
    let ch = rpc.event("session_changed", Duration::from_secs(20));
    assert_eq!(ch["previous"], old, "{ch}");
    let new = ch["session_id"].clone();
    assert!(new.is_string() && new != old, "{ch}");
    wait_screen(&mut rpc, "w1", |l| {
        l.iter().any(|l| l.contains("moved to the background"))
    });
    // The agents page's box starts another background Session: nothing is typed into it.
    rpc.send(json!({"id": "p2", "type": "prompt", "text": "not typed"}));
    let r = rpc.until("p2", Duration::from_secs(30), |v| {
        v["type"] == "response" && v["id"] == "p2"
    });
    assert_eq!(r["error"]["code"], "unrecognized_screen", "{r}");

    rpc.call("k2", json!({"type": "send_keys", "keys": ["esc"]}));
    wait_screen(&mut rpc, "w2", |l| {
        l.iter().any(|l| l.contains("← for agents"))
    });
    rpc.forget();
    let d = rpc.call("p3", json!({"type": "prompt", "text": "hello two"}));
    let t = rpc.event("turn_end", Duration::from_secs(30));
    assert_eq!(t["prompt_id"], d["prompt_id"], "{t}");
    assert_eq!(t["aborted"], false, "{t}");
    assert_eq!(t["last_text"], "ok", "{t}");
    // The new transcript only appeared with this prompt, starting with a copy of the old one:
    // none of the copy comes again, nor counts in this turn.
    let blocks: Vec<&Value> = rpc.seen.iter().filter(|v| v["type"] == "block").collect();
    let users: Vec<&Value> = blocks
        .iter()
        .filter(|b| b["kind"] == "user")
        .map(|b| &b["text"])
        .collect();
    assert_eq!(users, [&json!("hello two")], "{blocks:#?}");
    let texts = blocks.iter().filter(|b| b["kind"] == "text").count();
    assert_eq!(texts, 1, "{blocks:#?}");
    assert_eq!(t["usage"], t1["usage"], "{t} / {t1}");
    assert_eq!(t["api_cost"], t1["api_cost"], "{t} / {t1}");
    let st = rpc.call("g2", json!({"type": "get_state"}));
    assert_eq!(st["session_id"], new, "{st}");
    assert_eq!(st["state"], "idle", "{st}");

    // Slash commands in the background Session (while one is typed its suggestions show above
    // the box; the mode line stays under it), then a prompt: none of them is left in the box.
    let extra = tempfile::tempdir().unwrap();
    rpc.forget();
    rpc.call(
        "s1",
        json!({"type": "slash", "command": format!("add-dir {}", extra.path().display())}),
    );
    let c = rpc.until("add-dir choice", Duration::from_secs(10), |v| {
        v["type"] == "choice" || v["type"] == "dialog"
    });
    assert_eq!(c["title"], "Add directory to workspace", "{c}");
    rpc.call(
        "c1",
        json!({"type": "answer_choice", "request_id": c["request_id"], "option": "Yes, for this session"}),
    );
    wait_state(&mut rpc, "idle", 10);
    rpc.forget();
    rpc.call(
        "m1",
        json!({"type": "set_model", "model": "claude-opus-4-7"}),
    );
    // Mid-conversation Claude Code asks first.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(v) = rpc.recv(Duration::from_millis(200))
            && v["type"] == "choice"
        {
            rpc.call(
                "c2",
                json!({"type": "answer_choice", "request_id": v["request_id"], "option": v["options"][0]["label"]}),
            );
            break;
        }
    }
    settle_idle(&mut rpc);
    rpc.forget();
    let d = rpc.call("p4", json!({"type": "prompt", "text": "hello three"}));
    let t = rpc.event("turn_end", Duration::from_secs(30));
    assert_eq!(t["prompt_id"], d["prompt_id"], "{t}");
    let st = rpc.call("g3", json!({"type": "get_state"}));
    assert_eq!(st["model"], "claude-opus-4-7", "{st}");

    let row = ls();
    assert_eq!(row["session_id"], new, "{row}");
    assert_eq!(row["plugin"], true, "{row}");
    // By Holder or by the background Session's id (found through the Pane whose claude moved
    // it): either way already connected.
    for target in [holder.as_str(), new.as_str().unwrap()] {
        let out = ccrpc_cmd(&env)
            .args(["attach", target])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("已有 ccrpc 连着"),
            "{target}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// Moved to the background while a reply streams: the background claude finishes the reply
/// (without a UserPromptSubmit of its own); the turn ends there, under the caller's prompt id.
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_follows_a_running_turn_into_the_background() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let mut daemons = Daemons { pane_claude: None };
    let mut rpc = Rpc::start(&env);
    start_session(&mut rpc, &env);
    let pane_claude = ccrpc_cmd(&env)
        .args(["ls", "--json"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let rows: Value = serde_json::from_slice(&pane_claude.stdout).unwrap();
    daemons.pane_claude = rows[0]["claude_pid"].as_u64().map(|p| p as u32);

    rpc.forget();
    let d = rpc.call("p1", json!({"type": "prompt", "text": "MOCK-SLOW please"}));
    rpc.until("streaming", Duration::from_secs(10), |v| {
        v["type"] == "progress" && v["tokens"].as_u64().is_some_and(|n| n > 0)
    });
    rpc.call("k1", json!({"type": "send_keys", "keys": ["left"]}));
    let ch = rpc.event("session_changed", Duration::from_secs(20));
    assert!(ch["previous"].is_string(), "{ch}");
    let from = rpc.seen.len();
    let t = rpc.event("turn_end", Duration::from_secs(60));
    assert_eq!(t["prompt_id"], d["prompt_id"], "{t}");
    assert_eq!(t["aborted"], false, "{t}");
    assert!(
        t["last_text"].as_str().unwrap_or("").contains("slow"),
        "{t}"
    );
    // The reply the background claude finished comes as a block; the copied prompt does not
    // come again.
    let blocks: Vec<&Value> = rpc.seen[from..]
        .iter()
        .filter(|v| v["type"] == "block")
        .collect();
    let replies = blocks
        .iter()
        .filter(|b| b["kind"] == "text" && b["text"].as_str().is_some_and(|t| t.contains("slow")))
        .count();
    assert_eq!(replies, 1, "{blocks:#?}");
    assert!(
        !blocks.iter().any(|b| b["text"] == "MOCK-SLOW please"),
        "{blocks:#?}"
    );
}

/// The claude in Pane `holder`, as `ccrpc ls --json` lists it.
fn pane_claude(env: &Env, holder: &str) -> Option<u32> {
    let out = ccrpc_cmd(env)
        .args(["ls", "--json"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let rows: Value = serde_json::from_slice(&out.stdout).unwrap();
    rows.as_array()?.iter().find(|r| r["holder_id"] == holder)?["claude_pid"]
        .as_u64()
        .map(|p| p as u32)
}

/// One daemon per config dir, started by the first Pane that moved a conversation to the
/// background: its background claude processes carry that Pane's Holder. A second Pane's
/// conversation moved there is still followed.
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_follows_the_background_session_of_a_second_pane() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let mut daemons = Daemons { pane_claude: None };
    let mut first = Rpc::start(&env);
    let r1 = start_session(&mut first, &env);
    daemons.pane_claude = pane_claude(&env, r1["holder_id"].as_str().unwrap());
    first.call("p1", json!({"type": "prompt", "text": "hello one"}));
    first.event("turn_end", Duration::from_secs(30));
    first.call("k1", json!({"type": "send_keys", "keys": ["left"]}));
    first.event("session_changed", Duration::from_secs(20));

    let mut second = Rpc::start(&env);
    start_session(&mut second, &env);
    second.call("p2", json!({"type": "prompt", "text": "hello two"}));
    second.event("turn_end", Duration::from_secs(30));
    let old = second.call("g1", json!({"type": "get_state"}))["session_id"].clone();
    second.forget();
    second.call("k2", json!({"type": "send_keys", "keys": ["left"]}));
    let ch = second.until("session_changed", Duration::from_secs(20), |v| {
        v["type"] == "session_changed"
    });
    assert_eq!(ch["previous"], old, "{ch}");
    second.call("k3", json!({"type": "send_keys", "keys": ["esc"]}));
    wait_screen(&mut second, "w1", |l| {
        l.iter().any(|l| l.contains("← for agents"))
    });
    second.forget();
    let d = second.call("p3", json!({"type": "prompt", "text": "hello three"}));
    let t = second.event("turn_end", Duration::from_secs(30));
    assert_eq!(t["prompt_id"], d["prompt_id"], "{t}");
    let st = second.call("g2", json!({"type": "get_state"}));
    assert_eq!(st["session_id"], ch["session_id"], "{st}");
}

/// `new --fork`: a Fork of a running Session from before one of its prompts (conversation only),
/// from before the prompt that wrote a file (code too: the warning waits for the caller, cancel
/// keeps the code), of the whole Session, and a Fork that fails (nothing left, exit code 1).
#[test]
#[ignore = "real Claude Code against a local mock model; needs toh and claude"]
fn mock_model_fork() {
    let env = setup_with("mock-key", &mock_model(), "claude-sonnet-4-5");
    let mut rpc = Rpc::start(&env);
    start_session(&mut rpc, &env);
    let w = env.work.join("w.txt");
    let write = format!("MOCK-WRITE {} now", w.display());
    run_turn_allowing(&mut rpc, "p1", "first plain");
    run_turn_allowing(&mut rpc, "p2", &write);
    run_turn_allowing(&mut rpc, "p3", "third one");
    settle_idle(&mut rpc);
    let origin = rpc.call("g0", json!({"type": "get_state"}))["session_id"]
        .as_str()
        .unwrap()
        .to_string();
    let msgs = rpc.call("m1", json!({"type": "get_messages"}));
    let prompts: Vec<String> = msgs["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["kind"] == "user")
        .map(|b| b["uuid"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(prompts.len(), 3, "{prompts:?}");
    let transcript_of = |sid: &str| -> std::path::PathBuf {
        for d in std::fs::read_dir(env.cc_dir.join("projects"))
            .unwrap()
            .flatten()
        {
            let p = d.path().join(format!("{sid}.jsonl"));
            if p.exists() {
                return p;
            }
        }
        panic!("没有 {sid} 的 transcript");
    };
    let origin_len = std::fs::metadata(transcript_of(&origin)).unwrap().len();
    // Waits for the Fork, answering startup dialogs as `start_session` does.
    let fork_of = |opts: &[&str]| -> (Rpc, Value) {
        let log = std::env::var("CCRPC_E2E_FORK_LOG").unwrap_or_default();
        let mut all = vec!["--fork", origin.as_str()];
        if !log.is_empty() {
            all.extend(["--log", log.as_str()]);
        }
        all.extend(opts);
        let mut f = Rpc::start_in(&env, &env.work, &all);
        let ready = f.event("ready", Duration::from_secs(20));
        f.shell_pid = shell_pid(&env, ready["holder_id"].as_str().unwrap());
        let done = f.until("fork", Duration::from_secs(90), |v| {
            if v["type"] == "dialog" {
                panic!("Fork 启动时出现了认不出的界面：{v}");
            }
            v["type"] == "fork" || v["type"] == "rewind"
        });
        (f, done)
    };

    // 1. Before the third prompt, the conversation only.
    let (mut f1, done) = fork_of(&["--fork-at", &prompts[2], "--fork-title", "origin (Fork)"]);
    assert!(done["error"].is_null(), "{done}");
    let sid = done["session_id"].as_str().unwrap().to_string();
    assert_ne!(sid, origin);
    assert_eq!(done["origin"], origin.as_str(), "{done}");
    assert_eq!(done["text"], "third one", "{done}");
    assert_eq!(done["title"], "origin (Fork)", "{done}");
    let ch = f1
        .seen
        .iter()
        .find(|v| v["type"] == "session_changed")
        .cloned();
    assert_eq!(ch.unwrap()["session_id"], sid.as_str());
    // The copied history comes as no blocks.
    assert!(
        !f1.seen
            .iter()
            .any(|v| v["type"] == "block" && v["kind"] == "user"),
        "{:?}",
        f1.seen
    );
    let fork_tx = std::fs::read_to_string(transcript_of(&sid)).unwrap();
    let uuids = |tx: &str| -> Vec<String> {
        tx.lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter_map(|v| v["uuid"].as_str().map(str::to_string))
            .collect()
    };
    let entries = uuids(&fork_tx);
    assert!(entries.contains(&prompts[0]) && entries.contains(&prompts[1]));
    assert!(
        !entries.contains(&prompts[2]),
        "回退掉的提示词不该在 Fork 里"
    );
    assert!(fork_tx.contains("\"customTitle\":\"origin (Fork)\""));
    // The Origin Session is untouched; the Fork goes on as any Session.
    assert_eq!(
        std::fs::metadata(transcript_of(&origin)).unwrap().len(),
        origin_len
    );
    f1.forget();
    f1.call("f1p", json!({"type": "prompt", "text": "after fork"}));
    let t = f1.event("turn_end", Duration::from_secs(30));
    assert_eq!(t["last_text"], "ok", "{t}");
    drop(f1);

    // 2. Before the prompt that wrote w.txt, code too: the warning waits; cancel keeps the code.
    assert!(w.exists());
    let (mut f2, rw) = fork_of(&[
        "--fork-at",
        &prompts[1],
        "--fork-restore",
        "both",
        "--fork-title",
        "origin (Fork 2)",
    ]);
    assert_eq!(rw["type"], "rewind", "{rw}");
    assert!(rw["warning"].is_string(), "{rw}");
    let options: Vec<&str> = rw["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["option"].as_str().unwrap())
        .collect();
    assert_eq!(options, ["both", "conversation"], "{rw}");
    let st = f2.call("f2g", json!({"type": "get_state"}));
    assert_eq!(st["request"]["request_id"], rw["request_id"], "{st}");
    let a = f2.call(
        "f2a",
        json!({"type": "answer_rewind", "request_id": rw["request_id"], "cancel": true}),
    );
    assert_eq!(a["text"], write.as_str(), "{a}");
    let done = f2.event("fork", Duration::from_secs(30));
    assert!(done["error"].is_null(), "{done}");
    assert_eq!(done["text"], write.as_str(), "{done}");
    assert!(w.exists(), "取消时只恢复对话，代码不变");
    drop(f2);

    // 3. The whole Session.
    let (f3, done) = fork_of(&["--fork-title", "origin (Fork 3)"]);
    assert!(done["error"].is_null(), "{done}");
    assert!(done["text"].is_null(), "{done}");
    let tx = std::fs::read_to_string(transcript_of(done["session_id"].as_str().unwrap())).unwrap();
    let entries = uuids(&tx);
    assert!(
        prompts.iter().all(|p| entries.contains(p)),
        "整段 Fork 应含全部提示词"
    );
    drop(f3);

    // 3b. A prompt sent right after `ready` waits for the Fork, then goes into the new Session.
    let log = std::env::var("CCRPC_E2E_FORK_LOG").unwrap_or_default();
    let mut opts = vec!["--fork", origin.as_str(), "--fork-at", &prompts[2]];
    if !log.is_empty() {
        opts.extend(["--log", log.as_str()]);
    }
    let mut f5 = Rpc::start_in(&env, &env.work, &opts);
    let ready = f5.event("ready", Duration::from_secs(20));
    f5.shell_pid = shell_pid(&env, ready["holder_id"].as_str().unwrap());
    f5.send(json!({"id": "early", "type": "prompt", "text": "early prompt"}));
    let done = f5.event("fork", Duration::from_secs(90));
    assert!(done["error"].is_null(), "{done}");
    let r = f5.until("early", Duration::from_secs(60), |v| v["id"] == "early");
    assert_eq!(r["success"], true, "{r}");
    let tx = std::fs::read_to_string(transcript_of(done["session_id"].as_str().unwrap())).unwrap();
    assert!(tx.contains("early prompt"));
    drop(f5);

    // 3c. The warning's confirm page left with Esc in the TUI: no rewind, the Fork is cancelled.
    let (mut f6, rw) = fork_of(&["--fork-at", &prompts[1], "--fork-restore", "both"]);
    assert_eq!(rw["type"], "rewind", "{rw}");
    f6.call("esc", json!({"type": "send_keys", "keys": ["esc", "esc"]}));
    let done = f6.event("fork", Duration::from_secs(30));
    assert_eq!(done["error"]["code"], "cancelled", "{done}");
    f6.stdin.take();
    assert_eq!(f6.child.wait().unwrap().code(), Some(1));

    // 4. A prompt that is not on the Main Line: the Fork fails and ccrpc ends with 1.
    let (mut f4, done) = fork_of(&["--fork-at", "00000000-0000-0000-0000-000000000000"]);
    assert_eq!(done["error"]["code"], "not_found", "{done}");
    f4.stdin.take();
    let status = f4.child.wait().unwrap();
    assert_eq!(status.code(), Some(1));

    // 5. No such Session: refused before any Pane is opened.
    let out = ccrpc_cmd(&env)
        .args(["new", "--cwd"])
        .arg(&env.work)
        .args([
            "--fork",
            "11111111-2222-3333-4444-555555555555",
            "--",
            "--plugin-dir",
        ])
        .arg(&env.plugin_dir)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("找不到 Session"));
}
