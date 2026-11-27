//! 缓存保活（「设置 › 通用 › 缓存保活」）：Claude Code 的提示词缓存过期后，下一条提示词要把整个上下文重新写入缓存。
//! Live Session 空闲到设定的时长（默认 40 分钟）而它的 1 小时缓存还没过期时，Host 经 ccrpc 发一条 `/btw`（旁问：答案
//! 只在面板里，不进 transcript，不加长对话），读到答案后关掉面板，缓存就从这时起再算一小时。
//!
//! - 缓存上次被用到的时刻：transcript 里最后一条 `api-request`（claude 2.1.300 起每次主线请求都记一条，带缓存的 TTL）与
//!   上次成功保活的较晚者；后者存在缓存目录的 `keepalive.json`，Host 重启后还在。没有 `api-request` 的、缓存不是 1 小时的、
//!   claude 自己在续的（请求带它的 `cache-keepalive-…` beta）不续；离过期不到 [`EXPIRY_MARGIN`] 的也不续（缓存已经没了或
//!   马上没了，续只是重新写入），直到它再有动静。
//! - 只续 clay 里装了插件、此刻空闲、空闲压缩关着的 Session：claude 的空闲压缩只看主线请求，`/btw` 推迟不了它（CoDeck 在
//!   保活开着时启动的都关着，见 `ccrpc::launch`）。已有连接的经那个连接发，没有的临时 `ccrpc attach`，发完就断开。
//! - 没续成的（跳过或失败）不算续过，[`RETRY`] 后再试；跳过的原因变了才写进 gui.log。
//! - 期间前端收到 `KeepAlive` 事件：面板开着时 Live 状态是 waiting，这不算等待作答。

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant, SystemTime};

use codeck_core::model::LiveStatus;
use parking_lot::Mutex;
use serde_json::{Map, Value, json};

use crate::Host;
use crate::ccrpc::launch;
use crate::ccrpc::relay::{self, App};
use crate::connect::ConnectHub;
use crate::events::HostEvent;
use crate::settings;

/// What the `/btw` asks; its question marks it as the Host's own (`is_marker`), which no one is shown.
pub const QUESTION: &str = "ping, just say pong.";
/// What `is_marker` looks for: the question as ccrpc reads it off the screen, its last character aside.
const MARKER: &str = "ping, just say pong";
/// How long the prompt cache the keep-alive refreshes lasts unused (the one-hour kind).
pub const CACHE_TTL: Duration = Duration::from_secs(60 * 60);
const TICK: Duration = Duration::from_secs(30);
/// No refresh this close to the cache's expiry: the `/btw` takes seconds, a tick 30 of them.
const EXPIRY_MARGIN: Duration = Duration::from_secs(3 * 60);
/// After an attempt that refreshed nothing (skipped or failed), the next one.
const RETRY: Duration = Duration::from_secs(5 * 60);
/// How much of a transcript's end is read for its last `api-request` (one comes with every main-thread request).
const TAIL_BYTES: u64 = 1 << 20;
/// For each step of a refresh: ccrpc's `ready`, the panel, the answer, closing it.
const STEP_TIMEOUT: Duration = Duration::from_secs(60);

/// A `btw` event (or request) of the Host's own `/btw`.
pub fn is_marker(m: &Value) -> bool {
    m["question"]
        .as_str()
        .is_some_and(|q| q.trim().starts_with(MARKER))
}

/// The last main-thread request a transcript records.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LastRequest {
    pub at: SystemTime,
    /// Its prompt cache is the one-hour kind (every `cache_control` it set says `1h`).
    pub one_hour: bool,
    /// claude renews this cache itself (the request carries its `cache-keepalive-…` beta).
    pub renewed_by_claude: bool,
}

/// The last `api-request` entry in `text`, a transcript's end (its first line may be cut).
pub(crate) fn last_request_in(text: &str) -> Option<LastRequest> {
    text.lines()
        .rev()
        .filter(|l| l.contains(r#""type":"api-request""#))
        .find_map(|l| {
            let v: Value = serde_json::from_str(l).ok()?;
            if v["type"] != "api-request" {
                return None;
            }
            let at = chrono::DateTime::parse_from_rfc3339(v["timestamp"].as_str()?).ok()?;
            let controls = v["cacheControl"].as_array()?;
            let betas = v["params"]["betas"].as_array();
            Some(LastRequest {
                at: at.into(),
                one_hour: !controls.is_empty()
                    && controls.iter().all(|c| c["value"]["ttl"] == "1h"),
                renewed_by_claude: betas.is_some_and(|b| {
                    b.iter().any(|x| {
                        x.as_str()
                            .is_some_and(|s| s.starts_with("cache-keepalive-"))
                    })
                }),
            })
        })
}

/// The last main-thread request in the transcript at `path`, from its last [`TAIL_BYTES`].
fn last_request(path: &Path) -> Option<LastRequest> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))
        .ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    last_request_in(&String::from_utf8_lossy(&buf))
}

