//! End to end against a real interactive Claude Code in an isolated ToH clay Pane.
//!
//! `CCRPC_E2E_API_KEY=… cargo test --test e2e -- --ignored --nocapture --test-threads=1`
//! Optional: `CCRPC_E2E_BASE_URL` (default DeepSeek's Anthropic endpoint), `CCRPC_E2E_MODEL`
//! (default `deepseek-flash`), `CCRPC_E2E_TOH` (the `toh` binary; default the sibling checkout's
//! `../toh/target/debug/toh` when built, else `toh` on PATH — its Holders run that build).
//! Spends model credits. Needs `claude` on PATH.
//!
//! Isolation: a fresh `TOH_STATE_DIR` under /tmp (unix socket paths must stay short) with clay
//! enabled, loopback-only and mDNS off; a fresh `CLAUDE_CONFIG_DIR` pre-seeded to skip onboarding
//! and folder trust, in manual permission mode. Cleanup kills only the recorded Holder pid.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const HOST_TERMINAL_VARS: &[&str] = &[
    "TOH_CONFIG",
    "TOH_HOLDER_ID",
    "KITTY_WINDOW_ID",
    "KITTY_LISTEN_ON",
    "KITTY_PID",
    "WEZTERM_PANE",
    "WEZTERM_UNIX_SOCKET",
    "KAKU_UNIX_SOCKET",
    "ITERM_SESSION_ID",
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
];

struct Env {
    _toh: tempfile::TempDir,
    _cc: tempfile::TempDir,
    _work: tempfile::TempDir,
    toh_dir: PathBuf,
    cc_dir: PathBuf,
    work: PathBuf,
}

fn setup(key: &str) -> Env {
    let toh = tempfile::Builder::new()
        .prefix("ccrpc-e2e-")
        .tempdir_in("/tmp")
        .unwrap();
    let cc = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let toh_dir = toh.path().canonicalize().unwrap();
    let cc_dir = cc.path().canonicalize().unwrap();
    let work_dir = work.path().canonicalize().unwrap();
    let base = std::env::var("CCRPC_E2E_BASE_URL")
        .unwrap_or_else(|_| "https://api.deepseek.com/anthropic".into());
    let model = std::env::var("CCRPC_E2E_MODEL").unwrap_or_else(|_| "deepseek-flash".into());
    let env_vars = [
        ("CLAUDE_CONFIG_DIR", cc_dir.display().to_string()),
        ("ANTHROPIC_BASE_URL", base),
        ("ANTHROPIC_AUTH_TOKEN", key.to_string()),
        ("ANTHROPIC_MODEL", model.clone()),
        ("ANTHROPIC_DEFAULT_HAIKU_MODEL", model.clone()),
        ("ANTHROPIC_DEFAULT_SONNET_MODEL", model.clone()),
        ("ANTHROPIC_DEFAULT_OPUS_MODEL", model.clone()),
        ("CLAUDE_CODE_SUBAGENT_MODEL", model),
        ("DISABLE_AUTOUPDATER", "1".into()),
        ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1".into()),
    ];
    let env_toml: Vec<String> = env_vars
        .iter()
        .map(|(k, v)| format!("{k} = {}", toml_str(v)))
        .collect();
    let config = format!(
        "language = \"zh-Hans\"\n[backends]\nenabled = [\"clay\"]\n[lan]\nbind = \"127.0.0.1\"\nmdns = false\n[backends.clay]\nenv = {{ {} }}\n",
        env_toml.join(", ")
    );
    // The config holds the model key: owner-only.
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(toh_dir.join("config.toml"))
            .unwrap();
        f.write_all(config.as_bytes()).unwrap();
    }
    let claude_json = json!({
        "hasCompletedOnboarding": true,
        "projects": { work_dir.display().to_string(): { "hasTrustDialogAccepted": true } },
    });
    std::fs::write(cc_dir.join(".claude.json"), claude_json.to_string()).unwrap();
    std::fs::write(
        cc_dir.join("settings.json"),
        json!({"permissions": {"defaultMode": "default"}}).to_string(),
    )
    .unwrap();
    std::fs::write(work_dir.join("a.txt"), "hello\n").unwrap();
    Env {
        _toh: toh,
        _cc: cc,
        _work: work,
        toh_dir,
        cc_dir,
        work: work_dir,
    }
}

fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

struct Rpc {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<Value>,
    t0: Instant,
    holder_pid: Option<u32>,
    seen: Vec<Value>,
    /// Every `branch` event received.
    branches: Vec<Value>,
}

