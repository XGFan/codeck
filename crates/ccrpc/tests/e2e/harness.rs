//! The isolated ToH, Claude Code config and ccrpc state of a test, and `Rpc`: one ccrpc
//! process, its lines pumped through `Rpc::recv` (all logged, and recorded with
//! `CCRPC_E2E_WIRE_DIR`).

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub(crate) const HOST_TERMINAL_VARS: &[&str] = &[
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

pub(crate) struct Env {
    pub(crate) _toh: tempfile::TempDir,
    pub(crate) _state: tempfile::TempDir,
    pub(crate) _plugin: tempfile::TempDir,
    /// ccrpc's state directory (check-ins, hook sockets).
    pub(crate) state_dir: PathBuf,
    /// The generated ccrpc Plugin (`--plugin-dir`).
    pub(crate) plugin_dir: PathBuf,
    pub(crate) _cc: tempfile::TempDir,
    pub(crate) _work: tempfile::TempDir,
    pub(crate) _layouts: tempfile::TempDir,
    pub(crate) toh_dir: PathBuf,
    pub(crate) cc_dir: PathBuf,
    pub(crate) work: PathBuf,
    /// The test's own layout library (never the owner's).
    pub(crate) layout_dir: PathBuf,
    /// The model key, for the analyzer when `analyzer` is on.
    pub(crate) key: String,
    pub(crate) analyzer: bool,
}

pub(crate) fn setup(key: &str) -> Env {
    let base = std::env::var("CCRPC_E2E_BASE_URL")
        .unwrap_or_else(|_| "https://api.deepseek.com/anthropic".into());
    let model = std::env::var("CCRPC_E2E_MODEL").unwrap_or_else(|_| "deepseek-flash".into());
    setup_with(key, &base, &model)
}

pub(crate) fn setup_with(key: &str, base: &str, model: &str) -> Env {
    let (base, model) = (base.to_string(), model.to_string());
    let toh = tempfile::Builder::new()
        .prefix("ccrpc-e2e-")
        .tempdir_in("/tmp")
        .unwrap();
    let cc = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let state = tempfile::Builder::new()
        .prefix("ccrpc-st-")
        .tempdir_in("/tmp")
        .unwrap();
    let state_dir = state.path().canonicalize().unwrap();
    let plugin = tempfile::tempdir().unwrap();
    let plugin_dir =
        ccrpc::plugin::generate(plugin.path(), Path::new(env!("CARGO_BIN_EXE_ccrpc"))).unwrap();
    let toh_dir = toh.path().canonicalize().unwrap();
    let cc_dir = cc.path().canonicalize().unwrap();
    let work_dir = work.path().canonicalize().unwrap();
    let env_vars = [
        ("CLAUDE_CONFIG_DIR", cc_dir.display().to_string()),
        ("CCRPC_STATE_DIR", state_dir.display().to_string()),
        ("ANTHROPIC_BASE_URL", base),
        ("ANTHROPIC_AUTH_TOKEN", key.to_string()),
        ("ANTHROPIC_MODEL", model.clone()),
        ("ANTHROPIC_DEFAULT_HAIKU_MODEL", model.clone()),
        ("ANTHROPIC_DEFAULT_SONNET_MODEL", model.clone()),
        ("ANTHROPIC_DEFAULT_OPUS_MODEL", model.clone()),
        ("CLAUDE_CODE_SUBAGENT_MODEL", model),
        ("DISABLE_AUTOUPDATER", "1".into()),
        // On as for a real account: an empty input box then shows a dim `Try "…"` placeholder.
        ("CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION", "1".into()),
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
    std::fs::write(cc_dir.join("settings.json"), settings_json().to_string()).unwrap();
    std::fs::write(work_dir.join("a.txt"), "hello\n").unwrap();
    let layouts = tempfile::tempdir().unwrap();
    let layout_dir = layouts.path().to_path_buf();
    Env {
        _toh: toh,
        _state: state,
        _plugin: plugin,
        state_dir,
        plugin_dir,
        _cc: cc,
        _work: work,
        _layouts: layouts,
        toh_dir,
        cc_dir,
        work: work_dir,
        layout_dir,
        key: key.to_string(),
        analyzer: false,
    }
}

/// `CCRPC_E2E_TUI=fullscreen` runs Claude Code with the fullscreen TUI (the owner's daily setup).
pub(crate) fn settings_json() -> Value {
    let mut v = json!({"permissions": {"defaultMode": "default"}});
    if std::env::var("CCRPC_E2E_TUI").as_deref() == Ok("fullscreen") {
        v["tui"] = json!("fullscreen");
    }
    v
}

pub(crate) fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

pub(crate) struct Rpc {
    /// `CCRPC_E2E_WIRE_DIR`: every line sent and received, as CoDeck's drive `wire.jsonl`.
    pub(crate) wire: Option<Arc<Mutex<std::fs::File>>>,
    pub(crate) child: Child,
    pub(crate) stdin: Option<ChildStdin>,
    pub(crate) rx: Receiver<Value>,
    pub(crate) t0: Instant,
    /// The Pane's shell: hung up when the test is done (the Pane ends with it).
    pub(crate) shell_pid: Option<u32>,
    pub(crate) seen: Vec<Value>,
    /// Every `branch` event received.
    pub(crate) branches: Vec<Value>,
}

/// The `toh` to run: `CCRPC_E2E_TOH`, else the sibling checkout's debug build, else PATH's.
pub(crate) fn toh_bin() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CCRPC_E2E_TOH") {
        return Some(PathBuf::from(p));
    }
    let sibling = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../toh/target/debug/toh");
    sibling.is_file().then(|| sibling.canonicalize().unwrap())
}