/// A Live Session as the keep-alive sees it.
pub(crate) struct Seen {
    pub session_id: String,
    pub idle: bool,
    /// None when its transcript records none (claude before 2.1.300, or not indexed yet).
    pub last_request: Option<LastRequest>,
}

/// The Sessions whose cache to refresh at `now`: idle, with a one-hour cache claude does not renew itself, last used
/// (the later of its last request and the last refresh) at least `interval` ago and not within [`EXPIRY_MARGIN`] of
/// expiring, and not tried in the last [`RETRY`].
pub(crate) fn due(
    now: SystemTime,
    interval: Duration,
    seen: &[Seen],
    refreshed: &HashMap<String, SystemTime>,
    tried: &HashMap<String, SystemTime>,
) -> Vec<String> {
    let since = |t: SystemTime| now.duration_since(t).unwrap_or_default();
    seen.iter()
        .filter(|s| s.idle)
        .filter_map(|s| {
            let r = s.last_request?;
            if !r.one_hour || r.renewed_by_claude {
                return None;
            }
            let last = refreshed.get(&s.session_id).map_or(r.at, |t| r.at.max(*t));
            let age = since(last);
            let tried_lately = tried.get(&s.session_id).is_some_and(|t| since(*t) < RETRY);
            (age >= interval && age + EXPIRY_MARGIN < CACHE_TTL && !tried_lately)
                .then(|| s.session_id.clone())
        })
        .collect()
}

/// A transcript's last request as read: the file's size and modification time then, and what it held.
type ReadRequest = (u64, SystemTime, Option<LastRequest>);

#[derive(Default)]
pub(crate) struct KeepAliveState {
    /// Where `refreshed` is kept across restarts; None: not kept.
    path: Option<PathBuf>,
    /// When the Host last refreshed each Session's cache (its `/btw` answered).
    refreshed: Mutex<HashMap<String, SystemTime>>,
    /// When it last tried, whatever came of it.
    tried: Mutex<HashMap<String, SystemTime>>,
    /// Why each Session was last skipped, to say it in gui.log once.
    skipped: Mutex<HashMap<String, String>>,
    /// Each transcript's last request, with the size and modification time it was read at.
    requests: Mutex<HashMap<String, ReadRequest>>,
    running: Mutex<HashSet<String>>,
}

impl KeepAliveState {
    /// With the refreshes kept in `path` (`{"<Session ID>": <unix seconds>}`).
    pub(crate) fn load(path: PathBuf) -> Self {
        let refreshed = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<HashMap<String, u64>>(&t).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|(id, s)| (id, SystemTime::UNIX_EPOCH + Duration::from_secs(s)))
            .collect();
        Self {
            path: Some(path),
            refreshed: Mutex::new(refreshed),
            ..Self::default()
        }
    }

    fn save(&self, refreshed: &HashMap<String, SystemTime>) {
        let Some(path) = &self.path else { return };
        let secs: HashMap<&str, u64> = refreshed
            .iter()
            .filter_map(|(id, t)| {
                Some((
                    id.as_str(),
                    t.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs(),
                ))
            })
            .collect();
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json!(secs).to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }

    fn refreshed(&self, session_id: &str, at: SystemTime) {
        let mut r = self.refreshed.lock();
        r.insert(session_id.to_string(), at);
        self.save(&r);
    }

    /// The Sessions whose cache is being refreshed now (their `/btw` panel may be open).
    pub(crate) fn refreshing(&self) -> HashSet<String> {
        self.running.lock().clone()
    }

    /// Sessions no longer Live are forgotten.
    fn keep_only(&self, live: &HashSet<&str>) {
        let mut r = self.refreshed.lock();
        let before = r.len();
        r.retain(|id, _| live.contains(id.as_str()));
        if r.len() != before {
            self.save(&r);
        }
        drop(r);
        self.tried.lock().retain(|id, _| live.contains(id.as_str()));
        self.skipped
            .lock()
            .retain(|id, _| live.contains(id.as_str()));
        self.requests
            .lock()
            .retain(|id, _| live.contains(id.as_str()));
    }

    /// The last request in a Session's transcript at `path`, read again only once the file has changed.
    fn last_request(&self, session_id: &str, path: &Path) -> Option<LastRequest> {
        let meta = std::fs::metadata(path).ok()?;
        let (len, modified) = (meta.len(), meta.modified().ok()?);
        let mut cache = self.requests.lock();
        if let Some((l, m, r)) = cache.get(session_id)
            && (*l, *m) == (len, modified)
        {
            return *r;
        }
        let r = last_request(path);
        cache.insert(session_id.to_string(), (len, modified, r));
        r
    }
}

