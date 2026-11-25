//! Reads the logged-in account's subscription usage via headless `claude` (`get_usage` over the
//! stream-json control channel), without sending any prompt.
//!
//! Single-flight with a 60-second monotonic cache (both `Some` and `None` cached; errors are never
//! cached). The child runs in its own process group inside the Host's private `<cache>/plan-usage`
//! working directory, which `Engine` excludes from Live Session detection.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use codeck_core::model::{AppError, ErrorCode, PlanLimit, PlanLimitKind, PlanUsage};
use parking_lot::{Condvar, Mutex};
use serde_json::Value;

use crate::Host;
use crate::commands::{CmdResult, internal};

/// Maximum wait from `spawn` until the matching `control_response` arrives.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
/// Grace period after closing stdin before killing the probe's process group.
const EXIT_GRACE: Duration = Duration::from_secs(2);
/// How long a successful probe result (`Some` or `None`) is reused when `refresh == false`.
const CACHE_TTL: Duration = Duration::from_secs(60);
/// Per-line cap when reading probe stdout so an oversized noise line cannot blow up memory.
const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
/// Maximum number of recognized rate-limit items kept from a single `get_usage` response.
const MAX_LIMITS: usize = 16;
/// Maximum character length of a model display name (and logged non-success subtype).
const MAX_NAME_CHARS: usize = 64;
/// Brief drain window after the leader exits so buffered stdout bytes can still be read.
const LEADER_EXIT_DRAIN: Duration = Duration::from_millis(100);

const CLAUDE_ARGS: &[&str] = &[
    "-p",
    "--input-format",
    "stream-json",
    "--output-format",
    "stream-json",
    "--verbose",
    "--no-session-persistence",
    "--strict-mcp-config",
    "--mcp-config",
    r#"{"mcpServers":{}}"#,
    "--settings",
    r#"{"disableAllHooks":true}"#,
];

fn app_err(code: ErrorCode, message: impl Into<String>) -> AppError {
    AppError {
        code,
        message: message.into(),
    }
}

fn no_usage_err() -> AppError {
    app_err(ErrorCode::Internal, "claude 没有返回额度")
}

fn timeout_err() -> AppError {
    app_err(ErrorCode::Io, "读取订阅额度超时")
}

fn dir_err() -> AppError {
    app_err(ErrorCode::Io, "无法准备读取订阅额度所需的目录")
}

fn shutdown_err() -> AppError {
    app_err(ErrorCode::Cancelled, "应用正在退出")
}

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as f64
}

fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Creates `<cache_dir>/plan-usage` and returns its canonical (realpath) path so `Engine` and the
/// probe always agree on the exact directory to exclude from Live Session detection.
pub(crate) fn prepare_dir(cache_dir: &Path) -> std::io::Result<PathBuf> {
    let dir = codeck_core::paths::plan_usage_dir(cache_dir);
    std::fs::create_dir_all(&dir)?;
    dir.canonicalize()
}

enum EnvSource {
    Login(OnceLock<HashMap<String, String>>),
    #[cfg(test)]
    Fixed(HashMap<String, String>),
}

impl EnvSource {
    fn get(&self) -> &HashMap<String, String> {
        match self {
            EnvSource::Login(cell) => cell.get_or_init(crate::ccrpc::env::login_env),
            #[cfg(test)]
            EnvSource::Fixed(map) => map,
        }
    }
}

struct Cached {
    value: Option<PlanUsage>,
    at: Instant,
}

struct InFlight {
    done: Mutex<Option<CmdResult<Option<PlanUsage>>>>,
    ready: Condvar,
}

impl InFlight {
    fn new() -> Self {
        Self {
            done: Mutex::new(None),
            ready: Condvar::new(),
        }
    }

    fn wait(&self) -> CmdResult<Option<PlanUsage>> {
        let mut guard = self.done.lock();
        while guard.is_none() {
            self.ready.wait(&mut guard);
        }
        guard.clone().expect("in-flight result was set")
    }

    fn finish(&self, result: CmdResult<Option<PlanUsage>>) {
        let mut guard = self.done.lock();
        *guard = Some(result);
        self.ready.notify_all();
    }
}

/// One probe process group. The leader `Child` stays inside `Option` until [`finish_child`]
/// sends `SIGKILL` to the whole process group and reaps the leader, ensuring the PGID is never
/// recycled before `killpg` runs and descendants holding stdout are always cleaned up.
struct ProbeChild {
    pgid: libc::pid_t,
    child: Mutex<Option<Child>>,
    finished: Mutex<bool>,
    done_cv: Condvar,
}

impl ProbeChild {
    fn new(child: Child) -> Arc<Self> {
        let pgid = child.id() as libc::pid_t;
        Arc::new(Self {
            pgid,
            child: Mutex::new(Some(child)),
            finished: Mutex::new(false),
            done_cv: Condvar::new(),
        })
    }

    fn wait_finished(&self) {
        let mut done = self.finished.lock();
        while !*done {
            self.done_cv.wait(&mut done);
        }
    }
}

struct State {
    cache: Option<Cached>,
    in_flight: Option<Arc<InFlight>>,
    /// The most recently spawned probe; a subsequent probe waits for its cleanup to complete
    /// before spawning a new child so two probes never overlap.
    last_probe: Option<Arc<ProbeChild>>,
    shutting_down: bool,
    children: Vec<Arc<ProbeChild>>,
}

/// Single-flight subscription usage probe with monotonic caching and clean shutdown.
pub(crate) struct PlanUsageState {
    /// Canonical probe working directory already registered with `Engine`; `None` if it could not
    /// be created/canonicalized at startup (in which case the probe refuses to start).
    probe_cwd: Option<PathBuf>,
    env: EnvSource,
    bin_override: Option<PathBuf>,
    timeout: Duration,
    exit_grace: Duration,
    cache_ttl: Duration,
    seq: AtomicU64,
    state: Arc<Mutex<State>>,
}