pub(crate) fn ccrpc_cmd(env: &Env) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ccrpc"));
    cmd.env("TOH_STATE_DIR", &env.toh_dir)
        .env("CLAUDE_CONFIG_DIR", &env.cc_dir)
        .env("CCRPC_STATE_DIR", &env.state_dir)
        .env("CCRPC_LAYOUT_DIR", &env.layout_dir)
        .env("CCRPC_ANALYSIS_DIR", env.state_dir.join("analyses"))
        .env_remove("CCRPC_ANALYZER_API_KEY");
    if env.analyzer {
        // Learning is what is tested: start from no built-in layouts.
        cmd.env("CCRPC_BUILTIN_LAYOUTS", "0");
        cmd.env("CCRPC_ANALYZER_API_KEY", &env.key);
        if let Ok(b) = std::env::var("CCRPC_E2E_BASE_URL") {
            cmd.env("CCRPC_ANALYZER_BASE_URL", b);
        }
        if let Ok(m) = std::env::var("CCRPC_E2E_MODEL") {
            cmd.env("CCRPC_ANALYZER_MODEL", m);
        }
    }
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
    pub(crate) fn start(env: &Env) -> Rpc {
        Rpc::start_in(env, &env.work, &[])
    }

    /// `new` in `cwd` with ccrpc options `opts`.
    pub(crate) fn start_in(env: &Env, cwd: &Path, opts: &[&str]) -> Rpc {
        let mut cmd = ccrpc_cmd(env);
        cmd.args(["new", "--cwd"])
            .arg(cwd)
            .args(opts)
            .args(["--", "--plugin-dir"])
            .arg(&env.plugin_dir);
        Rpc::spawn(cmd)
    }

    pub(crate) fn attach(env: &Env, args: &[&str]) -> Rpc {
        let mut cmd = ccrpc_cmd(env);
        cmd.arg("attach").args(args);
        Rpc::spawn(cmd)
    }

    pub(crate) fn spawn(mut cmd: Command) -> Rpc {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = cmd.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let wire = wire_file();
        let wire_out = wire.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let v: Value = serde_json::from_str(&line)
                    .unwrap_or_else(|e| panic!("非 JSON 输出 {line:?}: {e}"));
                wire_write(&wire_out, "recv", &v);
                if tx.send(v).is_err() {
                    break;
                }
            }
        });
        let stdin = child.stdin.take();
        Rpc {
            wire,
            child,
            stdin,
            rx,
            t0: Instant::now(),
            shell_pid: None,
            seen: Vec::new(),
            branches: Vec::new(),
        }
    }

    pub(crate) fn send(&mut self, c: Value) {
        eprintln!("{:6.1} >>> {c}", self.t0.elapsed().as_secs_f32());
        wire_write(&self.wire, "send", &c);
        let w = self.stdin.as_mut().unwrap();
        writeln!(w, "{c}").unwrap();
        w.flush().unwrap();
    }

    /// The next line from ccrpc within `timeout`. Every line received goes through here: it is
    /// logged (a dialog's screen in full) and a `branch` is kept in `branches`.
    pub(crate) fn recv(&mut self, timeout: Duration) -> Option<Value> {
        let v = self.rx.recv_timeout(timeout).ok()?;
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
        Some(v)
    }

    /// Waits for a line matching `pred`; the others stay in `seen` for later waits.
    pub(crate) fn until(
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
            let Some(v) = self.recv(left) else {
                panic!("等 {what} 超时");
            };
            if pred(&v) {
                return v;
            }
            self.seen.push(v);
        }
    }

    pub(crate) fn call(&mut self, id: &str, c: Value) -> Value {
        let mut c = c;
        c["id"] = json!(id);
        self.send(c);
        let r = self.until(&format!("响应 {id}"), Duration::from_secs(90), |v| {
            v["type"] == "response" && v["id"] == id
        });
        assert_eq!(r["success"], true, "命令 {id} 失败：{r}");
        r["data"].clone()
    }

    pub(crate) fn event(&mut self, ty: &str, timeout: Duration) -> Value {
        self.until(ty, timeout, |v| v["type"] == ty)
    }

    /// Drops everything received so far (so the next `until` only sees what comes after).
    pub(crate) fn forget(&mut self) {
        while self.recv(Duration::ZERO).is_some() {}
        self.seen.clear();
    }
}