/// Checks the Live Sessions every [`TICK`] while the Host is alive.
pub(crate) fn spawn(host: Weak<Host>) {
    let _ = std::thread::Builder::new()
        .name("cv-keepalive".into())
        .spawn(move || {
            loop {
                std::thread::sleep(TICK);
                let Some(host) = host.upgrade() else { break };
                host.keep_alive_tick();
            }
        });
}

/// How a Session is reached: its connection, or a Pane to attach to for the moment.
enum Route {
    Conn(Arc<ConnectHub>, String),
    Attach(String),
}

impl Host {
    fn keep_alive_tick(self: &Arc<Self>) {
        let minutes = settings::load().keep_alive_minutes();
        if minutes == 0 {
            return;
        }
        let engine = self.state.engine();
        let Ok(live) = engine.live_now() else { return };
        let ka = &self.keep_alive;
        let seen: Vec<Seen> = live
            .iter()
            .map(|e| {
                let idle = e.state.status == LiveStatus::Idle;
                Seen {
                    session_id: e.session_id.clone(),
                    idle,
                    // Read for idle Sessions only: a busy one is using its cache anyway.
                    last_request: idle
                        .then(|| engine.session_file_path(&e.session_id, None).ok())
                        .flatten()
                        .and_then(|p| ka.last_request(&e.session_id, &p)),
                }
            })
            .collect();
        let now = SystemTime::now();
        let due = due(
            now,
            Duration::from_secs(u64::from(minutes) * 60),
            &seen,
            &ka.refreshed.lock(),
            &ka.tried.lock(),
        );
        for id in due {
            if !ka.running.lock().insert(id.clone()) {
                continue;
            }
            ka.tried.lock().insert(id.clone(), now);
            let host = self.clone();
            let _ = std::thread::Builder::new()
                .name("cv-keepalive-ping".into())
                .spawn(move || {
                    if host.keep_alive(&id) {
                        host.keep_alive.refreshed(&id, SystemTime::now());
                    }
                    host.keep_alive.running.lock().remove(&id);
                });
        }
        ka.keep_only(&seen.iter().map(|s| s.session_id.as_str()).collect());
    }

    /// Refreshes one Session's cache, if ccrpc can reach it and it is still idle; whether it did.
    fn keep_alive(&self, session_id: &str) -> bool {
        let Ok(app) = self.ccrpc.app() else {
            return false;
        };
        let route = match self.keep_alive_route(app, session_id) {
            Ok(Ok(r)) => r,
            Ok(Err(why)) => {
                self.keep_alive_skipped(app, session_id, why);
                return false;
            }
            Err(e) => {
                self.keep_alive_skipped(app, session_id, e.message);
                return false;
            }
        };
        self.keep_alive.skipped.lock().remove(session_id);
        let emit = |active: bool, error: Option<String>| {
            (self.events)(HostEvent::KeepAlive {
                session_id: session_id.to_string(),
                active,
                error,
            })
        };
        emit(true, None);
        let (how, result) = match route {
            Route::Conn(hub, conn) => (format!("via {conn}"), via_conn(&hub, &conn)),
            Route::Attach(holder) => (format!("attach {holder}"), via_attach(app, &holder)),
        };
        let error = result.err().map(|e| e.message);
        app.gui_line(&format!(
            "keepalive {session_id} {how}: {}",
            error.as_deref().unwrap_or("ok")
        ));
        let ok = error.is_none();
        emit(false, error);
        ok
    }