impl PlanUsageState {
    pub(crate) fn new(probe_cwd: Option<PathBuf>) -> Self {
        Self {
            probe_cwd,
            env: EnvSource::Login(OnceLock::new()),
            bin_override: None,
            timeout: PROBE_TIMEOUT,
            exit_grace: EXIT_GRACE,
            cache_ttl: CACHE_TTL,
            seq: AtomicU64::new(0),
            state: Arc::new(Mutex::new(State {
                cache: None,
                in_flight: None,
                last_probe: None,
                shutting_down: false,
                children: Vec::new(),
            })),
        }
    }

    #[cfg(test)]
    fn for_test(
        probe_cwd: Option<PathBuf>,
        bin: PathBuf,
        env: HashMap<String, String>,
        timeout: Duration,
        exit_grace: Duration,
        cache_ttl: Duration,
    ) -> Self {
        Self {
            probe_cwd,
            env: EnvSource::Fixed(env),
            bin_override: Some(bin),
            timeout,
            exit_grace,
            cache_ttl,
            seq: AtomicU64::new(0),
            state: Arc::new(Mutex::new(State {
                cache: None,
                in_flight: None,
                last_probe: None,
                shutting_down: false,
                children: Vec::new(),
            })),
        }
    }

    /// Returns subscription usage, joining any in-flight probe, reusing a fresh cache when
    /// `!refresh`, or starting a new probe (waiting for any previous probe's cleanup first).
    pub(crate) fn get(&self, refresh: bool) -> CmdResult<Option<PlanUsage>> {
        let (flight, prev_probe) = {
            let mut s = self.state.lock();
            if let Some(flight) = s.in_flight.clone() {
                drop(s);
                return flight.wait();
            }
            if s.shutting_down {
                return Err(shutdown_err());
            }
            if !refresh
                && let Some(cached) = &s.cache
                && cached.at.elapsed() < self.cache_ttl
            {
                return Ok(cached.value.clone());
            }
            let flight = Arc::new(InFlight::new());
            s.in_flight = Some(flight.clone());
            (flight, s.last_probe.clone())
        };

        let mut guard = FlightGuard {
            owner: self,
            flight: flight.clone(),
            finished: false,
        };
        let result = self.run_probe(prev_probe);
        guard.complete(result.clone());
        result
    }

    fn finish_flight(&self, flight: &InFlight, result: CmdResult<Option<PlanUsage>>) {
        {
            let mut s = self.state.lock();
            s.in_flight = None;
            if let Ok(ref value) = result
                && !s.shutting_down
            {
                s.cache = Some(Cached {
                    value: value.clone(),
                    at: Instant::now(),
                });
            }
        }
        flight.finish(result);
    }

    /// Prevents future probes from starting, then kills and reaps every tracked probe process
    /// group before returning.
    pub(crate) fn shutdown(&self) {
        let children: Vec<Arc<ProbeChild>> = {
            let mut s = self.state.lock();
            s.shutting_down = true;
            s.children.clone()
        };
        for probe in children {
            finish_child(&probe, &self.state, Duration::ZERO);
        }
    }

    fn is_shutting_down(&self) -> bool {
        self.state.lock().shutting_down
    }

    fn ensure_probe_cwd(&self) -> CmdResult<&PathBuf> {
        let Some(expected) = &self.probe_cwd else {
            return Err(dir_err());
        };
        std::fs::create_dir_all(expected).map_err(|_| dir_err())?;
        let actual = expected.canonicalize().map_err(|_| dir_err())?;
        if &actual != expected {
            return Err(dir_err());
        }
        Ok(expected)
    }

    fn run_probe(&self, prev_probe: Option<Arc<ProbeChild>>) -> CmdResult<Option<PlanUsage>> {
        // Wait for the previous probe's process group cleanup to finish before spawning a new one.
        if let Some(prev) = prev_probe {
            prev.wait_finished();
        }
        if self.is_shutting_down() {
            return Err(shutdown_err());
        }
        // Verify the private directory exists and still resolves to the canonical path Engine holds
        // before spawning, so a probe process can never leak into Live Sessions.
        let cwd = self.ensure_probe_cwd()?;

        // Resolve login environment lazily on the first call; its time does not count against the
        // 15-second probe timeout.
        let env = self.env.get();
        if self.is_shutting_down() {
            return Err(shutdown_err());
        }
        let bin = self
            .bin_override
            .clone()
            .unwrap_or_else(|| crate::ccrpc::env::tool_path(env, "claude"));

        let mut cmd = Command::new(&bin);
        cmd.env_clear()
            .envs(env)
            .current_dir(cwd)
            .args(CLAUDE_ARGS)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0);

        let probe = self.spawn_child(cmd)?;
        let deadline = Instant::now() + self.timeout;

        let (mut stdin, stdout) = {
            let mut guard = probe.child.lock();
            match guard.as_mut() {
                Some(c) => (c.stdin.take(), c.stdout.take()),
                None => (None, None),
            }
        };
        let Some(stdout) = stdout else {
            drop(stdin.take());
            self.spawn_finish(probe, Duration::ZERO);
            return if self.is_shutting_down() {
                Err(shutdown_err())
            } else {
                Err(app_err(ErrorCode::Io, "没有 stdout"))
            };
        };

        let req_id = format!("r{}", self.seq.fetch_add(1, Ordering::Relaxed) + 1);
        let req_line = format!(
            r#"{{"type":"control_request","request_id":"{req_id}","request":{{"subtype":"get_usage","skip_behaviors":true}}}}"#
        );
        if let Some(w) = stdin.as_mut() {
            let _ = w.write_all(req_line.as_bytes());
            let _ = w.write_all(b"\n");
            let _ = w.flush();
        }

