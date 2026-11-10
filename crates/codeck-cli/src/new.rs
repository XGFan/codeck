//! `new`: starts a Session in a clay Pane and sends the first prompt, the way the desktop app's
//! 新建 Session does (same launch rules and hidden-Pane setting), then lets go: ccrpc disconnects
//! when its stdin closes and the Session keeps running, an approval waiting for a person in the TUI.
//!
//! ccrpc's `ready` often precedes the plugin's check-in, so its `session_id` may be null and arrive
//! later as `session_changed`; the prompt goes out once the Session is known.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use codeck_host::ccrpc::{env, launch};
use codeck_host::settings::{self, LaunchRule};
use serde::Serialize;
use serde_json::{Value, json};

use crate::args::Parsed;
use crate::common::{CliError, cap};

/// How long ccrpc may take to start the Session, and how long the first prompt may take to go out.
const STEP_TIMEOUT: Duration = Duration::from_secs(60);
const TRUST_LAYOUT: &str = "trust_folder";
/// ccrpc needs up to 10 s to see the folder trust dialog close.
const TRUST_ANSWER_WAIT: Duration = Duration::from_secs(15);
/// After closing its stdin, how long ccrpc gets to detach before we exit without it.
const DETACH_WAIT: Duration = Duration::from_secs(5);
const DETACH_POLL: Duration = Duration::from_millis(50);
const STDERR_DRAIN: Duration = Duration::from_secs(1);
/// ccrpc's last stderr line in the error sentence.
const REASON_CHARS: usize = 200;

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Started {
    pub session_id: String,
    pub holder_id: String,
    pub cwd: String,
}

pub struct Launch<'a> {
    pub rules: &'a [LaunchRule],
    pub hidden: bool,
}

pub fn run(a: &Parsed) -> Result<Started, CliError> {
    if !a.positional.is_empty() {
        return Err(CliError::usage("提示词从 stdin 读，不收位置参数"));
    }
    let Some(cwd) = a.value("--cwd") else {
        return Err(CliError::usage("需要 --cwd"));
    };
    let cwd = std::path::absolute(cwd).unwrap_or_else(|_| PathBuf::from(cwd));
    if !cwd.is_dir() {
        return Err(CliError::new(
            "invalid_cwd",
            format!("{} 不是目录", cwd.display()),
        ));
    }
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Err(CliError::usage("提示词从 stdin 读"));
    }
    let mut prompt = String::new();
    stdin
        .read_to_string(&mut prompt)
        .map_err(|e| CliError::new("io", format!("读 stdin 失败：{e}")))?;
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err(CliError::new("empty_prompt", "提示词为空"));
    }
    // ccrpc runs text starting with `!` as a TUI shell command, which no approval stops. It drops
    // control characters first, so they cannot hide the `!`.
    if prompt
        .chars()
        .find(|c| !c.is_control() && !c.is_whitespace())
        == Some('!')
    {
        return Err(CliError::new("shell_prompt", "提示词不能以 ! 开头"));
    }
    let s = settings::load();
    let env = env::with_analyzer(&env::login_env(), &s.analyzer);
    start(
        &env,
        &Launch {
            rules: &s.launch_rules,
            hidden: s.hide_new_panes,
        },
        &cwd,
        prompt,
        STEP_TIMEOUT,
    )
}