    /// Says in gui.log why a Session was not refreshed, once for each reason in a row.
    fn keep_alive_skipped(&self, app: &App, session_id: &str, why: String) {
        let mut skipped = self.keep_alive.skipped.lock();
        if skipped.get(session_id) != Some(&why) {
            app.gui_line(&format!("keepalive {session_id}: 跳过，{why}"));
            skipped.insert(session_id.to_string(), why);
        }
    }

    /// Where to send the `/btw`, or why not now.
    fn keep_alive_route(&self, app: &App, session_id: &str) -> relay::R<Result<Route, String>> {
        let ls = app.ls_once()?;
        let Some(e) = ls.iter().find(|e| e["session_id"] == session_id) else {
            return Ok(Err("不在 clay 里".into()));
        };
        let holder = match drivable(e) {
            Ok(h) => h,
            Err(why) => return Ok(Err(why)),
        };
        let data_root = PathBuf::from(self.state.engine().app_info().data_root);
        let off = e["claude_pid"]
            .as_u64()
            .and_then(|pid| argv(u32::try_from(pid).ok()?))
            .is_some_and(|argv| {
                let cwd = e["cwd"].as_str().map(PathBuf::from).unwrap_or_default();
                idle_compaction_setting(&argv, &cwd, &data_root) == Some(false)
            });
        if !off {
            return Ok(Err("claude 的空闲压缩没关".into()));
        }
        if let Some(hub) = &self.connect
            && let Some(conn) = hub.live_conn_for(holder)
        {
            return Ok(Ok(Route::Conn(hub.clone(), conn)));
        }
        Ok(Ok(Route::Attach(holder.to_string())))
    }
}

/// The Holder to send the `/btw` to, from the Session's row of `ccrpc ls --json`; Err says why not now.
fn drivable(e: &Value) -> Result<&str, String> {
    let holder = e["holder_id"]
        .as_str()
        .ok_or_else(|| "ccrpc ls 没有给出 Holder ID".to_string())?;
    if e["plugin"] != true {
        return Err("没有 ccrpc 插件".into());
    }
    // `status` is the process file's: read as Live status is, so `shell` (a background task running) is idle.
    let status = e["status"].as_str();
    if LiveStatus::from_raw(status) != LiveStatus::Idle {
        return Err(format!("状态是 {}", status.unwrap_or("-")));
    }
    Ok(holder)
}

/// `idleCompaction` as claude started with `argv` in `cwd` reads it, policy settings aside: the first that sets it of
/// its `--settings` ([`launch::settings_flag`]), `cwd`'s `.claude/settings.local.json` and `.claude/settings.json`,
/// and the user's `settings.json` in the data root; None when none does (claude's default: on).
pub(crate) fn idle_compaction_setting(
    argv: &[String],
    cwd: &Path,
    data_root: &Path,
) -> Option<bool> {
    let flag = launch::settings_flag(argv)
        .and_then(|(i, prefix)| launch::settings_object(&argv[i][prefix.len()..], cwd));
    let file = |p: PathBuf| {
        std::fs::read_to_string(p)
            .ok()
            .and_then(|t| serde_json::from_str::<Map<String, Value>>(&t).ok())
    };
    [
        flag,
        file(cwd.join(".claude/settings.local.json")),
        file(cwd.join(".claude/settings.json")),
        file(data_root.join("settings.json")),
    ]
    .into_iter()
    .flatten()
    .find_map(|s| s.get("idleCompaction")?.as_bool())
}

/// A process's arguments, argv[0] first; None when they cannot be read.
#[cfg(target_os = "macos")]
fn argv(pid: u32) -> Option<Vec<String>> {
    let mut argmax: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: `mib` names an int-sized value and `argmax` is one, `size` its size.
    let r = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&raw mut argmax).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 {
        return None;
    }
    let mut buf = vec![0u8; usize::try_from(argmax).ok()?];
    let mut size = buf.len();
    let mut mib = [
        libc::CTL_KERN,
        libc::KERN_PROCARGS2,
        libc::c_int::try_from(pid).ok()?,
    ];
    // SAFETY: `buf` has `size` writable bytes; the kernel writes at most that many and says how many in `size`.
    let r = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 {
        return None;
    }
    procargs(&buf[..size])
}