/// The `toh` to run: `CCRPC_E2E_TOH`, else the sibling checkout's debug build, else PATH's.
fn toh_bin() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CCRPC_E2E_TOH") {
        return Some(PathBuf::from(p));
    }
    let sibling = Path::new(env!("CARGO_MANIFEST_DIR")).join("../toh/target/debug/toh");
    sibling.is_file().then(|| sibling.canonicalize().unwrap())
}

fn ccrpc_cmd(env: &Env) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ccrpc"));
    cmd.env("TOH_STATE_DIR", &env.toh_dir)
        .env("CLAUDE_CONFIG_DIR", &env.cc_dir);
    for k in HOST_TERMINAL_VARS {
        cmd.env_remove(k);
    }
    if let Some(toh) = toh_bin() {
        let dir = toh.parent().unwrap().to_path_buf();
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs = vec![dir];
        dirs.extend(std::env::split_paths(&path));
        cmd.env("PATH", std::env::join_paths(dirs).unwrap());
    }
    cmd
}

impl Rpc {
    fn start(env: &Env) -> Rpc {
        let mut cmd = ccrpc_cmd(env);
        cmd.args(["new", "--cwd"]).arg(&env.work);
        Rpc::spawn(cmd)
    }

    fn attach(env: &Env, args: &[&str]) -> Rpc {
        let mut cmd = ccrpc_cmd(env);
        cmd.arg("attach").args(args);
        Rpc::spawn(cmd)
    }

    fn spawn(mut cmd: Command) -> Rpc {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = cmd.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let v: Value = serde_json::from_str(&line)
                    .unwrap_or_else(|e| panic!("非 JSON 输出 {line:?}: {e}"));
                if tx.send(v).is_err() {
                    break;
                }
            }
        });
        let stdin = child.stdin.take();
        Rpc {
            child,
            stdin,
            rx,
            t0: Instant::now(),
            holder_pid: None,
            seen: Vec::new(),
            branches: Vec::new(),
        }
    }

    fn send(&mut self, c: Value) {
        eprintln!("{:6.1} >>> {c}", self.t0.elapsed().as_secs_f32());
        let w = self.stdin.as_mut().unwrap();
        writeln!(w, "{c}").unwrap();
        w.flush().unwrap();
    }

    /// Waits for a line matching `pred`, logging everything.
    fn until(
        &mut self,
        what: &str,
        timeout: Duration,
        mut pred: impl FnMut(&Value) -> bool,
    ) -> Value {
        // Lines already received count too (events may arrive before a response).
        if let Some(i) = self.seen.iter().position(&mut pred) {
            return self.seen.remove(i);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(v) => {
                    if v["type"] == "branch" {
                        self.branches.push(v.clone());
                    }
                    let s = v.to_string();
                    if v["type"] == "dialog" || v["type"] == "unrecognized" {
                        for l in v["lines"].as_array().into_iter().flatten() {
                            eprintln!("       | {}", l.as_str().unwrap_or(""));
                        }
                    }
                    eprintln!(
                        "{:6.1} {}",
                        self.t0.elapsed().as_secs_f32(),
                        &s[..s.floor_char_boundary(240)]
                    );
                    if pred(&v) {
                        return v;
                    }
                    self.seen.push(v);
                    if self.seen.len() > 400 {
                        self.seen.drain(..200);
                    }
                }
                Err(_) => panic!("等 {what} 超时"),
            }
        }
    }

    fn call(&mut self, id: &str, c: Value) -> Value {
        let mut c = c;
        c["id"] = json!(id);
        self.send(c);
        let r = self.until(&format!("响应 {id}"), Duration::from_secs(90), |v| {
            v["type"] == "response" && v["id"] == id
        });
        assert_eq!(r["success"], true, "命令 {id} 失败：{r}");
        r["data"].clone()
    }

    fn event(&mut self, ty: &str, timeout: Duration) -> Value {
        self.until(ty, timeout, |v| v["type"] == ty)
    }

    /// Drops everything received so far (so the next `until` only sees what comes after).
    fn forget(&mut self) {
        self.seen.clear();
        while self.rx.try_recv().is_ok() {}
    }
}

impl Drop for Rpc {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.wait();
        if let Some(pid) = self.holder_pid {
            // SAFETY: only the Holder pid recorded for this test's own Pane.
            unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        }
    }
}

fn holder_pid(toh_dir: &Path, holder: &str) -> Option<u32> {
    let bytes = std::fs::read(toh_dir.join("holders").join(format!("{holder}.json"))).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v["holder_pid"].as_u64().map(|p| p as u32)
}