/// `env` is the environment ccrpc runs in, and where `CODECK_CCRPC` is looked up.
pub fn start(
    env: &HashMap<String, String>,
    launch: &Launch<'_>,
    cwd: &Path,
    prompt: &str,
    timeout: Duration,
) -> Result<Started, CliError> {
    let mut args: Vec<String> = vec!["new".into()];
    if launch.hidden {
        args.push("--hidden".into());
    }
    args.extend(["--cwd".to_string(), cwd.display().to_string()]);
    let extra = launch::args_for(launch.rules, cwd, env)
        .map_err(|e| CliError::new("invalid_launch_rule", e))?;
    if !extra.is_empty() {
        args.push("--".into());
        args.extend(extra);
    }
    let mut child = Command::new(env::ccrpc_path(env))
        .args(&args)
        .env_clear()
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CliError::new("ccrpc_failed", format!("启动 ccrpc 失败：{e}")))?;

    let mut stdin = child.stdin.take().expect("stdin was piped");
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    let (tx, rx) = channel::<Value>();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<Value>(&line)
                && tx.send(v).is_err()
            {
                break;
            }
        }
    });
    // ccrpc reports why it could not start as one last line on stderr.
    let last_err: Arc<Mutex<String>> = Arc::default();
    let sink = last_err.clone();
    let (drained_tx, drained) = channel::<()>();
    thread::spawn(move || {
        let _drained = drained_tx;
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let line = line.trim();
            if !line.is_empty() {
                *sink.lock().unwrap() = line.to_owned();
            }
        }
    });

    let driven = drive(&rx, &mut stdin, prompt, timeout, cwd);
    drop(stdin);
    let result = driven.map_err(|e| {
        e.resolve(|| {
            let code = child.wait().ok().and_then(|s| s.code());
            // Bounded: a process ccrpc left behind may hold its stderr open.
            let _ = drained.recv_timeout(STDERR_DRAIN);
            early_exit_message(&last_err, code)
        })
    });
    if result.is_ok() {
        let until = Instant::now() + DETACH_WAIT;
        while Instant::now() < until && matches!(child.try_wait(), Ok(None)) {
            thread::sleep(DETACH_POLL);
        }
    }
    result
}

enum Failure {
    Error(CliError),
    /// ccrpc's stdout ended; the reason is whatever it left on stderr.
    Exited {
        holder_id: Option<String>,
    },
}

impl Failure {
    fn resolve(self, reason: impl FnOnce() -> String) -> CliError {
        match self {
            Failure::Error(e) => e,
            Failure::Exited { holder_id } => CliError {
                code: "ccrpc_failed",
                message: reason(),
                holder_id,
            },
        }
    }
}

fn early_exit_message(last_err: &Mutex<String>, code: Option<i32>) -> String {
    let last = last_err.lock().unwrap();
    let reason = cap(last.strip_prefix("ccrpc: ").unwrap_or(&last), REASON_CHARS);
    match (reason.is_empty(), code) {
        (false, _) => format!("ccrpc 提前退出：{reason}"),
        (true, Some(c)) => format!("ccrpc 提前退出，退出码 {c}"),
        (true, None) => "ccrpc 提前退出".to_owned(),
    }
}