/// A new wire file in `CCRPC_E2E_WIRE_DIR`, named after the test (and a sequence number).
pub(crate) fn wire_file() -> Option<Arc<Mutex<std::fs::File>>> {
    let dir = PathBuf::from(std::env::var_os("CCRPC_E2E_WIRE_DIR")?);
    std::fs::create_dir_all(&dir).unwrap();
    // The test function's name (`mock_model_tools-0.jsonl`; scripts/record-connect-fixtures.sh
    // reads these names).
    let thread = std::thread::current();
    let test = thread
        .name()
        .and_then(|n| n.rsplit("::").next())
        .unwrap_or("e2e");
    let path = (0..)
        .map(|n| dir.join(format!("{test}-{n}.jsonl")))
        .find(|p| !p.exists())
        .unwrap();
    Some(Arc::new(Mutex::new(std::fs::File::create(path).unwrap())))
}

/// One wire line: `{"t": <unix ms>, "dir": "send" | "recv", "msg": …}`.
pub(crate) fn wire_write(wire: &Option<Arc<Mutex<std::fs::File>>>, dir: &str, msg: &Value) {
    let Some(w) = wire else { return };
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let _ = writeln!(
        w.lock().unwrap(),
        "{}",
        json!({"t": t, "dir": dir, "msg": msg})
    );
}

impl Drop for Rpc {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.wait();
        if let Some(pid) = self.shell_pid {
            // SAFETY: only the shell pid recorded for this test's own Pane; the Pane ends with it.
            unsafe { libc::kill(pid as i32, libc::SIGHUP) };
        }
    }
}

/// The shell of this test's Pane `holder`, as `toh ls --json` lists it.
pub(crate) fn shell_pid(env: &Env, holder: &str) -> Option<u32> {
    let mut cmd = Command::new(toh_bin().unwrap_or_else(|| PathBuf::from("toh")));
    cmd.args(["ls", "--json"])
        .env("TOH_STATE_DIR", &env.toh_dir)
        .env_remove("TOH_CONFIG");
    for k in HOST_TERMINAL_VARS {
        cmd.env_remove(k);
    }
    let out = cmd.output().ok()?;
    let panes: Value = serde_json::from_slice(&out.stdout).ok()?;
    panes.as_array()?.iter().find(|p| p["id"] == holder)?["shell_pid"]
        .as_u64()
        .map(|p| p as u32)
}

