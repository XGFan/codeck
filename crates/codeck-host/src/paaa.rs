//! Relaying `paaa ls --all --json --watch` to the desktop front end (one child process at a time,
//! started lazily on the first `paaa_watch(true, id)` call without initializing ccrpc).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use codeck_core::model::{AppError, ErrorCode};
use parking_lot::Mutex;

use crate::Host;
use crate::commands::{CmdResult, internal};
use crate::events::{EventSink, HostEvent};

const DEFAULT_KILL_AFTER: Duration = Duration::from_secs(2);

fn app_err(code: ErrorCode, message: impl Into<String>) -> AppError {
    AppError {
        code,
        message: message.into(),
    }
}

struct Watch {
    stdin: Option<ChildStdin>,
    child: Arc<Mutex<Child>>,
}

/// Manages at most one `paaa ls --all --json --watch` child process, started lazily on demand.
pub(crate) struct PaaaState {
    env: OnceLock<HashMap<String, String>>,
    slot: Mutex<Option<Watch>>,
    generation: Arc<AtomicU64>,
    kill_after: Duration,
}

impl PaaaState {
    pub(crate) fn new() -> Self {
        Self {
            env: OnceLock::new(),
            slot: Mutex::new(None),
            generation: Arc::new(AtomicU64::new(0)),
            kill_after: DEFAULT_KILL_AFTER,
        }
    }

    #[cfg(test)]
    fn with_env(env: HashMap<String, String>, kill_after: Duration) -> Self {
        let cell = OnceLock::new();
        let _ = cell.set(env);
        Self {
            env: cell,
            kill_after,
            ..Self::new()
        }
    }

    fn watch(&self, on: bool, id: u32, sink: EventSink) -> CmdResult<()> {
        // Before the lock: the login shell can take seconds, and `stop()` (the app quitting) waits for the lock.
        let env = on.then(|| self.env.get_or_init(crate::ccrpc::env::login_env));
        let mut slot = self.slot.lock();
        let this = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(old) = slot.take() {
            stop_watch(old, self.kill_after);
        }
        let Some(env) = env else {
            return Ok(());
        };
        let bin = crate::ccrpc::env::tool_path(env, "paaa");
        let mut cmd = Command::new(&bin);
        cmd.env_clear()
            .envs(env)
            .args(["ls", "--all", "--json", "--watch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = crate::ccrpc::relay::spawn(&mut cmd).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => app_err(ErrorCode::NotFound, "没有找到 paaa"),
            _ => app_err(ErrorCode::Io, format!("运行不了 {}：{e}", bin.display())),
        })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| app_err(ErrorCode::Io, "没有 stdout"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| app_err(ErrorCode::Io, "没有 stderr"))?;
        let stdin = child.stdin.take();
        let child = Arc::new(Mutex::new(child));
        let (generation, waiter) = (self.generation.clone(), child.clone());
        std::thread::spawn(move || {
            let current = || generation.load(Ordering::SeqCst) == this;
            let errs = std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = stderr.read_to_end(&mut buf);
                String::from_utf8_lossy(&buf).into_owned()
            });
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if !current() {
                    break;
                }
                sink(HostEvent::PaaaWorkers { id, workers: line });
            }
            let err = errs.join().unwrap_or_default();
            let code = waiter.lock().wait().ok().and_then(|s| s.code());
            if !current() {
                return;
            }
            tracing::warn!(
                "paaa ls --watch exited (id={id}, code={code:?}): {}",
                err.trim().replace('\n', " | ")
            );
            let first = err.lines().map(str::trim).find(|l| !l.is_empty());
            let message = first.map_or_else(
                || "paaa ls 意外退出".to_string(),
                |l| l.trim_start_matches("paaa: ").to_string(),
            );
            sink(HostEvent::PaaaWatchEnded { id, message });
        });
        *slot = Some(Watch { stdin, child });
        Ok(())
    }

    /// Stops any active watch during `Host::shutdown()` by closing stdin without waiting.
    pub(crate) fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(mut old) = self.slot.lock().take() {
            drop(old.stdin.take());
        }
    }
}

fn stop_watch(mut w: Watch, kill_after: Duration) {
    drop(w.stdin.take());
    let child = w.child;
    std::thread::spawn(move || {
        std::thread::sleep(kill_after);
        let mut c = child.lock();
        if matches!(c.try_wait(), Ok(None)) {
            let _ = c.kill();
        }
    });
}

impl Host {
    /// Starts (`on = true`) a `paaa ls --all --json --watch` tagged with `id` (replacing any
    /// existing watch), or stops the existing watch (`on = false`; `id` is unused).
    pub async fn paaa_watch(&self, on: bool, id: u32) -> CmdResult<()> {
        let host = self.me.upgrade().ok_or_else(|| internal("host 已释放"))?;
        tokio::task::spawn_blocking(move || host.paaa.watch(on, id, host.events.clone()))
            .await
            .map_err(internal)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::time::Instant;

    fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn state_for_script(dir: &Path, body: &str, kill_after: Duration) -> PaaaState {
        let script = write_script(dir, "paaa", body);
        PaaaState::with_env(
            HashMap::from([
                ("CODECK_PAAA".into(), script.display().to_string()),
                ("PATH".into(), "/usr/bin:/bin".into()),
            ]),
            kill_after,
        )
    }

    fn event_sink() -> (EventSink, mpsc::Receiver<HostEvent>) {
        let (tx, rx) = mpsc::channel();
        (Arc::new(move |e| _ = tx.send(e)), rx)
    }

    fn expect_workers(rx: &mpsc::Receiver<HostEvent>, want_id: u32, want: &str) {
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            HostEvent::PaaaWorkers { id, workers } => {
                assert_eq!((id, workers.as_str()), (want_id, want));
            }
            other => panic!("expected PaaaWorkers, got {other:?}"),
        }
    }