/// Waits for `ready` and a Session that stays idle (answering the startup dialogs); returns `ready`.
fn start_session(rpc: &mut Rpc, env: &Env) -> Value {
    let ready = rpc.event("ready", Duration::from_secs(20));
    assert_eq!(ready["protocol"], 1);
    let holder = ready["holder_id"].as_str().unwrap().to_string();
    rpc.holder_pid = holder_pid(&env.toh_dir, &holder);
    assert!(rpc.holder_pid.is_some());
    // Startup: Claude Code may offer to make auto mode the default (a `dialog`, possibly right
    // after a first `idle`); keep manual mode through the generic dialog path (send_keys). The
    // Session is ready once it has been idle for 3 s with no dialog.
    let mut idle_since: Option<Instant> = None;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        assert!(Instant::now() < deadline, "启动后没有稳定在 idle");
        if idle_since.is_some_and(|t| t.elapsed() > Duration::from_secs(3)) {
            break;
        }
        let Ok(v) = rpc.rx.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        eprintln!("{:6.1} {v}", rpc.t0.elapsed().as_secs_f32());
        if v["type"] == "state" {
            idle_since = (v["state"] == "idle").then(Instant::now);
        } else if v["type"] == "dialog" {
            let lines: Vec<&str> = v["lines"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .collect();
            assert!(
                lines.iter().any(|l| l.contains("No, keep manual mode")),
                "未预期的对话框：{lines:#?}"
            );
            rpc.call(
                "d1",
                json!({"type": "send_keys", "keys": ["down", "enter"]}),
            );
        }
    }
    ready
}

