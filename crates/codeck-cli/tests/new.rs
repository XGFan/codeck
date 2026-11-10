//! `new` against a fake ccrpc (`CODECK_CCRPC`); no real Session is started.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use codeck_cli::new::{Launch, start};
use codeck_host::settings::LaunchRule;
use serde_json::Value;
use tempfile::TempDir;

/// Writes `body` as an executable script; it records what it receives next to itself.
fn fake(body: &str) -> (TempDir, HashMap<String, String>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ccrpc");
    fs::write(
        &path,
        format!("#!/bin/sh\nD=\"$(dirname \"$0\")\"\nprintf '%s\\n' \"$@\" > \"$D/args\"\n{body}"),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let env = HashMap::from([
        ("PATH".to_string(), std::env::var("PATH").unwrap()),
        (
            "CODECK_CCRPC".to_string(),
            path.to_string_lossy().into_owned(),
        ),
    ]);
    (dir, env)
}

fn sent(dir: &Path) -> Vec<Value> {
    fs::read_to_string(dir.join("sent"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

const NO_LAUNCH: Launch<'static> = Launch {
    rules: &[],
    hidden: false,
};

fn cwd() -> PathBuf {
    std::env::temp_dir()
}

const STARTED: &str = r#"
echo '{"type":"ready","protocol":3,"holder_id":"H1","session_id":null,"cwd":"/w","state":"idle"}'
echo '{"type":"session_changed","session_id":"S-1","previous":null}'
IFS= read -r line; printf '%s\n' "$line" >> "$D/sent"
echo '{"type":"response","id":"1","command":"prompt","success":true,"data":{"uuid":"u"}}'
cat > /dev/null
touch "$D/closed"
"#;

#[test]
fn sends_the_prompt_once_the_session_is_known_then_lets_go() {
    let (dir, env) = fake(STARTED);
    let rules = [LaunchRule {
        dir: "/".into(),
        args: "--add-dir /extra".into(),
    }];
    let started = start(
        &env,
        &Launch {
            rules: &rules,
            hidden: true,
        },
        &cwd(),
        "第一行\n第二行",
        Duration::from_secs(30),
    )
    .unwrap();
    assert_eq!(started.session_id, "S-1");
    assert_eq!(started.holder_id, "H1");

    let args = fs::read_to_string(dir.path().join("args")).unwrap();
    let args: Vec<&str> = args.lines().collect();
    assert_eq!(args[0], "new");
    assert_eq!(args[1], "--hidden");
    assert_eq!(args[2], "--cwd");
    assert_eq!(&args[4..], ["--", "--add-dir", "/extra"]);

    let sent = sent(dir.path());
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["type"], "prompt");
    assert_eq!(sent[0]["id"], "1");
    assert_eq!(sent[0]["text"], "第一行\n第二行");
    assert!(
        dir.path().join("closed").exists(),
        "stdin closed: ccrpc disconnected"
    );
}

#[test]
fn the_folder_trust_dialog_is_refused_and_nothing_is_sent() {
    let (dir, env) = fake(
        r#"
echo '{"type":"ready","protocol":3,"holder_id":"H1","session_id":null,"cwd":null,"state":"starting"}'
echo '{"type":"choice","request_id":"choice:1","title":"t","options":[{"label":"Yes, I trust this folder","meaning":"allow","description":null},{"label":"No, exit","meaning":"deny","description":null}],"tool_use_id":null,"layout":"trust_folder","layout_source":"builtin"}'
IFS= read -r line; printf '%s\n' "$line" >> "$D/sent"
echo '{"type":"state","state":"exited"}'
echo '{"type":"session_exit"}'
echo '{"type":"response","id":"trust","command":"answer_choice","success":false,"error":{"code":"timeout","message":"m"}}'
IFS= read -r line; printf '%s\n' "$line" >> "$D/sent"
echo '{"type":"response","id":"close","command":"exit","success":true,"data":{}}'
cat > /dev/null
"#,
    );
    let err = start(&env, &NO_LAUNCH, &cwd(), "hello", Duration::from_secs(30)).unwrap_err();
    assert_eq!(err.code, "untrusted_dir");
    assert_eq!(err.message, "目录还没有信任，先在终端里用 claude 打开一次");
    assert_eq!(err.holder_id, None, "the Pane is closed");
    let sent = sent(dir.path());
    assert_eq!(sent.len(), 2, "the prompt is not sent: {sent:?}");
    assert_eq!(sent[0]["type"], "answer_choice");
    assert_eq!(sent[0]["request_id"], "choice:1");
    assert_eq!(sent[0]["option"], "No, exit");
    assert_eq!(sent[1]["type"], "exit");
    assert_eq!(sent[1]["close_pane"], true);
}

#[test]
fn another_choice_before_ready_is_left_for_a_person() {
    let (_dir, env) = fake(
        r#"
echo '{"type":"choice","request_id":"choice:1","title":"t","options":[],"layout":"other","layout_source":"builtin"}'
cat > /dev/null
"#,
    );
    let err = start(&env, &NO_LAUNCH, &cwd(), "hello", Duration::from_secs(30)).unwrap_err();
    assert_eq!(err.code, "choice_pending");
    assert_eq!(err.holder_id, None);
}

#[test]
fn an_early_exit_is_one_sentence_with_the_reason() {
    let (_dir, env) = fake(
        r#"
echo 'some earlier noise' >&2
echo 'ccrpc: 没有找到 toh' >&2
exit 3
"#,
    );
    let err = start(&env, &NO_LAUNCH, &cwd(), "hello", Duration::from_secs(30)).unwrap_err();
    assert_eq!(err.code, "ccrpc_failed");
    assert_eq!(err.message, "ccrpc 提前退出：没有找到 toh");
}

#[test]
fn a_silent_ccrpc_times_out() {
    let (_dir, env) = fake("exec sleep 3\n");
    let err = start(&env, &NO_LAUNCH, &cwd(), "hello", Duration::from_secs(1)).unwrap_err();
    assert_eq!(err.code, "timeout");
    assert_eq!(err.holder_id, None);
}

#[test]
fn a_failed_prompt_names_the_pane_left_behind() {
    let (_dir, env) = fake(
        r#"
echo '{"type":"ready","protocol":3,"holder_id":"H1","session_id":"S-1","cwd":"/w","state":"idle"}'
IFS= read -r line
echo '{"type":"response","id":"1","command":"prompt","success":false,"error":{"code":"not_idle","message":"当前状态为 busy"}}'
cat > /dev/null
"#,
    );
    let err = start(&env, &NO_LAUNCH, &cwd(), "hello", Duration::from_secs(30)).unwrap_err();
    assert_eq!(err.code, "prompt_failed");
    assert_eq!(err.holder_id.as_deref(), Some("H1"));
    assert!(err.message.contains("当前状态为 busy"));
}

/// Through the binary: the prompt comes from stdin and the result is one JSON document.
#[test]
fn the_command_reads_stdin_and_prints_one_json_document() {
    let (dir, env) = fake(STARTED);
    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_codeck"))
        .args(["new", "--cwd"])
        .arg(cwd())
        .env("HOME", home.path())
        .env("CODECK_CCRPC", &env["CODECK_CCRPC"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"  a prompt\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["sessionId"], "S-1");
    assert_eq!(v["holderId"], "H1");
    assert_eq!(sent(dir.path())[0]["text"], "a prompt");
}

#[test]
fn an_empty_prompt_or_missing_directory_is_refused_before_ccrpc_runs() {
    let home = tempfile::tempdir().unwrap();
    let (dir, env) = fake(STARTED);
    let run = |args: &[&str], stdin: &str| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_codeck"))
            .args(args)
            .env("HOME", home.path())
            .env("CODECK_CCRPC", &env["CODECK_CCRPC"])
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        c.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        c.wait_with_output().unwrap()
    };
    let cwd = cwd().display().to_string();
    let out = run(&["new", "--cwd", &cwd], "   \n");
    assert_eq!(out.status.code(), Some(1));
    let e: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(e["code"], "empty_prompt");
    for shell in ["!rm -rf x", "\u{1b}!rm -rf x"] {
        let out = run(&["new", "--cwd", &cwd], shell);
        let e: Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(e["code"], "shell_prompt");
    }
    let out = run(&["new", "--cwd", "/no/such/dir"], "x");
    let e: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(e["code"], "invalid_cwd");
    assert!(!dir.path().join("args").exists(), "ccrpc never started");
}