fn drive(
    rx: &Receiver<Value>,
    stdin: &mut ChildStdin,
    prompt: &str,
    timeout: Duration,
    cwd: &Path,
) -> Result<Started, Failure> {
    let deadline = Instant::now() + timeout;
    let mut holder: Option<String> = None;
    let mut session: Option<String> = None;
    let mut ready = false;
    let mut not_ready = "ccrpc 还没有准备好";
    while !(ready && session.is_some()) {
        let v = next(rx, deadline, not_ready, timeout, &holder)?;
        match v["type"].as_str() {
            // Any startup screen, some of which pass by themselves: not a failure yet.
            Some("dialog") => not_ready = "新 Session 启动时停在一个对话框上",
            Some("ready") => {
                holder = v["holder_id"].as_str().map(str::to_owned);
                session = v["session_id"].as_str().map(str::to_owned);
                ready = true;
            }
            Some("session_changed") => session = v["session_id"].as_str().map(str::to_owned),
            Some("choice") if v["layout"] == TRUST_LAYOUT => {
                return Err(Failure::Error(refuse_trust(rx, stdin, &v, holder.clone())));
            }
            Some("choice") => {
                return Err(fail(
                    "choice_pending",
                    "新 Session 启动时有个选择在等人作答".to_owned(),
                    &holder,
                ));
            }
            Some("session_exit") => {
                return Err(fail("exited", "claude 启动后就退出了".to_owned(), &holder));
            }
            _ => {}
        }
    }
    let holder = holder.unwrap_or_default();
    let session = session.unwrap_or_default();
    let known = Some(holder.clone());
    send(stdin, &json!({"id": "1", "type": "prompt", "text": prompt}))
        .map_err(|_| fail("prompt_failed", "提示词发送失败".to_owned(), &known))?;

    let deadline = Instant::now() + timeout;
    loop {
        let v = next(rx, deadline, "提示词还没有送达", timeout, &known)?;
        match v["type"].as_str() {
            Some("response") if v["id"] == "1" => {
                if v["success"] == true {
                    break;
                }
                let why = v["error"]["message"].as_str().unwrap_or("未知原因");
                return Err(fail(
                    "prompt_failed",
                    format!("提示词发送失败：{why}"),
                    &known,
                ));
            }
            Some("session_exit") => {
                return Err(fail(
                    "exited",
                    "claude 在提示词送达前退出了".to_owned(),
                    &known,
                ));
            }
            _ => {}
        }
    }
    Ok(Started {
        session_id: session,
        holder_id: holder,
        cwd: cwd.display().to_string(),
    })
}

fn fail(code: &'static str, message: String, holder: &Option<String>) -> Failure {
    Failure::Error(CliError {
        code,
        message,
        holder_id: holder.clone(),
    })
}

fn next(
    rx: &Receiver<Value>,
    deadline: Instant,
    waiting_for: &str,
    timeout: Duration,
    holder: &Option<String>,
) -> Result<Value, Failure> {
    match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(v) => Ok(v),
        Err(RecvTimeoutError::Timeout) => Err(fail(
            "timeout",
            format!("等了 {} 秒，{waiting_for}", timeout.as_secs()),
            holder,
        )),
        Err(RecvTimeoutError::Disconnected) => Err(Failure::Exited {
            holder_id: holder.clone(),
        }),
    }
}

fn send(stdin: &mut ChildStdin, v: &Value) -> std::io::Result<()> {
    writeln!(stdin, "{v}")?;
    stdin.flush()
}

/// The folder trust dialog is a security boundary: an AI does not answer it with yes. Answering no
/// makes this claude exit, so no empty Session is left behind; the Pane's shell is then closed too.
fn refuse_trust(
    rx: &Receiver<Value>,
    stdin: &mut ChildStdin,
    choice: &Value,
    holder: Option<String>,
) -> CliError {
    let mut refused = CliError {
        code: "untrusted_dir",
        message: "目录还没有信任，先在终端里用 claude 打开一次".to_owned(),
        holder_id: holder,
    };
    let deny = choice["options"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|o| o["meaning"] == "deny")
        .and_then(|o| o["label"].as_str());
    let Some(deny) = deny else {
        return refused;
    };
    let answer = json!({
        "id": "trust",
        "type": "answer_choice",
        "request_id": choice["request_id"],
        "option": deny,
    });
    if send(stdin, &answer).is_err() {
        return refused;
    }
    let until = Instant::now() + TRUST_ANSWER_WAIT;
    // Not the answer's response: claude exiting can make it report a failure. Only once claude is gone
    // does `exit` end the Pane's shell without typing anything at claude.
    let mut exited = false;
    while let Ok(v) = rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
        if v["type"] == "session_exit" || (v["type"] == "state" && v["state"] == "exited") {
            exited = true;
            break;
        }
    }
    let close = json!({"id": "close", "type": "exit", "close_pane": true});
    if !exited || send(stdin, &close).is_err() {
        return refused;
    }
    while let Ok(v) = rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
        if v["type"] == "response" && v["id"] == "close" {
            if v["success"] == true {
                refused.holder_id = None;
            }
            break;
        }
    }
    refused
}