        let (tx, rx) = mpsc::sync_channel::<Result<Value, ()>>(1);
        let expected_id = req_id.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_bounded_line(&mut reader, MAX_LINE_BYTES) {
                    Ok(LineRead::Line(bytes)) => {
                        let Ok(v) = serde_json::from_slice::<Value>(&bytes) else {
                            continue;
                        };
                        if v.get("type").and_then(Value::as_str) != Some("control_response") {
                            continue;
                        }
                        if v.pointer("/response/request_id").and_then(Value::as_str)
                            != Some(expected_id.as_str())
                        {
                            continue;
                        }
                        let _ = tx.send(Ok(v));
                        return;
                    }
                    Ok(LineRead::TooLong) => continue,
                    Ok(LineRead::Eof) | Err(_) => {
                        let _ = tx.send(Err(()));
                        return;
                    }
                }
            }
        });

        let mut leader_dead_since: Option<Instant> = None;
        let outcome = loop {
            let now = Instant::now();
            if now >= deadline {
                break RecvOutcome::Timeout;
            }
            let step = deadline
                .saturating_duration_since(now)
                .min(Duration::from_millis(50));
            match rx.recv_timeout(step) {
                Ok(Ok(v)) => break RecvOutcome::Response(v),
                Ok(Err(())) | Err(mpsc::RecvTimeoutError::Disconnected) => break RecvOutcome::Eof,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if self.is_shutting_down() {
                        break RecvOutcome::Eof;
                    }
                    if leader_exited_unreaped_locked(&probe) {
                        let since = *leader_dead_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= LEADER_EXIT_DRAIN {
                            break RecvOutcome::Eof;
                        }
                    }
                }
            }
        };

        // Close stdin and run the unified cleanup off the calling thread so the command returns
        // immediately without waiting for the grace period / killpg / reap.
        drop(stdin.take());
        match outcome {
            RecvOutcome::Response(v) => {
                self.spawn_finish(probe, self.exit_grace);
                parse_control_response(&v, now_ms())
            }
            RecvOutcome::Eof => {
                self.spawn_finish(probe, Duration::ZERO);
                if self.is_shutting_down() {
                    Err(shutdown_err())
                } else {
                    Err(no_usage_err())
                }
            }
            RecvOutcome::Timeout => {
                self.spawn_finish(probe, Duration::ZERO);
                if self.is_shutting_down() {
                    Err(shutdown_err())
                } else {
                    Err(timeout_err())
                }
            }
        }
    }

    /// Serializes shutdown-check, `spawn`, and child registration under `self.state` so
    /// [`shutdown`] can never race with spawning a new child.
    fn spawn_child(&self, mut cmd: Command) -> CmdResult<Arc<ProbeChild>> {
        let mut s = self.state.lock();
        if s.shutting_down {
            return Err(shutdown_err());
        }
        let child = crate::ccrpc::relay::spawn(&mut cmd).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => app_err(ErrorCode::NotFound, "未找到 claude"),
            _ => app_err(ErrorCode::Io, format!("运行不了 claude：{e}")),
        })?;
        let probe = ProbeChild::new(child);
        s.children.push(probe.clone());
        s.last_probe = Some(probe.clone());
        Ok(probe)
    }

    fn spawn_finish(&self, probe: Arc<ProbeChild>, grace: Duration) {
        let state = self.state.clone();
        std::thread::spawn(move || finish_child(&probe, &state, grace));
    }
}

enum RecvOutcome {
    Response(Value),
    Eof,
    Timeout,
}

struct FlightGuard<'a> {
    owner: &'a PlanUsageState,
    flight: Arc<InFlight>,
    finished: bool,
}

impl FlightGuard<'_> {
    fn complete(&mut self, result: CmdResult<Option<PlanUsage>>) {
        self.finished = true;
        self.owner.finish_flight(&self.flight, result);
    }
}

impl Drop for FlightGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.owner.finish_flight(&self.flight, Err(no_usage_err()));
        }
    }
}

/// Checks whether `probe`'s group leader has exited (without reaping it) while holding `probe.child`.
fn leader_exited_unreaped_locked(probe: &ProbeChild) -> bool {
    let guard = probe.child.lock();
    match guard.as_ref() {
        Some(_) => leader_exited_unreaped(probe.pgid),
        None => true,
    }
}

/// Non-reaping check (`waitid` with `WEXITED | WNOHANG | WNOWAIT`) so the leader's PID/PGID stays
/// reserved until [`finish_child`] signals the process group and calls `wait()`.
fn leader_exited_unreaped(pid: libc::pid_t) -> bool {
    // SAFETY: `siginfo_t` is plain old data; `waitid` with `WNOHANG | WNOWAIT` only inspects our
    // unreaped child without reaping it.
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        let rc = libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        );
        rc == 0 && info.si_pid() == pid
    }
}

