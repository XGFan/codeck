//! End to end against a real interactive Claude Code in an isolated ToH clay Pane.
//!
//! `CCRPC_E2E_API_KEY=… cargo test --test e2e -- --ignored --nocapture --test-threads=1`
//! Optional: `CCRPC_E2E_BASE_URL` (default DeepSeek's Anthropic endpoint), `CCRPC_E2E_MODEL`
//! (default `deepseek-flash`). Spends model credits. Needs `toh` and `claude` on PATH.
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
}

impl Rpc {
    fn start(env: &Env) -> Rpc {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_ccrpc"));
        cmd.args(["new", "--cwd"]).arg(&env.work);
        cmd.env("TOH_STATE_DIR", &env.toh_dir)
            .env("CLAUDE_CONFIG_DIR", &env.cc_dir);
        for k in HOST_TERMINAL_VARS {
            cmd.env_remove(k);
        }
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

#[test]
#[ignore = "real Claude Code; needs CCRPC_E2E_API_KEY, toh and claude"]
fn controls_a_real_session() {
    let Ok(key) = std::env::var("CCRPC_E2E_API_KEY") else {
        eprintln!("跳过：没有 CCRPC_E2E_API_KEY");
        return;
    };
    let env = setup(&key);
    let mut rpc = Rpc::start(&env);
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