/// `KERN_PROCARGS2`: argc (an int), the executable's path, NUL padding, then argc NUL-ended arguments (the
/// environment after them).
#[cfg(any(target_os = "macos", test))]
fn procargs(b: &[u8]) -> Option<Vec<String>> {
    let argc = usize::try_from(i32::from_ne_bytes(b.get(..4)?.try_into().ok()?)).ok()?;
    let rest = &b[4..];
    let path_end = rest.iter().position(|&c| c == 0)?;
    let start = path_end + rest[path_end..].iter().position(|&c| c != 0)?;
    let args: Vec<String> = rest[start..]
        .split(|&c| c == 0)
        .take(argc)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    (args.len() == argc).then_some(args)
}

#[cfg(target_os = "linux")]
fn argv(pid: u32) -> Option<Vec<String>> {
    let b = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let b = b.strip_suffix(&[0]).unwrap_or(&b);
    Some(
        b.split(|&c| c == 0)
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect(),
    )
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn argv(_pid: u32) -> Option<Vec<String>> {
    None
}

fn prompt() -> Value {
    json!({ "type": "prompt", "text": format!("/btw {QUESTION}") })
}

/// The response to `prompt()`: the panel is up.
fn check_btw(r: &Value) -> relay::R<()> {
    if r["success"] == true && r["data"]["delivered_as"] == "btw" {
        return Ok(());
    }
    let why = r["error"]["message"]
        .as_str()
        .unwrap_or("没有出现 /btw 面板");
    Err(relay::failed(why.to_string()))
}

/// Waits for the answer of our `/btw` among `lines` (it has used the cache by then).
fn wait_answer(lines: &Receiver<Value>) -> relay::R<()> {
    let deadline = Instant::now() + STEP_TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match lines.recv_timeout(left) {
            Ok(m) if m["type"] == "btw" && is_marker(&m) && m["text"].is_string() => return Ok(()),
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => return Err(relay::io("等不到 /btw 的回答")),
            Err(RecvTimeoutError::Disconnected) => return Err(relay::io("ccrpc 已退出")),
        }
    }
}

/// Over the Session's connection, as the Host's own commands (their responses go to no client).
fn via_conn(hub: &Arc<ConnectHub>, conn: &str) -> relay::R<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|e| relay::io(format!("建不了运行时：{e}")))?;
    let lines = hub.observe(conn);
    check_btw(&rt.block_on(hub.command(conn, prompt()))?)?;
    let answered = wait_answer(&lines);
    // Closed even without an answer: the panel would hide the input box.
    let closed = rt.block_on(hub.command(conn, json!({ "type": "close_btw" })));
    answered?;
    closed.map(|_| ())
}

