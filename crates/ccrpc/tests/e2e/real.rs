//! Against a real model (`CCRPC_E2E_API_KEY`; spends credits).

use std::io::Write;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::harness::*;

#[test]
#[ignore = "real Claude Code; needs CCRPC_E2E_API_KEY, toh and claude"]
fn controls_a_real_session() {
    let Ok(key) = std::env::var("CCRPC_E2E_API_KEY") else {
        eprintln!("跳过：没有 CCRPC_E2E_API_KEY");
        return;
    };
    let env = setup(&key);
    if let Some(t) = toh_bin() {
        eprintln!("toh: {}", t.display());
    }
    let mut rpc = Rpc::start(&env);
    let ready = start_session(&mut rpc, &env);
    assert_eq!(ready["size"], json!([160, 48]));
    let scr = rpc.call("g0", json!({"type": "get_screen"}));
    assert_eq!(scr["lines"].as_array().unwrap().len(), 48);

    // prompt → reply → turn_end
    let d = rpc.call(
        "p1",
        json!({"type": "prompt", "text": "Reply with exactly the word PONG and nothing else."}),
    );
    assert!(d["uuid"].is_string());
    assert!(d["prompt_id"].is_string(), "{d}");
    let te = rpc.event("turn_end", Duration::from_secs(60));
    // One prompt, one turn: the plugin's prompt id.
    assert_eq!(te["prompt_id"], d["prompt_id"], "{te}");
    assert_eq!(te["trigger"], "prompt");
    assert_eq!(te["background_tasks"], json!([]));
    assert!(
        te["last_text"].as_str().unwrap_or("").contains("PONG"),
        "{te}"
    );
    // What the turn did.
    assert!(te["error"].is_null(), "{te}");
    assert_eq!(te["aborted"], false);
    assert!(te["duration_ms"].as_u64().unwrap_or(0) > 0, "{te}");
    assert!(
        te["usage"]["output_tokens"].as_u64().unwrap_or(0) > 0,
        "{te}"
    );

    // Permission modes: to plan and back.
    let d = rpc.call("m1", json!({"type": "set_mode", "mode": "plan"}));
    assert_eq!(d["mode"], "plan");
    let d = rpc.call("m2", json!({"type": "set_mode", "mode": "manual"}));
    assert_eq!(d["mode"], "manual");

    // A prompt with an image, pasted by its path.
    let png = env.work.join("dot.png");
    std::fs::write(&png, red_dot_png()).unwrap();
    let d = rpc.call(
        "im1",
        json!({"type": "prompt", "text": "What color is this image? Reply with one word and use no tools.",
            "images": [png.display().to_string()]}),
    );
    assert!(d["uuid"].is_string(), "{d}");
    loop {
        let v = rpc.until("image turn", Duration::from_secs(90), |v| {
            v["type"] == "turn_end" || v["type"] == "permission_request"
        });
        if v["type"] == "turn_end" {
            break;
        }
        rpc.call(
            "im2",
            json!({"type": "answer_permission", "request_id": v["request_id"], "decision": "deny"}),
        );
    }

    // permission: allow
    rpc.call("p2", json!({"type": "prompt", "text": "Run exactly this shell command and nothing else: touch created.txt"}));
    let pr = rpc.event("permission_request", Duration::from_secs(60));
    assert_eq!(pr["tool_name"], "Bash");
    // Answered after a while: the closing dialog is not reported as another one.
    std::thread::sleep(Duration::from_millis(1500));
    let d = rpc.call(
        "p3",
        json!({"type": "answer_permission", "request_id": pr["request_id"], "decision": "allow"}),
    );
    assert_ne!(d["state"], "waiting", "{d}");
    let v = rpc.until("turn_end", Duration::from_secs(60), |v| {
        v["type"] == "turn_end" || v["type"] == "dialog"
    });
    assert_eq!(v["type"], "turn_end", "作答后误报 dialog：{v}");
    assert!(env.work.join("created.txt").exists());

    // permission: always allow, as the dialog's own option does (a prefix rule in the project's
    // local settings); the same command then runs without asking.
    rpc.forget();
    let cmd_text = "Run exactly this shell command and nothing else: python3 -c 'print(7)'";
    rpc.call("aa1", json!({"type": "prompt", "text": cmd_text}));
    let pr = rpc.event("permission_request", Duration::from_secs(60));
    rpc.call(
        "aa2",
        json!({"type": "answer_permission", "request_id": pr["request_id"], "decision": "allow_always"}),
    );
    rpc.event("turn_end", Duration::from_secs(60));
    let local =
        std::fs::read_to_string(env.work.join(".claude/settings.local.json")).unwrap_or_default();
    assert!(local.contains("Bash(python3"), "{local}");
    rpc.forget();
    rpc.call("aa3", json!({"type": "prompt", "text": cmd_text}));
    let v = rpc.until("no second approval", Duration::from_secs(60), |v| {
        v["type"] == "turn_end" || v["type"] == "permission_request"
    });
    assert_eq!(v["type"], "turn_end", "总是允许之后又要审批：{v}");

    // permission: deny with a message
    rpc.forget();
    rpc.call("p4", json!({"type": "prompt", "text": "Run exactly this shell command and nothing else: rm a.txt"}));
    let pr = rpc.event("permission_request", Duration::from_secs(60));
    let d = rpc.call(
        "p5",
        json!({"type": "answer_permission", "request_id": pr["request_id"], "decision": "deny",
        "message": "Do not delete anything. Reply with exactly KEPT."}),
    );
    // Through the plugin the message is the model's feedback; the turn goes on.
    assert!(d.get("message_uuid").is_none(), "{d}");
    rpc.until("KEPT", Duration::from_secs(60), |v| {
        v["type"] == "turn_end" && v["last_text"].as_str().unwrap_or("").contains("KEPT")
    });
    assert!(env.work.join("a.txt").exists());

    // AskUserQuestion: two tabbed questions, single + multi
    rpc.forget();
    rpc.call("q1", json!({"type": "prompt", "text": "Call the AskUserQuestion tool once with exactly two questions: (1) single-choice question 'Indent style?' with options Tabs and Spaces; (2) multi-select question 'Which linters?' with options eslint, prettier, biome. After I answer, reply with my answers."}));
    let q = rpc.event("question", Duration::from_secs(60));
    assert_eq!(q["questions"].as_array().unwrap().len(), 2);
    rpc.call(
        "q2",
        json!({"type": "answer_question", "request_id": q["request_id"],
        "answers": [{"options": ["Spaces"]}, {"options": ["prettier", "biome"]}]}),
    );
    let tr = rpc.until("question result", Duration::from_secs(30), |v| {
        v["type"] == "block" && v["kind"] == "tool_result" && v["tool_use_id"] == q["request_id"]
    });
    let c = tr["content"].as_str().unwrap();
    assert!(
        c.contains("\"Indent style?\"=\"Spaces\"") && c.contains("prettier, biome"),
        "{c}"
    );
    rpc.event("turn_end", Duration::from_secs(60));

    // AskUserQuestion: single question, free text
    rpc.forget();
    rpc.call("q3", json!({"type": "prompt", "text": "Call the AskUserQuestion tool once with a single question 'Favorite color?' with options Red and Blue. Then reply with my answer."}));
    let q = rpc.event("question", Duration::from_secs(60));
    rpc.call("q4", json!({"type": "answer_question", "request_id": q["request_id"], "answers": [{"other": "teal green"}]}));
    let tr = rpc.until("other result", Duration::from_secs(30), |v| {
        v["type"] == "block" && v["kind"] == "tool_result" && v["tool_use_id"] == q["request_id"]
    });
    assert!(tr["content"].as_str().unwrap().contains("teal green"));
    rpc.event("turn_end", Duration::from_secs(60));

    // steer mid-turn + follow_up after the turn
    rpc.forget();
    rpc.call("s1", json!({"type": "prompt", "text": "Run this exact shell command in the foreground (do not set run_in_background; it is safe): python3 -c \"import time; time.sleep(8); print('first-done')\" Then reply DONE."}));
    if let Some(pr) = wait_permission_or_tool(&mut rpc) {
        rpc.call(
            "s1a",
            json!({"type": "answer_permission", "request_id": pr, "decision": "allow"}),
        );
    }
    rpc.call(
        "s2",
        json!({"type": "follow_up", "text": "Reply with exactly FOLLOWUP-OK."}),
    );
    // A second follow-up, dropped again.
    rpc.call(
        "s2b",
        json!({"type": "follow_up", "text": "Reply with exactly SHOULD-NOT-RUN."}),
    );
    let d = rpc.call("s2c", json!({"type": "cancel_follow_up", "index": 1}));
    assert_eq!(d["removed"], 1);
    assert_eq!(d["follow_ups"], json!(["Reply with exactly FOLLOWUP-OK."]));
    // The command is running now (its tool_use was seen, any permission answered).
    std::thread::sleep(Duration::from_secs(1));
    let d = rpc.call(
        "s3",
        json!({"type": "steer", "text": "Also include the word STEERED in your final reply."}),
    );
    assert_eq!(d["delivered_as"], "queued");
    rpc.until("queued block", Duration::from_secs(30), |v| {
        v["type"] == "block" && v["queued"] == true
    });
    let te = rpc.event("turn_end", Duration::from_secs(60));
    assert!(
        te["last_text"].as_str().unwrap_or("").contains("STEERED"),
        "{te}"
    );
    rpc.until("follow_up turn", Duration::from_secs(60), |v| {
        v["type"] == "turn_end"
            && v["last_text"]
                .as_str()
                .unwrap_or("")
                .contains("FOLLOWUP-OK")
    });

    // abort
    rpc.forget();
    rpc.call("a1", json!({"type": "prompt", "text": "Run this exact shell command in the foreground (do not set run_in_background; it is safe): python3 -c \"import time; time.sleep(30)\" Then reply DONE."}));
    if let Some(pr) = wait_permission_or_tool(&mut rpc) {
        rpc.call(
            "a1a",
            json!({"type": "answer_permission", "request_id": pr, "decision": "allow"}),
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    let d = rpc.call("a2", json!({"type": "abort"}));
    assert_eq!(d["state"], "idle");

    // a prompt right after abort (the aborted prompt is restored into the input box first)
    rpc.forget();
    rpc.call(
        "a3",
        json!({"type": "prompt", "text": "Reply with exactly AFTER-ABORT."}),
    );
    rpc.until("after abort", Duration::from_secs(60), |v| {
        v["type"] == "turn_end"
            && v["last_text"]
                .as_str()
                .unwrap_or("")
                .contains("AFTER-ABORT")
    });

    // Subagent Run: its blocks carry agent_id and the Agent call's id
    let seen = run_turn_allowing(
        &mut rpc,
        "sa1",
        "Use the Agent tool exactly once (subagent_type general-purpose) with this task: run the shell command `touch subagent.txt && echo SUBAGENT-OK` with Bash and report its output. Then reply with exactly the subagent's report.",
    );
    let mut sub_requests: Vec<Value> = seen
        .iter()
        .filter(|v| v["type"] == "permission_request" && v["agent_id"].is_string())
        .cloned()
        .collect();
    let agent_call = seen
        .iter()
        .find(|v| v["kind"] == "tool_use" && (v["name"] == "Agent" || v["name"] == "Task"))
        .expect("没有 Agent 调用");
    assert!(
        seen.iter().any(|v| v["type"] == "block"
            && v["agent_id"].is_string()
            && v["parent_tool_use_id"] == agent_call["id"]),
        "没有 Subagent Run 的 block"
    );
    // A background Subagent Run ends its turn at once; its result then wakes the main line for
    // another turn. Wait until the report is in.
    if !seen.iter().any(|v| {
        v["type"] == "turn_end"
            && v["last_text"]
                .as_str()
                .unwrap_or("")
                .contains("SUBAGENT-OK")
    }) {
        // The background Subagent's own approval comes after that first turn: answered on the
        // screen (its hook does not wait), with agent_id.
        let (te, answered) = until_answering(&mut rpc, "subagent report", |v| {
            v["type"] == "turn_end"
                && v["last_text"]
                    .as_str()
                    .unwrap_or("")
                    .contains("SUBAGENT-OK")
        });
        assert_eq!(te["trigger"], "task_notification", "{te}");
        sub_requests.extend(answered.into_iter().filter(|v| v["agent_id"].is_string()));
    }
    assert!(
        !sub_requests.is_empty(),
        "Subagent 的 touch 没有经 permission_request（带 agent_id）作答"
    );
    assert!(env.work.join("subagent.txt").exists());
    // The history has the Subagent Run's blocks under the Agent call.
    let msgs = rpc.call(
        "sa2",
        json!({"type": "get_messages", "subagents": true, "limit": 30}),
    );
    assert!(
        msgs["subagent_blocks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["agent_id"].is_string() && b["parent_tool_use_id"] == agent_call["id"]),
        "get_messages 没有 Subagent Run 的块：{}",
        msgs["subagent_blocks"]
    );

    // Workflow Run (when the model has the Workflow tool): workflow snapshots and run_id blocks
    let seen = run_turn_allowing(
        &mut rpc,
        "wf1",
        "Call the Workflow tool exactly once with this script and nothing else, then reply DONE:\nexport const meta = { name: 'e2e-echo', description: 'echo', phases: [{ title: 'Echo' }] }\nconst r = await agent('Reply with exactly WF-OK', { label: 'echo', phase: 'Echo' })\nreturn r",
    );
    if seen
        .iter()
        .any(|v| v["kind"] == "tool_use" && v["name"] == "Workflow")
    {
        let done = |v: &Value| v["type"] == "workflow" && v["status"] == "completed";
        let in_turn = seen.iter().find(|v| done(v)).cloned();
        let wf = match &in_turn {
            Some(v) => v.clone(),
            None => rpc.until("workflow", Duration::from_secs(180), done),
        };
        assert_eq!(wf["name"], "e2e-echo");
        assert!(wf["tool_use_id"].is_string(), "{wf}");
        assert!(
            seen.iter()
                .any(|v| v["type"] == "block" && v["run_id"].is_string())
                || rpc
                    .seen
                    .iter()
                    .any(|v| v["type"] == "block" && v["run_id"].is_string()),
            "没有 Workflow Run 里 Subagent 的 block"
        );
        let _ = in_turn;
        // The run's completion notice may wake the main line once more: wait until it settles.
        settle_idle(&mut rpc);
    } else {
        eprintln!("模型没有调用 Workflow 工具，跳过 Workflow 断言");
    }

    // Background shell: listed by get_tasks, stopped by stop_task.
    run_turn_allowing(
        &mut rpc,
        "bg1",
        "Use the Bash tool exactly once with run_in_background set to true to run: sleep 300 (description: long sleep). Then reply STARTED.",
    );
    let tasks = rpc.call("bg2", json!({"type": "get_tasks"}));
    let shell = tasks["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["kind"] == "shell" && t["status"] == "running")
        .unwrap_or_else(|| panic!("没有运行中的后台 shell：{tasks}"))
        .clone();
    let d = rpc.call("bg3", json!({"type": "stop_task", "task_id": shell["id"]}));
    assert_ne!(d["status"], "running", "{d}");
    settle_idle(&mut rpc);

    // get_state / get_messages / get_screen
    let st = rpc.call("g1", json!({"type": "get_state"}));
    assert_eq!(st["state"], "idle");
    let msgs = rpc.call("g2", json!({"type": "get_messages"}));
    assert!(msgs["blocks"].as_array().unwrap().len() > 10);
    let scr = rpc.call("g3", json!({"type": "get_screen"}));
    assert!(
        scr["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l.as_str().unwrap_or("").starts_with('❯'))
    );

    // Nothing so far forked the conversation: no `branch` may have been reported.
    assert!(rpc.branches.is_empty(), "误报 branch：{:?}", rpc.branches);

    // Rewind: Esc Esc on an empty input opens the picker (newest message last, `❯ (current)`);
    // pick the newest prompt, restore the conversation only, then ask something else there.
    rpc.forget();
    rpc.call("r1", json!({"type": "send_keys", "keys": ["esc", "esc"]}));
    let picker = wait_screen(&mut rpc, "r2", |ls| {
        ls.iter()
            .any(|l| l.contains("Restore the code and/or conversation"))
    });
    assert!(
        picker.iter().any(|l| l.trim() == "❯ (current)"),
        "{picker:#?}"
    );
    rpc.call("r3", json!({"type": "send_keys", "keys": ["up", "enter"]}));
    let confirm = wait_screen(&mut rpc, "r4", |ls| {
        ls.iter().any(|l| l.contains("Restore conversation"))
    });
    let n = confirm
        .iter()
        .find_map(|l| {
            let t = l.trim_start().trim_start_matches('❯').trim_start();
            let (num, label) = t.split_once(". ")?;
            (label.trim() == "Restore conversation").then(|| num.to_string())
        })
        .unwrap_or_else(|| panic!("找不到 Restore conversation：{confirm:#?}"));
    rpc.call("r5", json!({"type": "send_keys", "keys": [{"text": n}]}));
    std::thread::sleep(Duration::from_millis(800));
    rpc.call(
        "r6",
        json!({"type": "prompt", "text": "Reply with exactly REWOUND."}),
    );
    let b = rpc.event("branch", Duration::from_secs(30));
    assert!(b["at_uuid"].is_string(), "{b}");
    rpc.until("rewound", Duration::from_secs(60), |v| {
        v["type"] == "turn_end" && v["last_text"].as_str().unwrap_or("").contains("REWOUND")
    });
    let msgs = rpc.call("r7", json!({"type": "get_messages"}));
    let texts: Vec<&str> = msgs["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|b| b["text"].as_str())
        .collect();
    assert!(texts.iter().any(|t| t.contains("REWOUND")));
    // `up` picked the newest prompt before the rewind: the background shell one.
    assert!(
        !texts
            .iter()
            .any(|t| t.starts_with("Use the Bash tool exactly once with run_in_background")),
        "{texts:#?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("Call the Workflow tool")),
        "{texts:#?}"
    );

    // /clear changes the session id
    rpc.forget();
    let before = st["session_id"].clone();
    rpc.call("c1", json!({"type": "clear"}));
    let ch = rpc.event("session_changed", Duration::from_secs(15));
    assert_eq!(ch["previous"], before);
    rpc.call(
        "c2",
        json!({"type": "prompt", "text": "Reply with exactly AFTER-CLEAR."}),
    );
    rpc.until("after clear", Duration::from_secs(60), |v| {
        v["type"] == "turn_end"
            && v["last_text"]
                .as_str()
                .unwrap_or("")
                .contains("AFTER-CLEAR")
    });

    // bad requests
    rpc.send(json!({"id": "e1", "type": "nope"}));
    let r = rpc.until("e1", Duration::from_secs(5), |v| v["id"] == "e1");
    assert_eq!(r["error"]["code"], "unknown_command");
    rpc.send(json!({"id": "e2", "type": "steer", "text": "x"}));
    let r = rpc.until("e2", Duration::from_secs(5), |v| v["id"] == "e2");
    assert_eq!(r["error"]["code"], "not_busy");

    // exit Claude Code: session_exit
    rpc.call("x1", json!({"type": "slash", "command": "exit"}));
    // A shell the model left running in the background makes Claude Code ask first (a `dialog`).
    let v = rpc.until("exit", Duration::from_secs(20), |v| {
        v["type"] == "session_exit" || v["type"] == "dialog"
    });
    if v["type"] == "dialog" {
        let lines: Vec<&str> = v["lines"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(
            lines.iter().any(|l| l.contains("Exit and stop tasks")),
            "未预期的对话框：{lines:#?}"
        );
        rpc.call("x2", json!({"type": "send_keys", "keys": ["enter"]}));
        rpc.event("session_exit", Duration::from_secs(20));
    }
}

/// Waits for `pred`, allowing every permission request on the way; returns the match and the
/// requests answered.
fn until_answering(
    rpc: &mut Rpc,
    what: &str,
    pred: impl Fn(&Value) -> bool,
) -> (Value, Vec<Value>) {
    let mut n = 0;
    let mut answered = Vec::new();
    loop {
        let v = rpc.until(what, Duration::from_secs(180), |v| {
            pred(v) || v["type"] == "permission_request"
        });
        if pred(&v) {
            return (v, answered);
        }
        answered.push(v.clone());
        n += 1;
        rpc.call(
            &format!("ua-{n}"),
            json!({"type": "answer_permission", "request_id": v["request_id"], "decision": "allow"}),
        );
    }
}

/// After a prompt that runs a shell command: returns the request id if a permission prompt comes
/// first, `None` once the command is running without one.
fn wait_permission_or_tool(rpc: &mut Rpc) -> Option<Value> {
    let v = rpc.until("permission or tool_use", Duration::from_secs(60), |v| {
        v["type"] == "permission_request" || (v["type"] == "block" && v["kind"] == "tool_use")
    });
    if v["type"] == "permission_request" {
        return Some(v["request_id"].clone());
    }
    // tool_use first: a permission request may still follow within a moment
    let deadline = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < deadline {
        if let Some(v) = rpc.recv(Duration::from_millis(100)) {
            if v["type"] == "permission_request" {
                return Some(v["request_id"].clone());
            }
            rpc.seen.push(v);
        }
    }
    None
}

/// A raw ToH client standing in for a person: attaches and takes the PTY size.
fn person_takes_over(env: &Env, holder: &str) -> std::os::unix::net::UnixStream {
    use toh_proto::{AttachInfo, Frame};
    let sock = env.toh_dir.join("holders").join(format!("{holder}.sock"));
    let mut s = std::os::unix::net::UnixStream::connect(sock).unwrap();
    let attach = Frame::Attach(AttachInfo {
        client: "e2e-person".into(),
        cols: 100,
        rows: 30,
        ..Default::default()
    });
    s.write_all(&attach.encode()).unwrap();
    s.write_all(
        &Frame::Resize {
            cols: 100,
            rows: 30,
        }
        .encode(),
    )
    .unwrap();
    s
}

#[test]
#[ignore = "real Claude Code; needs CCRPC_E2E_API_KEY, toh and claude"]
fn yields_when_a_person_takes_over() {
    let Ok(key) = std::env::var("CCRPC_E2E_API_KEY") else {
        eprintln!("跳过：没有 CCRPC_E2E_API_KEY");
        return;
    };
    let env = setup(&key);
    // Without the plugin there is no Controlled Session.
    let out = ccrpc_cmd(&env)
        .args(["new", "--cwd"])
        .arg(&env.work)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("ccrpc 插件未启用"));
    let mut rpc = Rpc::start(&env);
    let ready = start_session(&mut rpc, &env);
    let holder = ready["holder_id"].as_str().unwrap().to_string();

    // Someone opens the Pane in ToH: ccrpc reports it and stops writing.
    let person = person_takes_over(&env, &holder);
    let t = rpc.event("taken_over", Duration::from_secs(10));
    assert_eq!(t["client"], "e2e-person");
    rpc.send(json!({"id": "t1", "type": "prompt", "text": "hello"}));
    let r = rpc.until("t1", Duration::from_secs(5), |v| v["id"] == "t1");
    assert_eq!(r["error"]["code"], "taken_over");
    let st = rpc.call("t2", json!({"type": "get_state"}));
    assert_eq!(st["taken_over"], true);
    // The VT follows the person's size.
    let scr = rpc.call("t3", json!({"type": "get_screen"}));
    assert_eq!(scr["lines"].as_array().unwrap().len(), 30);

    // Taking it back: refused while the person is there, fine once they left.
    rpc.send(json!({"id": "r1", "type": "reclaim"}));
    let r = rpc.until("r1", Duration::from_secs(5), |v| v["id"] == "r1");
    assert_eq!(r["error"]["code"], "taken_over", "{r}");
    drop(person);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut n = 0;
    loop {
        n += 1;
        rpc.send(json!({"id": format!("r2-{n}"), "type": "reclaim"}));
        let id = format!("r2-{n}");
        let r = rpc.until("r2", Duration::from_secs(10), |v| v["id"] == id.as_str());
        if r["success"] == true {
            break;
        }
        assert!(Instant::now() < deadline, "人离开后取不回：{r}");
        std::thread::sleep(Duration::from_millis(300));
    }
    rpc.event("reclaimed", Duration::from_secs(5));
    let st = rpc.call("r3", json!({"type": "get_state"}));
    assert_eq!(st["taken_over"], false);
    let scr = rpc.call("r4", json!({"type": "get_screen"}));
    assert_eq!(scr["lines"].as_array().unwrap().len(), 48);

    // One ccrpc per Session.
    let out = ccrpc_cmd(&env)
        .args(["attach", "--force", &holder])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("已有 ccrpc 连着"), "{err}");
    let st = rpc.call("r5", json!({"type": "get_state"}));
    assert_eq!(
        st["taken_over"], false,
        "the refused ccrpc did not touch the Pane"
    );

    // Another ccrpc refuses the Session while a person holds it, unless forced.
    // (The Pane outlives this ccrpc: the forced one cleans it up.)
    let holder_pid = rpc.holder_pid.take();
    drop(rpc);
    let person = person_takes_over(&env, &holder);
    let out = ccrpc_cmd(&env)
        .args(["attach", &holder])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("正被 e2e-person 使用"), "{err}");
    let mut forced = Rpc::attach(&env, &["--force", &holder]);
    forced.holder_pid = holder_pid;
    let ready = forced.event("ready", Duration::from_secs(20));
    assert_eq!(ready["holder_id"], holder.as_str());
    forced.call("f1", json!({"type": "get_state"}));

    // Co-display: shown alongside the person, no size of its own, no takeover, still writes.
    let holder_pid = forced.holder_pid.take();
    drop(forced);
    let mut co = Rpc::attach(&env, &["--co-display", &holder]);
    co.holder_pid = holder_pid;
    let ready = co.event("ready", Duration::from_secs(20));
    assert_eq!(ready["co_display"], true);
    assert_eq!(ready["taken_over"], false);
    assert_eq!(
        ready["size"],
        json!([160, 48]),
        "the PTY as the last one left it"
    );
    // The person resizes: the co-displayed VT follows, and it is no takeover.
    let mut person = person;
    person
        .write_all(&toh_proto::Frame::Resize { cols: 90, rows: 28 }.encode())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let scr = co.call("c0", json!({"type": "get_screen"}));
        if scr["lines"].as_array().unwrap().len() == 28 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "共显的屏幕没有跟随 PTY 尺寸：{}",
            scr["lines"].as_array().unwrap().len()
        );
        std::thread::sleep(Duration::from_millis(300));
    }
    co.call("c1", json!({"type": "send_keys", "keys": [{"text": "x"}]}));
    std::thread::sleep(Duration::from_millis(500));
    let st = co.call("c2", json!({"type": "get_state"}));
    assert_eq!(
        st["taken_over"], false,
        "a person's keys are not a takeover here"
    );
    co.call("c3", json!({"type": "send_keys", "keys": ["backspace"]}));
    drop(co);
    drop(person);
}

#[test]
#[ignore = "layout probe: prints screens of real Claude Code; needs CCRPC_E2E_API_KEY"]
fn probe_layouts() {
    let Ok(key) = std::env::var("CCRPC_E2E_API_KEY") else {
        eprintln!("跳过：没有 CCRPC_E2E_API_KEY");
        return;
    };
    let env = setup(&key);
    let mut rpc = Rpc::start(&env);
    start_session(&mut rpc, &env);
    dump(&mut rpc, "idle");

    // Permission modes: shift+tab cycles; print the footer each time.
    for i in 0..4 {
        rpc.call(
            &format!("m{i}"),
            json!({"type": "send_keys", "keys": ["shift+tab"]}),
        );
        std::thread::sleep(Duration::from_millis(600));
        dump(&mut rpc, &format!("mode{i}"));
    }

    // Image by path: a bracketed paste of a PNG file path.
    let png = env.work.join("dot.png");
    // 1×1 red PNG.
    let bytes = base64::engine::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==",
    )
    .unwrap();
    std::fs::write(&png, bytes).unwrap();
    let paste = format!("\x1b[200~{}\x1b[201~", png.display());
    rpc.call(
        "i1",
        json!({"type": "send_keys", "keys": [{"text": paste}]}),
    );
    std::thread::sleep(Duration::from_millis(1000));
    dump(&mut rpc, "image-pasted");
    rpc.call("i2", json!({"type": "send_keys", "keys": [{"text": " What color is this image? Reply with one word, without using tools."}, "enter"]}));
    let mut n = 0;
    loop {
        let v = rpc.until("image turn", Duration::from_secs(90), |v| {
            v["type"] == "turn_end" || v["type"] == "permission_request"
        });
        if v["type"] == "turn_end" {
            break;
        }
        n += 1;
        rpc.call(&format!("i3-{n}"), json!({"type": "answer_permission", "request_id": v["request_id"], "decision": "allow"}));
    }
    dump(&mut rpc, "image-answered");

    // Permission dialog and question in this TUI.
    rpc.call("p1", json!({"type": "prompt", "text": "Run exactly this shell command and nothing else: touch probe.txt"}));
    rpc.until("permission", Duration::from_secs(60), |v| {
        v["type"] == "state" && v["state"] == "waiting"
    });
    std::thread::sleep(Duration::from_millis(800));
    dump(&mut rpc, "permission");
    rpc.call("p2", json!({"type": "send_keys", "keys": ["esc"]}));
    wait_state(&mut rpc, "idle", 20);
    rpc.call("q1", json!({"type": "prompt", "text": "Call the AskUserQuestion tool once with a single question 'Pick one?' with options Alpha and Beta."}));
    rpc.until("question", Duration::from_secs(60), |v| {
        v["type"] == "state" && v["state"] == "waiting"
    });
    std::thread::sleep(Duration::from_millis(800));
    dump(&mut rpc, "question");
    rpc.call("q2", json!({"type": "send_keys", "keys": ["esc"]}));
    wait_state(&mut rpc, "idle", 20);

    // Background agent and its management view.
    rpc.call("b1", json!({"type": "prompt", "text": "Use the Agent tool once with run_in_background true (subagent_type general-purpose) with this task: run `sleep 25` with Bash, then reply DONE. Do not wait for it; reply STARTED right away."}));
    std::thread::sleep(Duration::from_secs(8));
    dump(&mut rpc, "background-running");
    rpc.call("b2", json!({"type": "send_keys", "keys": ["down"]}));
    std::thread::sleep(Duration::from_millis(800));
    dump(&mut rpc, "background-down");
    rpc.call("b3", json!({"type": "send_keys", "keys": ["enter"]}));
    std::thread::sleep(Duration::from_millis(800));
    dump(&mut rpc, "background-enter");
    rpc.call("b4", json!({"type": "send_keys", "keys": ["esc"]}));
    std::thread::sleep(Duration::from_millis(500));
    rpc.call(
        "b5",
        json!({"type": "send_keys", "keys": [{"text": "/tasks"}]}),
    );
    std::thread::sleep(Duration::from_millis(500));
    dump(&mut rpc, "tasks-typed");
    rpc.call("b6", json!({"type": "send_keys", "keys": ["enter"]}));
    std::thread::sleep(Duration::from_millis(1000));
    dump(&mut rpc, "tasks-open");
    rpc.call("b7", json!({"type": "send_keys", "keys": ["esc"]}));
    std::thread::sleep(Duration::from_secs(25));
    dump(&mut rpc, "background-done");

    // Plan mode: the plan approval dialog.
    rpc.forget();
    rpc.call(
        "pl1",
        json!({"type": "send_keys", "keys": ["esc", "ctrl+u"]}),
    );
    for _ in 0..2 {
        rpc.call("pl2", json!({"type": "send_keys", "keys": ["shift+tab"]}));
        std::thread::sleep(Duration::from_millis(400));
    }
    dump(&mut rpc, "plan-mode");
    rpc.call("pl3", json!({"type": "send_keys", "keys": [{"text": "Make a one-step plan to create a file named plan.txt, then present it for approval right away."}, "enter"]}));
    let _ = rpc.until("plan approval", Duration::from_secs(90), |v| {
        v["type"] == "state" && v["state"] == "waiting"
    });
    std::thread::sleep(Duration::from_millis(1000));
    dump(&mut rpc, "plan-approval");
    rpc.call("pl4", json!({"type": "send_keys", "keys": ["esc"]}));
    std::thread::sleep(Duration::from_millis(800));
    dump(&mut rpc, "end");
}

#[test]
#[ignore = "real Claude Code and the analyzer; needs CCRPC_E2E_API_KEY"]
fn learns_an_unknown_dialog() {
    let Ok(key) = std::env::var("CCRPC_E2E_API_KEY") else {
        eprintln!("跳过：没有 CCRPC_E2E_API_KEY");
        return;
    };
    let mut env = setup(&key);
    env.analyzer = true;
    let mut rpc = Rpc::start(&env);
    start_session(&mut rpc, &env);
    rpc.forget();
    // The plan approval is not a built-in layout: the analyzer learns it.
    rpc.call("m1", json!({"type": "set_mode", "mode": "plan"}));
    rpc.call(
        "p1",
        json!({"type": "prompt", "text": "Make a one-step plan to create an empty file named plan.txt, then present it for approval right away with ExitPlanMode."}),
    );
    let v = rpc.until("learned", Duration::from_secs(180), |v| {
        v["type"] == "choice" || v["type"] == "permission_request" || v["type"] == "dialog"
    });
    assert!(
        rpc.seen.iter().any(|e| e["type"] == "layout_learned"),
        "没有 layout_learned：{v}"
    );
    match v["type"].as_str() {
        Some("choice") => {
            let opts: Vec<&str> = v["options"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|o| o["label"].as_str())
                .collect();
            let pick = opts
                .iter()
                .find(|l| l.contains("manually approve"))
                .or_else(|| opts.first())
                .unwrap()
                .to_string();
            rpc.call(
                "c1",
                json!({"type": "answer_choice", "request_id": v["request_id"], "option": pick}),
            );
        }
        Some("permission_request") => {
            rpc.call("c1", json!({"type": "answer_permission", "request_id": v["request_id"], "decision": "allow"}));
        }
        _ => panic!("分析后仍报 dialog：{v}"),
    }
    // The plan goes ahead; allow what it asks for.
    loop {
        let v = rpc.until("plan turn", Duration::from_secs(120), |v| {
            v["type"] == "turn_end" || v["type"] == "permission_request"
        });
        if v["type"] == "turn_end" {
            break;
        }
        rpc.call("c2", json!({"type": "answer_permission", "request_id": v["request_id"], "decision": "allow"}));
    }
    // The layout is in the library; copy it out for export when asked.
    let learned: Vec<_> = std::fs::read_dir(&env.layout_dir)
        .unwrap()
        .flatten()
        .collect();
    assert!(!learned.is_empty());
    if let Ok(out) = std::env::var("CCRPC_E2E_KEEP_LAYOUTS") {
        std::fs::create_dir_all(&out).unwrap();
        for e in learned {
            std::fs::copy(e.path(), Path::new(&out).join(e.file_name())).unwrap();
        }
    }
}