/// A raw ToH client standing in for a person's: attaches to `holder` and sets the PTY size.
pub(crate) fn person_resizes(
    env: &Env,
    holder: &str,
    cols: u16,
    rows: u16,
) -> std::os::unix::net::UnixStream {
    use toh_proto::{AttachInfo, Frame};
    let mut cmd = Command::new(toh_bin().unwrap_or_else(|| PathBuf::from("toh")));
    cmd.args(["ls", "--json"])
        .env("TOH_STATE_DIR", &env.toh_dir)
        .env_remove("TOH_CONFIG");
    let panes: Value = serde_json::from_slice(&cmd.output().unwrap().stdout).unwrap();
    let sock = panes
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == holder)
        .unwrap()["socket"]
        .as_str()
        .unwrap()
        .to_string();
    let mut s = std::os::unix::net::UnixStream::connect(sock).unwrap();
    let attach = Frame::Attach(AttachInfo {
        client: "e2e-person".into(),
        cols,
        rows,
        ..Default::default()
    });
    s.write_all(&attach.encode()).unwrap();
    s.write_all(&Frame::Resize { cols, rows }.encode()).unwrap();
    s
}

/// Waits for `ready` and a Session that stays idle (answering the startup dialogs); returns `ready`.
pub(crate) fn start_session(rpc: &mut Rpc, env: &Env) -> Value {
    let ready = rpc.event("ready", Duration::from_secs(20));
    assert_eq!(ready["protocol"], 3);
    let holder = ready["holder_id"].as_str().unwrap().to_string();
    rpc.shell_pid = shell_pid(env, &holder);
    assert!(rpc.shell_pid.is_some());
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
        let Some(v) = rpc.recv(Duration::from_millis(200)) else {
            continue;
        };
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
pub(crate) fn run_turn_allowing(rpc: &mut Rpc, id: &str, text: &str) -> Vec<Value> {
    rpc.forget();
    rpc.call(id, json!({"type": "prompt", "text": text}));
    let mut seen = Vec::new();
    let mut n = 0;
    loop {
        let v = rpc.until("turn", Duration::from_secs(180), |v| {
            v["type"] == "turn_end"
                || v["type"] == "permission_request"
                || v["type"] == "block"
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

/// Polls `get_screen` until `pred` holds on its lines (logged on failure).
pub(crate) fn wait_screen(
    rpc: &mut Rpc,
    id: &str,
    pred: impl Fn(&[String]) -> bool,
) -> Vec<String> {
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
pub(crate) fn settle_idle(rpc: &mut Rpc) {
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
        if let Some(v) = rpc.recv(Duration::from_millis(200))
            && v["type"] == "state"
        {
            idle_since = (v["state"] == "idle").then(Instant::now);
        }
    }
}

/// Prints the screen under a name (layout probing).
pub(crate) fn dump(rpc: &mut Rpc, name: &str) -> Vec<String> {
    let scr = rpc.call(&format!("dump-{name}"), json!({"type": "get_screen"}));
    let lines: Vec<String> = scr["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap_or("").to_string())
        .collect();
    eprintln!("SCREEN[{name}] cursor={}", scr["cursor"]);
    for (i, l) in lines.iter().enumerate() {
        eprintln!("SCREEN[{name}]|{i:02}|{l}");
    }
    lines
}

/// Waits for a `state` event whose state is `state` (and `waiting.kind`, if given).
pub(crate) fn wait_state(rpc: &mut Rpc, state: &str, secs: u64) -> Value {
    rpc.until(state, Duration::from_secs(secs), |v| {
        v["type"] == "state" && v["state"] == state
    })
}

pub(crate) fn red_dot_png() -> Vec<u8> {
    base64::engine::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==",
    )
    .unwrap()
}