/// Unified probe process-group cleanup shared by grace-period exit, timeout, and shutdown.
///
/// Waits up to `grace` for the leader to exit without reaping it (waking early on shutdown), then
/// always sends `SIGKILL` to the entire process group BEFORE reaping the leader (killing any
/// descendants holding stdout even when the leader already exited), reaps the leader, removes the
/// entry from `state.children`, and notifies waiters on `probe.done_cv`.
fn finish_child(probe: &Arc<ProbeChild>, state: &Arc<Mutex<State>>, grace: Duration) {
    if !grace.is_zero() {
        let deadline = Instant::now() + grace;
        loop {
            if state.lock().shutting_down
                || leader_exited_unreaped_locked(probe)
                || Instant::now() >= deadline
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    {
        let mut guard = probe.child.lock();
        if let Some(mut c) = guard.take() {
            // SAFETY: `probe.pgid` is our unreaped child's PID/PGID (alive or zombie) until
            // `c.wait()` below reaps it, so its PGID cannot have been recycled.
            unsafe { libc::killpg(probe.pgid, libc::SIGKILL) };
            let _ = c.wait();
        }
    }
    state.lock().children.retain(|c| !Arc::ptr_eq(c, probe));
    {
        let mut done = probe.finished.lock();
        *done = true;
        probe.done_cv.notify_all();
    }
}

enum LineRead {
    Line(Vec<u8>),
    TooLong,
    Eof,
}

fn read_bounded_line(reader: &mut impl BufRead, max_bytes: usize) -> std::io::Result<LineRead> {
    let mut out = Vec::new();
    let mut too_long = false;
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return Ok(if too_long {
                LineRead::TooLong
            } else if out.is_empty() {
                LineRead::Eof
            } else {
                LineRead::Line(out)
            });
        }
        if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            if !too_long && out.len() + pos <= max_bytes {
                out.extend_from_slice(&buf[..pos]);
            } else {
                too_long = true;
            }
            reader.consume(pos + 1);
            return Ok(if too_long {
                LineRead::TooLong
            } else {
                LineRead::Line(out)
            });
        }
        if !too_long && out.len() + buf.len() <= max_bytes {
            out.extend_from_slice(buf);
        } else {
            too_long = true;
        }
        let n = buf.len();
        reader.consume(n);
    }
}

/// Tolerantly parses Claude Code's `control_response` for `get_usage`.
pub(crate) fn parse_control_response(v: &Value, fetched_ms: f64) -> CmdResult<Option<PlanUsage>> {
    let resp = &v["response"];
    let subtype = resp.get("subtype").and_then(Value::as_str);
    if subtype != Some("success") {
        let shown = subtype.map_or("<missing>", |s| truncate_chars(s, MAX_NAME_CHARS));
        tracing::warn!("get_usage control_response subtype was not success: {shown}");
        return Err(no_usage_err());
    }
    let body = &resp["response"];
    if body.get("rate_limits_available").and_then(Value::as_bool) == Some(false) {
        return Ok(None);
    }
    let rl = &body["rate_limits"];
    let mut limits = Vec::new();
    let raw_limits = rl.get("limits").and_then(Value::as_array);
    if let Some(arr) = raw_limits.filter(|a| !a.is_empty()) {
        for item in arr {
            if limits.len() >= MAX_LIMITS {
                break;
            }
            let kind = match item.get("kind").and_then(Value::as_str) {
                Some("session") => PlanLimitKind::Session,
                Some("weekly_all") => PlanLimitKind::WeeklyAll,
                Some("weekly_scoped") => PlanLimitKind::WeeklyScoped,
                _ => continue,
            };
            let Some(percent) = finite_non_neg(item.get("percent")) else {
                continue;
            };
            let model = item
                .pointer("/scope/model/display_name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| truncate_chars(s, MAX_NAME_CHARS).to_string());
            limits.push(PlanLimit {
                kind,
                model,
                percent,
                resets_ms: parse_resets_at(item.get("resets_at")),
            });
        }
    } else {
        for (key, kind) in [
            ("five_hour", PlanLimitKind::Session),
            ("seven_day", PlanLimitKind::WeeklyAll),
        ] {
            let Some(bucket) = rl.get(key) else {
                continue;
            };
            let Some(percent) = finite_non_neg(bucket.get("utilization")) else {
                continue;
            };
            limits.push(PlanLimit {
                kind,
                model: None,
                percent,
                resets_ms: parse_resets_at(bucket.get("resets_at")),
            });
        }
    }

    if limits.is_empty() {
        return Err(no_usage_err());
    }

    let plan = body
        .get("subscription_type")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Ok(Some(PlanUsage {
        plan,
        fetched_ms,
        limits,
    }))
}

fn finite_non_neg(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .map(|n| n.max(0.0))
}

fn parse_resets_at(v: Option<&Value>) -> Option<f64> {
    let s = v.and_then(Value::as_str)?;
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis() as f64)
}