/// Runs a turn to its end, allowing every permission request on the way; returns all lines seen.
fn run_turn_allowing(rpc: &mut Rpc, id: &str, text: &str) -> Vec<Value> {
    rpc.forget();
    rpc.call(id, json!({"type": "prompt", "text": text}));
    let mut seen = Vec::new();
    let mut n = 0;
    loop {
        let v = rpc.until("turn", Duration::from_secs(180), |v| {
            v["type"] == "turn_end" || v["type"] == "permission_request" || v["type"] == "block"
                || v["type"] == "workflow"
        });
        if v["type"] == "permission_request" {
            n += 1;
            rpc.call(
                &format!("{id}-ok{n}"),
                json!({"type": "answer_permission", "request_id": v["request_id"], "decision": "allow"}),
            );
        }
        let end = v["type"] == "turn_end";
        seen.push(v);
        if end {
            return seen;
        }
    }
}

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
    let te = rpc.event("turn_end", Duration::from_secs(60));
    assert!(
        te["last_text"].as_str().unwrap_or("").contains("PONG"),
        "{te}"
    );

    // permission: allow
    rpc.call("p2", json!({"type": "prompt", "text": "Run exactly this shell command and nothing else: touch created.txt"}));
    let pr = rpc.event("permission_request", Duration::from_secs(60));
    assert_eq!(pr["tool_name"], "Bash");
    rpc.call(
        "p3",
        json!({"type": "answer_permission", "request_id": pr["request_id"], "decision": "allow"}),
    );
    rpc.event("turn_end", Duration::from_secs(60));
    assert!(env.work.join("created.txt").exists());

    // permission: deny with a message
    rpc.forget();
    rpc.call("p4", json!({"type": "prompt", "text": "Run exactly this shell command and nothing else: rm a.txt"}));
    let pr = rpc.event("permission_request", Duration::from_secs(60));
    let d = rpc.call(
        "p5",
        json!({"type": "answer_permission", "request_id": pr["request_id"], "decision": "deny",
        "message": "Do not delete anything. Reply with exactly KEPT."}),
    );
    assert!(d["message_uuid"].is_string());
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
        "Use the Agent tool exactly once (subagent_type general-purpose) with this task: run the shell command `echo SUBAGENT-OK` with Bash and report its output. Then reply with exactly the subagent's report.",
    );
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
        v["type"] == "turn_end" && v["last_text"].as_str().unwrap_or("").contains("SUBAGENT-OK")
    }) {
        rpc.until("subagent report", Duration::from_secs(120), |v| {
            v["type"] == "turn_end"
                && v["last_text"].as_str().unwrap_or("").contains("SUBAGENT-OK")
        });
    }

    // Workflow Run (when the model has the Workflow tool): workflow snapshots and run_id blocks
    let seen = run_turn_allowing(
        &mut rpc,
        "wf1",
        "Call the Workflow tool exactly once with this script and nothing else, then reply DONE:\nexport const meta = { name: 'e2e-echo', description: 'echo', phases: [{ title: 'Echo' }] }\nconst r = await agent('Reply with exactly WF-OK', { label: 'echo', phase: 'Echo' })\nreturn r",
    );
    if seen.iter().any(|v| v["kind"] == "tool_use" && v["name"] == "Workflow") {
        let done = |v: &Value| v["type"] == "workflow" && v["status"] == "completed";
        let in_turn = seen.iter().find(|v| done(v)).cloned();
        let wf = match &in_turn {
            Some(v) => v.clone(),
            None => rpc.until("workflow", Duration::from_secs(180), done),
        };
        assert_eq!(wf["name"], "e2e-echo");
        assert!(wf["tool_use_id"].is_string(), "{wf}");
        assert!(
            seen.iter().any(|v| v["type"] == "block" && v["run_id"].is_string())
                || rpc.seen.iter().any(|v| v["type"] == "block" && v["run_id"].is_string()),
            "没有 Workflow Run 里 Subagent 的 block"
        );
        let _ = in_turn;
        // The run's completion notice may wake the main line once more: wait until it settles.
        settle_idle(&mut rpc);
    } else {
        eprintln!("模型没有调用 Workflow 工具，跳过 Workflow 断言");
    }

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
    // pick the Workflow prompt, restore the conversation only, then ask something else there.
    rpc.forget();
    rpc.call("r1", json!({"type": "send_keys", "keys": ["esc", "esc"]}));
    let picker = wait_screen(&mut rpc, "r2", |ls| ls.iter().any(|l| l.contains("Restore the code and/or conversation")));
    assert!(picker.iter().any(|l| l.trim() == "❯ (current)"), "{picker:#?}");
    rpc.call("r3", json!({"type": "send_keys", "keys": ["up", "enter"]}));
    let confirm = wait_screen(&mut rpc, "r4", |ls| ls.iter().any(|l| l.contains("Restore conversation")));
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
    rpc.call("r6", json!({"type": "prompt", "text": "Reply with exactly REWOUND."}));
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
    assert!(!texts.iter().any(|t| t.starts_with("Call the Workflow tool")), "{texts:#?}");

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
        if let Ok(v) = rpc.rx.recv_timeout(Duration::from_millis(100)) {
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
    s.write_all(&Frame::Resize { cols: 100, rows: 30 }.encode()).unwrap();
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

    // Another ccrpc refuses the Session while the person holds it, unless forced.
    let out = ccrpc_cmd(&env)
        .args(["attach", &holder])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("正被 e2e-person 使用"), "{err}");
    let mut forced = Rpc::attach(&env, &["--force", &holder]);
    let ready = forced.event("ready", Duration::from_secs(20));
    assert_eq!(ready["holder_id"], holder.as_str());
    forced.call("f1", json!({"type": "get_state"}));
    drop(forced);
    drop(person);
}

/// Polls `get_screen` until `pred` holds on its lines (logged on failure).
fn wait_screen(rpc: &mut Rpc, id: &str, pred: impl Fn(&[String]) -> bool) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut n = 0;
    loop {
        n += 1;
        let scr = rpc.call(&format!("{id}-{n}"), json!({"type": "get_screen"}));
        let lines: Vec<String> = scr["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l.as_str().unwrap_or("").to_string())
            .collect();
        if pred(&lines) {
            return lines;
        }
        if Instant::now() >= deadline {
            panic!("屏幕没出现预期内容：{lines:#?}");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Waits until the Session has stayed idle for 3 s (background work may start more turns).
fn settle_idle(rpc: &mut Rpc) {
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut idle_since = Some(Instant::now());
    let st = rpc.call("settle", json!({"type": "get_state"}));
    if st["state"] != "idle" {
        idle_since = None;
    }
    loop {
        assert!(Instant::now() < deadline, "没有稳定在 idle");
        if idle_since.is_some_and(|t| t.elapsed() > Duration::from_secs(3)) {
            return;
        }
        if let Ok(v) = rpc.rx.recv_timeout(Duration::from_millis(200)) {
            eprintln!("{:6.1} {v}", rpc.t0.elapsed().as_secs_f32());
            if v["type"] == "branch" {
                rpc.branches.push(v.clone());
            }
            if v["type"] == "state" {
                idle_since = (v["state"] == "idle").then(Instant::now);
            }
        }
    }
}