    fn expect_ended(rx: &mpsc::Receiver<HostEvent>, want_id: u32, want: &str) {
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            HostEvent::PaaaWatchEnded { id, message } => {
                assert_eq!((id, message.as_str()), (want_id, want));
            }
            other => panic!("expected PaaaWatchEnded, got {other:?}"),
        }
    }

    fn pid_alive(pid: i32) -> bool {
        // SAFETY: signal 0 only checks process existence.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    fn wait_pid_dead(pid: i32, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline && pid_alive(pid) {
            std::thread::sleep(Duration::from_millis(20));
        }
        !pid_alive(pid)
    }

    fn read_pid(path: &Path) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok(s) = std::fs::read_to_string(path)
                && let Ok(pid) = s.trim().parse()
            {
                return pid;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out reading pid from {}", path.display());
    }

    #[test]
    fn relays_lines_and_stops_cleanly_without_ended() {
        let d = tempfile::tempdir().unwrap();
        let pidfile = d.path().join("child.pid");
        let state = state_for_script(
            d.path(),
            &format!(
                r#"[ "$*" = "ls --all --json --watch" ] || exit 2
echo $$ > {}
echo '[]'
echo '[{{"id":"u1","name":"w1"}}]'
read _ || {{ echo 'paaa: stopped' >&2; exit 1; }}"#,
                pidfile.display()
            ),
            DEFAULT_KILL_AFTER,
        );
        let (sink, rx) = event_sink();
        state.watch(true, 7, sink.clone()).unwrap();
        expect_workers(&rx, 7, "[]");
        expect_workers(&rx, 7, r#"[{"id":"u1","name":"w1"}]"#);

        let pid = read_pid(&pidfile);
        state.watch(false, 0, sink.clone()).unwrap();
        assert!(wait_pid_dead(pid, Duration::from_secs(2)));
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(300)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
    }

    #[test]
    fn emits_ended_with_stderr_or_default_message() {
        let d = tempfile::tempdir().unwrap();
        let state = state_for_script(
            d.path(),
            "echo '[]'\nprintf '\\n  paaa: 旧版不支持 --watch\\nsecond\\n' >&2\nexit 1",
            Duration::from_millis(200),
        );
        let (sink, rx) = event_sink();
        state.watch(true, 9, sink).unwrap();
        expect_workers(&rx, 9, "[]");
        expect_ended(&rx, 9, "旧版不支持 --watch");

        let d2 = tempfile::tempdir().unwrap();
        let state2 = state_for_script(d2.path(), "exit 1", Duration::from_millis(200));
        let (sink2, rx2) = event_sink();
        state2.watch(true, 10, sink2).unwrap();
        expect_ended(&rx2, 10, "paaa ls 意外退出");
    }

    #[test]
    fn replacing_watch_switches_id_and_suppresses_old_ended() {
        let d = tempfile::tempdir().unwrap();
        let state = state_for_script(
            d.path(),
            "echo '[]'\nread _ || { echo 'paaa: old exited' >&2; exit 1; }",
            Duration::from_millis(200),
        );
        let (sink, rx) = event_sink();
        state.watch(true, 1, sink.clone()).unwrap();
        expect_workers(&rx, 1, "[]");

        state.watch(true, 2, sink.clone()).unwrap();
        expect_workers(&rx, 2, "[]");
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(300)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        state.stop();
    }

    #[test]
    fn kills_and_reaps_child_ignoring_stdin_eof() {
        let d = tempfile::tempdir().unwrap();
        let pidfile = d.path().join("stuck.pid");
        let state = state_for_script(
            d.path(),
            &format!("echo $$ > {}\necho '[]'\nexec sleep 30", pidfile.display()),
            Duration::from_millis(150),
        );
        let (sink, rx) = event_sink();
        state.watch(true, 3, sink.clone()).unwrap();
        expect_workers(&rx, 3, "[]");

        let pid = read_pid(&pidfile);
        assert!(pid_alive(pid));
        state.watch(false, 0, sink.clone()).unwrap();
        assert!(wait_pid_dead(pid, Duration::from_secs(2)));
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(200)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
    }

    #[test]
    fn a_missing_paaa_is_not_found() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().display().to_string();
        let env = HashMap::from([("HOME".into(), dir.clone()), ("PATH".into(), dir)]);
        let (sink, _rx) = event_sink();
        let err = PaaaState::with_env(env, DEFAULT_KILL_AFTER)
            .watch(true, 1, sink)
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
        assert_eq!(err.message, "没有找到 paaa");
    }
}