impl Host {
    /// Reads the logged-in account's subscription usage via headless `claude` (`get_usage`);
    /// `None` when the account has no subscription rate limits (e.g. API key).
    pub async fn get_plan_usage(&self, refresh: bool) -> CmdResult<Option<PlanUsage>> {
        let host = self.me.upgrade().ok_or_else(|| internal("host 已释放"))?;
        tokio::task::spawn_blocking(move || host.plan_usage.get(refresh))
            .await
            .map_err(internal)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Barrier;

    const FIXTURE: &str = include_str!("../tests/fixtures/get_usage.response.json");

    fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn base_env() -> HashMap<String, String> {
        HashMap::from([("PATH".into(), "/usr/bin:/bin".into())])
    }

    fn make_probe_dir(dir: &Path) -> PathBuf {
        let p = dir.join("plan-usage");
        std::fs::create_dir_all(&p).unwrap();
        p.canonicalize().unwrap()
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

    /// Shell snippet that reads the `control_request` line and echoes its `request_id` into a
    /// compact `control_response` JSON built from `body_json`.
    fn reply_with_body(body_json: &str) -> String {
        format!(
            r#"IFS= read -r req_line
req_id=${{req_line#*\"request_id\":\"}}
req_id=${{req_id%%\"*}}
printf '{{"type":"control_response","response":{{"subtype":"success","request_id":"%s","response":{body_json}}}}}\n' "$req_id""#
        )
    }

    const SAMPLE_BODY: &str = r#"{"subscription_type":"max","rate_limits_available":true,"rate_limits":{"limits":[{"kind":"session","percent":25,"resets_at":"2026-11-25T08:40:00+00:00","scope":null,"is_active":true}]}}"#;

    // ------------------------------------------------------------ parsing tests

    #[test]
    fn parses_real_fixture() {
        let v: Value = serde_json::from_str(FIXTURE).unwrap();
        let usage = parse_control_response(&v, 1_700_000_000_000.0)
            .unwrap()
            .expect("fixture has usage");
        assert_eq!(usage.plan.as_deref(), Some("max"));
        assert_eq!(usage.fetched_ms, 1_700_000_000_000.0);
        assert_eq!(usage.limits.len(), 3);

        assert_eq!(usage.limits[0].kind, PlanLimitKind::Session);
        assert_eq!(usage.limits[0].model, None);
        assert_eq!(usage.limits[0].percent, 58.0);
        assert_eq!(
            usage.limits[0].resets_ms,
            Some(
                chrono::DateTime::parse_from_rfc3339("2026-11-25T08:40:00.238455+00:00")
                    .unwrap()
                    .timestamp_millis() as f64
            )
        );

        assert_eq!(usage.limits[1].kind, PlanLimitKind::WeeklyAll);
        assert_eq!(usage.limits[1].model, None);
        assert_eq!(usage.limits[1].percent, 36.0);
        assert_eq!(
            usage.limits[1].resets_ms,
            Some(
                chrono::DateTime::parse_from_rfc3339("2026-11-30T06:00:00.238473+00:00")
                    .unwrap()
                    .timestamp_millis() as f64
            )
        );

        assert_eq!(usage.limits[2].kind, PlanLimitKind::WeeklyScoped);
        assert_eq!(usage.limits[2].model.as_deref(), Some("Fable"));
        assert_eq!(usage.limits[2].percent, 55.0);
        assert_eq!(
            usage.limits[2].resets_ms,
            Some(
                chrono::DateTime::parse_from_rfc3339("2026-11-30T06:00:00.238607+00:00")
                    .unwrap()
                    .timestamp_millis() as f64
            )
        );
    }

    #[test]
    fn rate_limits_unavailable_returns_none() {
        let v = json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": "r1",
                "response": {
                    "subscription_type": null,
                    "rate_limits_available": false,
                    "rate_limits": {}
                }
            }
        });
        assert_eq!(parse_control_response(&v, 1.0).unwrap(), None);
    }

    #[test]
    fn falls_back_to_legacy_fields_when_limits_missing_or_empty() {
        for limits_field in [Value::Null, json!([])] {
            let v = json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": "r1",
                    "response": {
                        "subscription_type": "pro",
                        "rate_limits_available": true,
                        "rate_limits": {
                            "limits": limits_field,
                            "five_hour": {
                                "utilization": 42,
                                "resets_at": "2026-11-25T10:00:00Z"
                            },
                            "seven_day": {
                                "utilization": -5,
                                "resets_at": "2026-11-30T06:00:00Z"
                            }
                        }
                    }
                }
            });
            let usage = parse_control_response(&v, 2.0).unwrap().unwrap();
            assert_eq!(usage.plan.as_deref(), Some("pro"));
            assert_eq!(usage.limits.len(), 2);
            assert_eq!(usage.limits[0].kind, PlanLimitKind::Session);
            assert_eq!(usage.limits[0].percent, 42.0);
            assert!(usage.limits[0].resets_ms.is_some());
            assert_eq!(usage.limits[1].kind, PlanLimitKind::WeeklyAll);
            assert_eq!(usage.limits[1].percent, 0.0);
            assert!(usage.limits[1].resets_ms.is_some());
        }
    }