/// A ccrpc of its own for the moment (`attach`), closed when done.
fn via_attach(app: &App, holder: &str) -> relay::R<()> {
    let mut cmd = app.command();
    cmd.args(["attach", holder])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child =
        relay::spawn(&mut cmd).map_err(|e| relay::io(format!("运行不了 ccrpc attach：{e}")))?;
    let mut stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let (tx, lines) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        let Some(out) = stdout else { return };
        for line in BufReader::new(out).lines() {
            let Ok(line) = line else { break };
            if let Ok(v) = serde_json::from_str::<Value>(&line)
                && tx.send(v).is_err()
            {
                break;
            }
        }
    });
    let mut send = |v: Value| -> relay::R<()> {
        let w = stdin
            .as_mut()
            .ok_or_else(|| relay::io("ccrpc 的 stdin 已关闭"))?;
        writeln!(w, "{v}")
            .and_then(|()| w.flush())
            .map_err(|e| relay::io(format!("写不进 ccrpc：{e}")))
    };
    let result = (|| {
        wait_for(&lines, |m| m["type"] == "ready", "ccrpc 没有就绪")?;
        let mut p = prompt();
        p["id"] = "k1".into();
        send(p)?;
        let r = wait_for(
            &lines,
            |m| m["type"] == "response" && m["id"] == "k1",
            "ccrpc 没有回应 /btw",
        )?;
        check_btw(&r)?;
        let answered = wait_answer(&lines);
        send(json!({ "id": "k2", "type": "close_btw" }))?;
        wait_for(
            &lines,
            |m| m["type"] == "response" && m["id"] == "k2",
            "ccrpc 没有关掉 /btw 面板",
        )?;
        answered
    })();
    // Closing stdin detaches ccrpc; the Session keeps running.
    drop(stdin);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && matches!(child.try_wait(), Ok(None)) {
        std::thread::sleep(Duration::from_millis(50));
    }
    if matches!(child.try_wait(), Ok(None)) {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn wait_for(lines: &Receiver<Value>, want: impl Fn(&Value) -> bool, why: &str) -> relay::R<Value> {
    let deadline = Instant::now() + STEP_TIMEOUT;
    loop {
        match lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(m) if want(&m) => return Ok(m),
            Ok(_) => {}
            Err(_) => return Err(relay::io(why.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    fn t0() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000)
    }

    fn request(at: SystemTime) -> LastRequest {
        LastRequest {
            at,
            one_hour: true,
            renewed_by_claude: false,
        }
    }

    fn seen(id: &str, idle: bool, last_request: Option<LastRequest>) -> Seen {
        Seen {
            session_id: id.into(),
            idle,
            last_request,
        }
    }

    #[test]
    fn due_from_the_interval_until_close_to_expiry() {
        let s = [seen("a", true, Some(request(t0())))];
        let none = HashMap::new();
        let at = |m: u32| due(t0() + m * MIN, 40 * MIN, &s, &none, &none);
        assert!(at(39).is_empty());
        assert_eq!(at(40), ["a"]);
        assert_eq!(at(56), ["a"]);
        // Within the last 3 minutes, or past the hour: refreshing would only write the cache again.
        assert!(at(57).is_empty());
        assert!(at(61).is_empty());
    }

    #[test]
    fn only_a_refresh_starts_the_interval_again_and_a_failed_try_waits_five_minutes() {
        let s = [seen("a", true, Some(request(t0())))];
        let none = HashMap::new();
        let refreshed = HashMap::from([("a".to_string(), t0() + 40 * MIN)]);
        assert!(due(t0() + 70 * MIN, 40 * MIN, &s, &refreshed, &none).is_empty());
        assert_eq!(due(t0() + 80 * MIN, 40 * MIN, &s, &refreshed, &none), ["a"]);
        // Tried at 40 and nothing came of it: again at 45, not counted as a refresh.
        let tried = HashMap::from([("a".to_string(), t0() + 40 * MIN)]);
        assert!(due(t0() + 44 * MIN, 40 * MIN, &s, &none, &tried).is_empty());
        assert_eq!(due(t0() + 45 * MIN, 40 * MIN, &s, &none, &tried), ["a"]);
    }

    #[test]
    fn busy_unknown_five_minute_and_self_renewed_caches_are_left_alone() {
        let none = HashMap::new();
        let at = |s: Seen| due(t0() + 45 * MIN, 40 * MIN, &[s], &none, &none);
        assert!(at(seen("a", false, Some(request(t0())))).is_empty());
        assert!(at(seen("a", true, None)).is_empty());
        let five = LastRequest {
            one_hour: false,
            ..request(t0())
        };
        assert!(at(seen("a", true, Some(five))).is_empty());
        let renewed = LastRequest {
            renewed_by_claude: true,
            ..request(t0())
        };
        assert!(at(seen("a", true, Some(renewed))).is_empty());
    }

    #[test]
    fn the_last_api_request_gives_when_and_which_cache() {
        let line = |ts: &str, ttl: &str, beta: &str| {
            json!({
                "type": "api-request", "timestamp": ts, "querySource": "repl_main_thread",
                "params": { "betas": ["claude-code-20250219", beta] },
                "cacheControl": [
                    { "path": ["system", 1], "value": { "type": "ephemeral", "ttl": "1h", "scope": "global" } },
                    { "path": ["messages", 1, "content", 0], "value": { "type": "ephemeral", "ttl": ttl } }
                ]
            })
            .to_string()
        };
        let text = [
            r#"ues","value":{"ttl":"5m"}}]}"#.to_string(),
            line("2026-11-26T12:00:00.000Z", "1h", "x"),
            line("2026-11-26T12:08:44.500Z", "1h", "x"),
            r#"{"type":"assistant","timestamp":"2026-11-26T12:09:03.000Z"}"#.into(),
            r#"{"type":"ai-title"}"#.into(),
        ]
        .join("\n");
        let r = last_request_in(&text).unwrap();
        let at = chrono::DateTime::parse_from_rfc3339("2026-11-26T12:08:44.500Z").unwrap();
        assert_eq!(r.at, SystemTime::from(at));
        assert!(r.one_hour && !r.renewed_by_claude);
        let r = last_request_in(&line("2026-11-26T12:00:00Z", "5m", "x")).unwrap();
        assert!(!r.one_hour);
        let r = last_request_in(&line(
            "2026-11-26T12:00:00Z",
            "1h",
            "cache-keepalive-2026-09-03",
        ))
        .unwrap();
        assert!(r.renewed_by_claude);
        assert_eq!(last_request_in(r#"{"type":"assistant"}"#), None);
    }

    #[test]
    fn refreshes_are_kept_across_restarts_and_forgotten_when_no_longer_live() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("keepalive.json");
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_795_000_000);
        let ka = KeepAliveState::load(path.clone());
        ka.refreshed("a", at);
        ka.refreshed("b", at);
        assert_eq!(
            KeepAliveState::load(path.clone()).refreshed.lock().get("a"),
            Some(&at)
        );
        ka.keep_only(&HashSet::from(["a"]));
        let again = KeepAliveState::load(path);
        assert_eq!(again.refreshed.lock().keys().collect::<Vec<_>>(), ["a"]);
    }

    #[test]
    fn idle_compaction_is_read_from_settings_claude_would_use() {
        let d = tempfile::tempdir().unwrap();
        let (cwd, root) = (d.path().join("w"), d.path().join("root"));
        std::fs::create_dir_all(cwd.join(".claude")).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let read = |a: &[&str]| idle_compaction_setting(&args(a), &cwd, &root);
        // CoDeck's: on the command line, inline or a file; claude keeps the last --settings.
        assert_eq!(
            read(&["claude", "--settings", r#"{"idleCompaction":false}"#]),
            Some(false)
        );
        std::fs::write(
            cwd.join("s.json"),
            r#"{"idleCompaction": false, "model": "opus"}"#,
        )
        .unwrap();
        assert_eq!(read(&["claude", "--settings=s.json"]), Some(false));
        assert_eq!(
            read(&[
                "claude",
                "--settings",
                r#"{"idleCompaction":false}"#,
                "--settings",
                "{}"
            ]),
            None
        );
        // Else the project's local, the project's, the user's: the first that sets it.
        assert_eq!(read(&["claude"]), None);
        std::fs::write(root.join("settings.json"), r#"{"idleCompaction": false}"#).unwrap();
        assert_eq!(read(&["claude"]), Some(false));
        std::fs::write(
            cwd.join(".claude/settings.json"),
            r#"{"idleCompaction": true}"#,
        )
        .unwrap();
        assert_eq!(read(&["claude"]), Some(true));
        std::fs::write(
            cwd.join(".claude/settings.local.json"),
            r#"{"idleCompaction": false}"#,
        )
        .unwrap();
        assert_eq!(read(&["claude"]), Some(false));
        assert_eq!(
            read(&["claude", "--settings", r#"{"idleCompaction":true}"#]),
            Some(true)
        );
    }

    #[test]
    fn reads_a_processs_arguments() {
        let mut b = 3i32.to_ne_bytes().to_vec();
        b.extend(b"/usr/bin/claude\0\0\0claude\0--settings\0{\"a\": 1}\0HOME=/x\0");
        assert_eq!(
            procargs(&b).unwrap(),
            ["claude", "--settings", r#"{"a": 1}"#]
        );
        // This test's own, as the kernel has them.
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            let own: Vec<String> = std::env::args().collect();
            assert_eq!(argv(std::process::id()).unwrap(), own);
        }
    }

    #[test]
    fn a_session_running_a_background_task_is_idle_enough() {
        let row = |status: &str, plugin: bool| json!({ "holder_id": "m1", "session_id": "a", "status": status, "plugin": plugin });
        // The process file says `shell` while a background shell or Monitor runs; the input box is free.
        assert_eq!(drivable(&row("shell", true)), Ok("m1"));
        assert_eq!(drivable(&row("idle", true)), Ok("m1"));
        assert_eq!(drivable(&row("busy", true)), Err("状态是 busy".into()));
        assert!(drivable(&row("waiting", true)).is_err());
        assert_eq!(drivable(&row("idle", false)), Err("没有 ccrpc 插件".into()));
    }

    #[test]
    fn only_the_hosts_own_btw_is_a_marker() {
        assert!(is_marker(&json!({ "type": "btw", "question": QUESTION })));
        assert!(!is_marker(
            &json!({ "type": "btw", "question": "这段代码为什么慢" })
        ));
    }
}
