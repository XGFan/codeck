//! The relay against real ccrpc and Claude Code, in the sandbox of scripts/drive-sandbox (own ToH /
//! claude / ccrpc directories, the local mock model; nothing of the owner's is touched):
//!
//! ```sh
//! (cargo build --release -p ccrpc)
//! python3 -I scripts/drive-sandbox/mock_model.py 18765 &
//! cd src-tauri && cargo test --test drive_sandbox -- --ignored --nocapture
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use codeck_lib::drive::relay::{App, Out};
use serde_json::{Value, json};

struct Holder {
    toh: PathBuf,
    id: Option<String>,
}

impl Drop for Holder {
    fn drop(&mut self) {
        // Only the Holder this test started.
        let Some(id) = &self.id else { return };
        let Ok(b) = std::fs::read(self.toh.join("holders").join(format!("{id}.json"))) else {
            return;
        };
        let v: Value = serde_json::from_slice(&b).unwrap_or(Value::Null);
        if let Some(pid) = v["holder_pid"].as_i64() {
            unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        }
    }
}

fn next(
    rx: &mpsc::Receiver<Out>,
    what: &str,
    secs: u64,
    mut pred: impl FnMut(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(Out::Line { line, .. }) => {
                eprintln!("<<< {}", &line[..line.floor_char_boundary(200)]);
                let v: Value = serde_json::from_str(&line).unwrap();
                if pred(&v) {
                    return v;
                }
            }
            Ok(Out::Exit(e)) => panic!("等 {what} 时 ccrpc 退出了：{e:?}"),
            Err(_) => panic!("等 {what} 超时"),
        }
    }
}

#[test]
#[ignore = "real ccrpc + claude in the sandbox, with scripts/drive-sandbox/mock_model.py on :18765"]
fn relays_a_real_session_and_logs_it() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let sandbox = PathBuf::from("/tmp/codeck-drive-sandbox-test");
    let out = Command::new(root.join("scripts/drive-sandbox/env.sh"))
        .env("CODECK_SANDBOX", &sandbox)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "env.sh: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let work = String::from_utf8(out.stdout).unwrap().trim().to_string();

    let mut env: HashMap<String, String> = std::env::vars().collect();
    for k in [
        "TOH_CONFIG",
        "CLAUDECODE",
        "CLAUDE_CODE_ENTRYPOINT",
        "TOH_HOLDER_ID",
        "KITTY_WINDOW_ID",
        "WEZTERM_PANE",
        "KAKU_UNIX_SOCKET",
        "ITERM_SESSION_ID",
    ] {
        env.remove(k);
    }
    let s = |p: &str| sandbox.join(p).display().to_string();
    env.insert("TOH_STATE_DIR".into(), s("toh"));
    env.insert("CLAUDE_CONFIG_DIR".into(), s("cc"));
    env.insert("CCRPC_STATE_DIR".into(), s("st"));
    env.insert("CCRPC_LAYOUT_DIR".into(), s("layouts"));
    if let Ok(toh) = root.join("../toh/target/debug").canonicalize() {
        let path = env.get("PATH").cloned().unwrap_or_default();
        env.insert("PATH".into(), format!("{}:{path}", toh.display()));
    }
    let logs = tempfile::tempdir().unwrap();
    let app = App::new(
        env,
        sandbox.join("ccrpc-wrap"),
        logs.path().to_path_buf(),
        logs.path().join("img"),
    )
    .unwrap();
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let sink = Arc::new(move |o: Out| {
        let _ = tx.lock().unwrap().send(o);
    });

    let conn = app.new_session(sink, &work).unwrap();
    let mut holder = Holder {
        toh: sandbox.join("toh"),
        id: None,
    };
    let ready = next(&rx, "ready", 60, |v| v["type"] == "ready");
    let hid = ready["holder_id"].as_str().unwrap().to_string();
    holder.id = Some(hid.clone());
    assert_eq!(ready["co_display"], true);
    // A new Session's log directory is renamed after its Holder ID.
    let dir = app.log_dir(&conn).unwrap();
    assert!(
        dir.file_name().unwrap().to_str().unwrap().starts_with(&hid),
        "{}",
        dir.display()
    );

    let mut seq = 0;
    let mut send = |app: &App, mut v: Value| -> String {
        seq += 1;
        let id = format!("t{seq}");
        v["id"] = json!(id);
        app.send(&conn, &v.to_string()).unwrap();
        id
    };
    // Idle (a startup notice offering auto mode is declined, as in ccrpc's e2e).
    loop {
        let v = next(&rx, "idle", 60, |v| {
            (v["type"] == "state" && v["state"] == "idle") || v["type"] == "dialog"
        });
        if v["type"] == "dialog" {
            send(
                &app,
                json!({"type": "send_keys", "keys": ["down", "enter"]}),
            );
            continue;
        }
        break;
    }
    std::thread::sleep(Duration::from_secs(2));

    let p = send(&app, json!({"type": "prompt", "text": "MOCK-TOOL please"}));
    let pr = next(&rx, "permission_request", 60, |v| {
        v["type"] == "permission_request"
    });
    assert!(
        pr["options"].as_array().is_some_and(|o| o.len() >= 2),
        "{pr}"
    );
    send(
        &app,
        json!({"type": "answer_permission", "request_id": pr["request_id"], "decision": "allow"}),
    );
    let end = next(&rx, "turn_end", 60, |v| v["type"] == "turn_end");
    assert_eq!(end["aborted"], false);
    assert!(Path::new(&work).join("mock-tool.txt").exists());
    let m = send(&app, json!({"type": "get_messages", "limit": 50}));
    let page = next(&rx, "get_messages", 30, |v| {
        v["type"] == "response" && v["id"] == m
    });
    assert!(
        page["data"]["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["kind"] == "tool_result"),
        "{page}"
    );

    send(&app, json!({"type": "exit", "close_pane": true}));
    next(&rx, "session_exit", 30, |v| v["type"] == "session_exit");
    app.disconnect(&conn);
    let deadline = Instant::now() + Duration::from_secs(15);
    let code = loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Out::Exit(e)) => break e.code,
            Ok(_) => continue,
            Err(_) => panic!("ccrpc 没有退出"),
        }
    };
    assert_eq!(code, Some(0));
    holder.id = None;

    // The three logs, in the connection's directory.
    let wire = std::fs::read_to_string(dir.join("wire.jsonl")).unwrap();
    let rows: Vec<Value> = wire
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(rows[0]["dir"], "recv");
    assert_eq!(rows[0]["msg"]["type"], "ready");
    assert!(
        rows.iter()
            .any(|r| r["dir"] == "send" && r["msg"]["id"] == p)
    );
    let ccrpc_log = std::fs::read_to_string(dir.join("ccrpc.log")).unwrap();
    assert!(
        ccrpc_log.contains(&format!("holder={hid}")),
        "ccrpc.log 没有版本行"
    );
    let gui = std::fs::read_to_string(dir.join("gui.log")).unwrap();
    assert!(
        gui.contains("spawn") && gui.contains("exit code=Some(0)"),
        "{gui}"
    );
}