    #[test]
    fn skips_unknown_kind_and_handles_bad_resets_at() {
        let v = json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": "r1",
                "response": {
                    "subscription_type": "max",
                    "rate_limits_available": true,
                    "rate_limits": {
                        "limits": [
                            { "kind": "future_bucket", "percent": 10, "resets_at": "2026-11-25T10:00:00Z" },
                            { "kind": "session", "percent": -3.5, "resets_at": "not-a-timestamp", "is_active": false }
                        ]
                    }
                }
            }
        });
        let usage = parse_control_response(&v, 3.0).unwrap().unwrap();
        assert_eq!(usage.limits.len(), 1);
        assert_eq!(usage.limits[0].kind, PlanLimitKind::Session);
        assert_eq!(usage.limits[0].percent, 0.0);
        assert_eq!(usage.limits[0].resets_ms, None);
    }

    #[test]
    fn error_subtype_and_empty_limits_fail() {
        let err_subtype = json!({
            "type": "control_response",
            "response": {
                "subtype": "error",
                "request_id": "r1",
                "error": "not logged in"
            }
        });
        let e = parse_control_response(&err_subtype, 4.0).unwrap_err();
        assert_eq!(e.message, "claude 没有返回额度");

        let empty_limits = json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": "r1",
                "response": {
                    "subscription_type": "max",
                    "rate_limits_available": true,
                    "rate_limits": { "limits": [] }
                }
            }
        });
        let e = parse_control_response(&empty_limits, 5.0).unwrap_err();
        assert_eq!(e.message, "claude 没有返回额度");
    }

    #[test]
    fn caps_recognized_limits_at_16_and_truncates_model_name_at_64_chars() {
        let long_model = format!("模型-{}", "a".repeat(80));
        let mut raw = Vec::new();
        // Mix in unknown kinds before and between recognized items.
        raw.push(json!({ "kind": "unknown_kind", "percent": 1 }));
        for i in 0..20 {
            raw.push(json!({
                "kind": "weekly_scoped",
                "percent": i,
                "scope": { "model": { "display_name": if i == 0 { long_model.clone() } else { format!("M{i}") } } }
            }));
        }
        let v = json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": "r1",
                "response": {
                    "subscription_type": "max",
                    "rate_limits_available": true,
                    "rate_limits": { "limits": raw }
                }
            }
        });
        let usage = parse_control_response(&v, 6.0).unwrap().unwrap();
        assert_eq!(usage.limits.len(), MAX_LIMITS);
        assert_eq!(usage.limits[15].percent, 15.0);
        let first_model = usage.limits[0].model.as_deref().unwrap();
        assert_eq!(first_model.chars().count(), MAX_NAME_CHARS);
        assert!(long_model.starts_with(first_model));
    }

    // ------------------------------------------------------------ probe tests

    #[test]
    fn probe_skips_noise_lines_checks_args_and_cwd() {
        let d = tempfile::tempdir().unwrap();
        let probe_cwd = make_probe_dir(d.path());
        let recorded = d.path().join("recorded.txt");
        let bin = write_script(
            d.path(),
            "claude",
            &format!(
                r#"pwd > {rec}
printf '%s\n' "$@" >> {rec}
echo 'not json at all'
echo '{{"type":"system","subtype":"init"}}'
echo '{{"type":"control_response","response":{{"subtype":"success","request_id":"wrong","response":{{}}}}}}'
{reply}"#,
                rec = recorded.display(),
                reply = reply_with_body(SAMPLE_BODY),
            ),
        );
        let state = PlanUsageState::for_test(
            Some(probe_cwd.clone()),
            bin,
            base_env(),
            Duration::from_secs(5),
            Duration::from_millis(200),
            CACHE_TTL,
        );
        let usage = state.get(false).unwrap().expect("got usage");
        assert_eq!(usage.plan.as_deref(), Some("max"));
        assert_eq!(usage.limits.len(), 1);
        assert_eq!(usage.limits[0].percent, 25.0);

        let lines: Vec<String> = std::fs::read_to_string(&recorded)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        assert_eq!(PathBuf::from(&lines[0]), probe_cwd);
        assert_eq!(
            &lines[1..],
            CLAUDE_ARGS
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>()
                .as_slice()
        );
    }

    #[test]
    fn probe_timeout_kills_process_group_and_returns_error() {
        let d = tempfile::tempdir().unwrap();
        let probe_cwd = make_probe_dir(d.path());
        let pidfile = d.path().join("child.pid");
        let gpidfile = d.path().join("grand.pid");
        let bin = write_script(
            d.path(),
            "claude",
            &format!(
                r#"echo $$ > {}
sleep 30 &
echo $! > {}
wait"#,
                pidfile.display(),
                gpidfile.display(),
            ),
        );
        let state = Arc::new(PlanUsageState::for_test(
            Some(probe_cwd),
            bin,
            base_env(),
            Duration::from_millis(800),
            Duration::from_millis(100),
            CACHE_TTL,
        ));
        let st = state.clone();
        let handle = std::thread::spawn(move || st.get(false));
        let pid = read_pid(&pidfile);
        let gpid = read_pid(&gpidfile);
        let err = handle.join().unwrap().unwrap_err();
        assert_eq!(err.message, "读取订阅额度超时");
        assert!(wait_pid_dead(pid, Duration::from_secs(2)));
        assert!(wait_pid_dead(gpid, Duration::from_secs(2)));
    }

    #[test]
    fn probe_exiting_without_response_fails() {
        let d = tempfile::tempdir().unwrap();
        let probe_cwd = make_probe_dir(d.path());
        let bin = write_script(d.path(), "claude", "exit 0");
        let state = PlanUsageState::for_test(
            Some(probe_cwd),
            bin,
            base_env(),
            Duration::from_secs(5),
            Duration::from_millis(100),
            CACHE_TTL,
        );
        let err = state.get(false).unwrap_err();
        assert_eq!(err.message, "claude 没有返回额度");
    }

    #[test]
    fn leader_exiting_immediately_with_stdout_holding_descendant_kills_descendant() {
        let d = tempfile::tempdir().unwrap();
        let probe_cwd = make_probe_dir(d.path());
        let desc_pidfile = d.path().join("desc.pid");
        // Leader spawns a background process that inherits stdout, then exits immediately without
        // writing a response. Cleanup must SIGKILL the process group before reaping the leader.
        let bin = write_script(
            d.path(),
            "claude",
            &format!("sleep 30 &\necho $! > {}\nexit 0", desc_pidfile.display()),
        );
        let state = PlanUsageState::for_test(
            Some(probe_cwd),
            bin,
            base_env(),
            Duration::from_secs(5),
            Duration::from_secs(2),
            CACHE_TTL,
        );
        let started = Instant::now();
        let err = state.get(false).unwrap_err();
        assert_eq!(err.message, "claude 没有返回额度");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "should not wait for full 5s timeout"
        );
        let desc_pid = read_pid(&desc_pidfile);
        assert!(wait_pid_dead(desc_pid, Duration::from_secs(2)));
    }

    #[test]
    fn probe_lingering_after_response_returns_immediately_and_is_killed() {
        let d = tempfile::tempdir().unwrap();
        let probe_cwd = make_probe_dir(d.path());
        let pidfile = d.path().join("linger.pid");
        let bin = write_script(
            d.path(),
            "claude",
            &format!(
                "echo $$ > {}\n{}\nexec sleep 30",
                pidfile.display(),
                reply_with_body(SAMPLE_BODY)
            ),
        );
        let grace = Duration::from_millis(1200);
        let state = PlanUsageState::for_test(
            Some(probe_cwd),
            bin,
            base_env(),
            Duration::from_secs(5),
            grace,
            CACHE_TTL,
        );
        let started = Instant::now();
        let usage = state.get(false).unwrap().unwrap();
        let elapsed = started.elapsed();
        assert_eq!(usage.limits[0].percent, 25.0);
        // Command must return immediately without waiting for the grace period.
        assert!(
            elapsed < grace,
            "returned after {elapsed:?}, expected < grace {grace:?}"
        );

        let pid = read_pid(&pidfile);
        assert!(wait_pid_dead(pid, Duration::from_secs(4)));
    }

    #[test]
    fn refresh_waits_for_previous_probe_cleanup_without_overlapping() {
        // Case 1: lingering probe killed by SIGKILL after grace period — second probe must never
        // observe the first probe's PID still alive.
        let d = tempfile::tempdir().unwrap();
        let probe_cwd = make_probe_dir(d.path());
        let prev_pid = d.path().join("prev.pid");
        let overlap = d.path().join("overlap");
        let bin = write_script(
            d.path(),
            "claude",
            &format!(
                r#"if [ -f {prev} ]; then
  old=$(cat {prev})
  if kill -0 "$old" 2>/dev/null; then
    echo "overlap $old" > {ov}
  fi
fi
echo $$ > {prev}
{reply}
exec sleep 30"#,
                prev = prev_pid.display(),
                ov = overlap.display(),
                reply = reply_with_body(SAMPLE_BODY),
            ),
        );
        let grace = Duration::from_millis(800);
        let state = PlanUsageState::for_test(
            Some(probe_cwd),
            bin,
            base_env(),
            Duration::from_secs(5),
            grace,
            CACHE_TTL,
        );
        let started = Instant::now();
        assert_eq!(state.get(false).unwrap().unwrap().limits[0].percent, 25.0);
        // First call returns immediately while the child is still in its grace period.
        assert!(started.elapsed() < grace);
        // Non-refresh call hits cache immediately without waiting for cleanup.
        let cache_start = Instant::now();
        assert_eq!(state.get(false).unwrap().unwrap().limits[0].percent, 25.0);
        assert!(cache_start.elapsed() < Duration::from_millis(100));
        // Immediate refresh waits for cleanup of the first probe before spawning the second.
        assert_eq!(state.get(true).unwrap().unwrap().limits[0].percent, 25.0);
        assert!(
            !overlap.exists(),
            "second probe overlapped with first: {:?}",
            std::fs::read_to_string(&overlap).ok()
        );
        state.shutdown();

        // Case 2: probe that exits gracefully after a short delay on stdin EOF — log must show
        // strictly serialized start/exit pairs with concurrency never exceeding 1.
        let d2 = tempfile::tempdir().unwrap();
        let probe_cwd2 = make_probe_dir(d2.path());
        let events = d2.path().join("events.log");
        let bin2 = write_script(
            d2.path(),
            "claude",
            &format!(
                r#"echo start >> {ev}
{reply}
read _ || true
sleep 0.25
echo exit >> {ev}"#,
                ev = events.display(),
                reply = reply_with_body(SAMPLE_BODY),
            ),
        );
        let state2 = PlanUsageState::for_test(
            Some(probe_cwd2),
            bin2,
            base_env(),
            Duration::from_secs(5),
            Duration::from_secs(2),
            CACHE_TTL,
        );
        assert!(state2.get(false).unwrap().is_some());
        assert!(state2.get(true).unwrap().is_some());
        state2.shutdown();
        let lines: Vec<String> = std::fs::read_to_string(&events)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        // First probe finished cleanly before second started.
        assert_eq!(&lines[..3], ["start", "exit", "start"]);
    }

    #[test]
    fn concurrent_calls_share_one_probe_cache_and_refresh_and_error_behavior() {
        let d = tempfile::tempdir().unwrap();
        let probe_cwd = make_probe_dir(d.path());
        let count_file = d.path().join("count");
        let bin = write_script(
            d.path(),
            "claude",
            &format!(
                r#"echo x >> {}
sleep 0.2
{}"#,
                count_file.display(),
                reply_with_body(SAMPLE_BODY),
            ),
        );
        let state = Arc::new(PlanUsageState::for_test(
            Some(probe_cwd),
            bin,
            base_env(),
            Duration::from_secs(5),
            Duration::from_millis(100),
            Duration::from_secs(60),
        ));

        // 4 concurrent callers (mix of refresh=false and refresh=true) spawn only ONE probe.
        let barrier = Arc::new(Barrier::new(4));
        let mut handles = Vec::new();
        for i in 0..4 {
            let st = state.clone();
            let b = barrier.clone();
            handles.push(std::thread::spawn(move || {
                b.wait();
                st.get(i % 2 == 1)
            }));
        }
        let results: Vec<Option<PlanUsage>> = handles
            .into_iter()
            .map(|h| h.join().unwrap().unwrap())
            .collect();
        for r in &results {
            assert_eq!(r.as_ref().unwrap().limits[0].percent, 25.0);
        }
        let runs = || {
            std::fs::read_to_string(&count_file)
                .map(|s| s.lines().count())
                .unwrap_or(0)
        };
        assert_eq!(runs(), 1);

        // Within cache TTL, refresh=false reuses cached result without spawning again.
        let cached = state.get(false).unwrap().unwrap();
        assert_eq!(cached.limits[0].percent, 25.0);
        assert_eq!(runs(), 1);

        // refresh=true bypasses cache and spawns a new probe.
        let refreshed = state.get(true).unwrap().unwrap();
        assert_eq!(refreshed.limits[0].percent, 25.0);
        assert_eq!(runs(), 2);
    }

    #[test]
    fn none_is_cached_and_errors_are_not_cached() {
        let d = tempfile::tempdir().unwrap();
        let probe_cwd = make_probe_dir(d.path());
        let count_file = d.path().join("count");
        let unavailable_body =
            r#"{"subscription_type":null,"rate_limits_available":false,"rate_limits":{}}"#;
        let bin = write_script(
            d.path(),
            "claude",
            &format!(
                r#"echo x >> {cnt}
n=$(wc -l < {cnt} | tr -d ' ')
if [ "$n" = "1" ]; then
  exit 1
fi
{reply}"#,
                cnt = count_file.display(),
                reply = reply_with_body(unavailable_body),
            ),
        );
        let state = PlanUsageState::for_test(
            Some(probe_cwd),
            bin,
            base_env(),
            Duration::from_secs(5),
            Duration::from_millis(100),
            Duration::from_secs(60),
        );
        // First call fails; failure must not be cached.
        assert_eq!(state.get(false).unwrap_err().message, "claude 没有返回额度");
        // Second call runs again and gets Ok(None), which IS cached.
        assert_eq!(state.get(false).unwrap(), None);
        assert_eq!(state.get(false).unwrap(), None);
        let runs = std::fs::read_to_string(&count_file)
            .unwrap()
            .lines()
            .count();
        assert_eq!(runs, 2);
    }

    #[test]
    fn missing_claude_reports_not_found_and_missing_probe_dir_refuses_spawn() {
        let d = tempfile::tempdir().unwrap();
        let probe_cwd = make_probe_dir(d.path());
        let missing = d.path().join("no-such-claude");
        let state = PlanUsageState::for_test(
            Some(probe_cwd),
            missing,
            base_env(),
            Duration::from_secs(5),
            Duration::from_millis(100),
            CACHE_TTL,
        );
        let err = state.get(false).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
        assert_eq!(err.message, "未找到 claude");

        // When Engine has no registered probe cwd (None), the probe refuses to spawn.
        let marker = d.path().join("ran");
        let bin = write_script(
            d.path(),
            "claude",
            &format!(
                "touch {}\n{}",
                marker.display(),
                reply_with_body(SAMPLE_BODY)
            ),
        );
        let no_dir = PlanUsageState::for_test(
            None,
            bin,
            base_env(),
            Duration::from_secs(5),
            Duration::from_millis(100),
            CACHE_TTL,
        );
        assert!(no_dir.get(false).is_err());
        assert!(!marker.exists());
    }

    #[test]
    fn shutdown_kills_running_probe_blocks_new_ones_and_handles_concurrent_get() {
        let d = tempfile::tempdir().unwrap();
        let probe_cwd = make_probe_dir(d.path());
        let pidfile = d.path().join("running.pid");
        let bin = write_script(
            d.path(),
            "claude",
            &format!("echo $$ > {}\nexec sleep 30", pidfile.display()),
        );
        let state = Arc::new(PlanUsageState::for_test(
            Some(probe_cwd),
            bin,
            base_env(),
            Duration::from_secs(10),
            Duration::from_millis(100),
            CACHE_TTL,
        ));
        let st = state.clone();
        let handle = std::thread::spawn(move || st.get(false));
        let pid = read_pid(&pidfile);
        assert!(pid_alive(pid));

        state.shutdown();
        assert!(wait_pid_dead(pid, Duration::from_secs(2)));
        assert!(handle.join().unwrap().is_err());
        assert_eq!(state.get(false).unwrap_err().code, ErrorCode::Cancelled);

        // Concurrent `get` and `shutdown` across multiple iterations: no child left alive after
        // `shutdown` returns, and no child spawned after `shutdown`.
        for _ in 0..8 {
            let d2 = tempfile::tempdir().unwrap();
            let cwd2 = make_probe_dir(d2.path());
            let pids_file = d2.path().join("pids");
            let bin2 = write_script(
                d2.path(),
                "claude",
                &format!("echo $$ >> {}\nexec sleep 30", pids_file.display()),
            );
            let st2 = Arc::new(PlanUsageState::for_test(
                Some(cwd2),
                bin2,
                base_env(),
                Duration::from_secs(5),
                Duration::from_millis(100),
                CACHE_TTL,
            ));
            let barrier = Arc::new(Barrier::new(4));
            let mut joins = Vec::new();
            for i in 0..3 {
                let s = st2.clone();
                let b = barrier.clone();
                joins.push(std::thread::spawn(move || {
                    b.wait();
                    let _ = s.get(i % 2 == 1);
                }));
            }
            let s_shut = st2.clone();
            let b_shut = barrier.clone();
            let shut = std::thread::spawn(move || {
                b_shut.wait();
                s_shut.shutdown();
            });
            shut.join().unwrap();
            for j in joins {
                j.join().unwrap();
            }
            assert_eq!(st2.get(true).unwrap_err().code, ErrorCode::Cancelled);
            if let Ok(text) = std::fs::read_to_string(&pids_file) {
                for line in text.lines() {
                    if let Ok(p) = line.trim().parse::<i32>() {
                        assert!(
                            wait_pid_dead(p, Duration::from_secs(2)),
                            "pid {p} survived shutdown"
                        );
                    }
                }
            }
        }
    }

    /// Manual verification against the real logged-in `claude` binary on this machine.
    /// Run with: `cargo test -p codeck-host real_claude_probe_returns_usage -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_claude_probe_returns_usage() {
        let d = tempfile::tempdir().unwrap();
        let cwd = prepare_dir(d.path()).expect("prepare plan-usage dir");
        let state = PlanUsageState::new(Some(cwd));
        let usage = state.get(true).expect("real claude probe succeeded");
        println!("real claude plan usage: {usage:#?}");
        let usage = usage.expect("logged-in account has subscription rate limits");
        assert!(!usage.limits.is_empty(), "expected at least one limit item");
    }
}
