//! The ccrpc processes, without Tauri: one `ccrpc` child per connection, its stdout relayed line
//! by line to a sink, everything written to the connection's logs (the root docs/spec.md, Connect › 日志).

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use codeck_core::model::{AppError, ErrorCode};
use parking_lot::Mutex;
use serde::Serialize;

use super::logs::{self, Wire};
use crate::settings::LaunchRule;

pub type R<T> = Result<T, AppError>;

fn error(code: ErrorCode, message: impl Into<String>) -> AppError {
    AppError {
        code,
        message: message.into(),
    }
}

/// What the caller asked for is not valid (an ID, a directory, an image, a connection that is gone).
pub fn invalid(message: impl Into<String>) -> AppError {
    error(ErrorCode::InvalidQuery, message)
}

/// A file, pipe or process could not be used.
pub fn io(message: impl Into<String>) -> AppError {
    error(ErrorCode::Io, message)
}

/// ccrpc failed (its own error, or output that is not what it should be).
pub fn failed(message: impl Into<String>) -> AppError {
    error(ErrorCode::Internal, message)
}

/// How long a one-shot subcommand (`doctor`, `ls`, `hide`, `show`) may run before it is killed.
const SUBCOMMAND_TIMEOUT: Duration = Duration::from_secs(10);
/// How much of a connection's stderr its exit event carries.
const STDERR_TAIL_CHARS: usize = 4000;
/// How long Disconnect waits for a `send` that holds ccrpc's stdin (one stuck writing into a
/// stalled ccrpc) before ending that ccrpc instead.
const STDIN_LOCK_WAIT: Duration = Duration::from_secs(1);
/// After Disconnect closed its stdin, how long ccrpc has to detach and exit before it gets SIGTERM.
const DISCONNECT_GRACE: Duration = Duration::from_secs(5);
/// On SIGTERM ccrpc detaches once the command in progress is done (the longest one waits up to
/// 120 s, `get_messages`); one still running this long after it gets SIGKILL.
const KILL_AFTER: Duration = Duration::from_secs(150);
/// How often a connection's waiter looks whether its ccrpc exited.
const EXIT_POLL: Duration = Duration::from_millis(100);
/// Once ccrpc exited, how long to wait for each further line it wrote (a process it left behind
/// may keep its stdout open: the exit is reported once none came for this long).
const DRAIN_WAIT: Duration = Duration::from_secs(1);

#[derive(Serialize, Clone, Debug)]
pub struct ExitEvent {
    pub conn: String,
    pub code: Option<i32>,
    pub stderr: String,
}

/// What a connection reports.
#[derive(Debug, Clone)]
pub enum Out {
    Line { conn: String, line: String },
    Exit(ExitEvent),
}

pub type Sink = Arc<dyn Fn(Out) + Send + Sync>;

struct Conn {
    /// Its own lock: a stalled ccrpc blocks only its own writes, not the connection table.
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    /// The ccrpc process. Reaped only through this lock (by the connection's waiter), so a signal
    /// sent while holding it never reaches a recycled pid.
    child: Arc<Mutex<Child>>,
    wire: Arc<Mutex<Wire>>,
    gui: Arc<Mutex<File>>,
    dir: Arc<Mutex<PathBuf>>,
}

#[derive(Serialize, Clone, Debug, specta::Type)]
pub struct DoctorLine {
    pub level: String,
    pub text: String,
}

pub struct App {
    pub env: HashMap<String, String>,
    pub ccrpc: PathBuf,
    pub log_root: PathBuf,
    pub images: PathBuf,
    conns: Arc<Mutex<HashMap<String, Conn>>>,
    gui: Arc<Mutex<File>>,
    seq: Mutex<u64>,
    pub doctor: Mutex<Option<Vec<DoctorLine>>>,
    sub_timeout: Duration,
    disconnect_grace: Duration,
    kill_after: Duration,
}

impl App {
    /// Logs under `log_root` (a global `gui.log` there), pasted images in `images`.
    pub fn new(
        env: HashMap<String, String>,
        ccrpc: PathBuf,
        log_root: PathBuf,
        images: PathBuf,
    ) -> R<App> {
        logs::mkdir(&log_root)
            .map_err(|e| io(format!("建不了日志目录 {}：{e}", log_root.display())))?;
        logs::prune(&log_root, chrono::Local::now().date_naive());
        logs::prune_files(&images);
        let gui = logs::append(&log_root.join("gui.log"))
            .map_err(|e| io(format!("打不开 gui.log：{e}")))?;
        let app = App {
            env,
            ccrpc,
            log_root,
            images,
            conns: Arc::new(Mutex::new(HashMap::new())),
            gui: Arc::new(Mutex::new(gui)),
            seq: Mutex::new(0),
            doctor: Mutex::new(None),
            sub_timeout: SUBCOMMAND_TIMEOUT,
            disconnect_grace: DISCONNECT_GRACE,
            kill_after: KILL_AFTER,
        };
        app.gui_line(&format!("start ccrpc={}", app.ccrpc.display()));
        Ok(app)
    }

    /// A ccrpc process with the login shell's environment, the analyzer's settings over it (read at each
    /// start: a change applies to the next connection).
    pub fn command(&self) -> Command {
        let mut c = Command::new(&self.ccrpc);
        c.env_clear().envs(self.effective_env());
        c
    }

    pub fn effective_env(&self) -> HashMap<String, String> {
        super::env::with_analyzer(&self.env, &crate::settings::load().analyzer)
    }

    pub fn gui_line(&self, text: &str) {
        logs::line(&mut self.gui.lock(), text);
    }

    /// Runs the one-shot subcommand `args`, killed after `sub_timeout`.
    fn run(&self, args: &[&str]) -> R<Output> {
        let mut cmd = self.command();
        cmd.args(args);
        output_within(cmd, self.sub_timeout).map_err(|e| match e.kind() {
            std::io::ErrorKind::TimedOut => io(format!(
                "ccrpc {} 没有在 {} 秒内结束",
                args[0],
                self.sub_timeout.as_secs_f64()
            )),
            _ => io(format!("运行不了 {}：{e}", self.ccrpc.display())),
        })
    }

    /// A subcommand's failure: the first line of its stderr (without `ccrpc: `) as the message,
    /// all of it in gui.log.
    fn stderr_error(&self, sub: &str, stderr: &[u8]) -> AppError {
        let text = String::from_utf8_lossy(stderr);
        let text = text.trim();
        self.gui_line(&format!("{sub} stderr: {}", text.replace('\n', " | ")));
        let first = text.lines().map(str::trim).find(|l| !l.is_empty());
        failed(first.unwrap_or("").trim_start_matches("ccrpc: "))
    }

    fn next(&self) -> u64 {
        let mut s = self.seq.lock();
        *s += 1;
        *s
    }

    /// `ccrpc doctor` again (after installing ccrpc or its plugin).
    pub fn refresh_doctor(&self) -> Vec<DoctorLine> {
        self.doctor.lock().take();
        self.doctor()
    }

    pub fn doctor(&self) -> Vec<DoctorLine> {
        if let Some(d) = self.doctor.lock().clone() {
            return d;
        }
        let d = match self.run(&["doctor"]) {
            Ok(out) => String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| DoctorLine {
                    level: l.split('\t').next().unwrap_or("").to_string(),
                    text: l.to_string(),
                })
                .collect(),
            Err(e) => vec![DoctorLine {
                level: "fail".into(),
                text: format!("fail\tccrpc\t{}", e.message),
            }],
        };
        for l in d.iter().filter(|l| l.level != "ok") {
            self.gui_line(&format!("doctor {}", l.text));
        }
        *self.doctor.lock() = Some(d.clone());
        d
    }

    pub fn list_sessions(&self) -> R<serde_json::Value> {
        let out = self.run(&["ls", "--json"])?;
        if !out.status.success() {
            return Err(self.stderr_error("ls", &out.stderr));
        }
        serde_json::from_slice(&out.stdout)
            .map_err(|e| failed(format!("ccrpc ls 的输出不是 JSON：{e}")))
    }

    /// `ccrpc hide|show <holder>`: hides the Pane on the ToH Mac Client, or shows it again (ToH ADR-0050).
    pub fn set_pane_hidden(&self, holder_id: &str, hidden: bool) -> R<()> {
        if !valid_holder_id(holder_id) {
            return Err(invalid("Holder ID 不对"));
        }
        let verb = if hidden { "hide" } else { "show" };
        let out = self.run(&[verb, holder_id])?;
        self.gui_line(&format!("{verb} {holder_id} → {}", out.status));
        if !out.status.success() {
            return Err(self.stderr_error(verb, &out.stderr));
        }
        Ok(())
    }

    /// `ccrpc attach --co-display <holder>`.
    pub fn connect(&self, sink: Sink, holder_id: &str) -> R<String> {
        if !valid_holder_id(holder_id) {
            return Err(invalid("Holder ID 不对"));
        }
        self.spawn(sink, "attach", &[holder_id.to_string()], holder_id)
    }

    /// `ccrpc new --co-display [--hidden] --cwd <cwd> [-- <launch rule's claude arguments>]`.
    pub fn new_session(
        &self,
        sink: Sink,
        cwd: &str,
        rules: &[LaunchRule],
        hidden: bool,
    ) -> R<String> {
        if !Path::new(cwd).is_dir() {
            return Err(invalid(format!("{cwd} 不是目录")));
        }
        let mut args = hidden_arg(hidden);
        args.extend(["--cwd".to_string(), cwd.to_string()]);
        let extra = super::launch::args_for(rules, Path::new(cwd), &self.env).map_err(invalid)?;
        if !extra.is_empty() {
            args.push("--".into());
            args.extend(extra);
        }
        self.spawn(sink, "new", &args, "new")
    }

    /// Resume: `ccrpc new --co-display [--hidden] --cwd <cwd> -- [launch rule's arguments] --resume <session_id>`, in a
    /// new Pane.
    pub fn resume_session(
        &self,
        sink: Sink,
        session_id: &str,
        cwd: &str,
        rules: &[LaunchRule],
        hidden: bool,
    ) -> R<String> {
        // toh types the command into the Pane's shell: only a Session ID goes through.
        if !is_session_id(session_id) {
            return Err(invalid("Session ID 不对"));
        }
        if !Path::new(cwd).is_dir() {
            return Err(invalid(format!("{cwd} 不是目录")));
        }
        let mut args = hidden_arg(hidden);
        args.extend(["--cwd".to_string(), cwd.to_string(), "--".into()]);
        args.extend(super::launch::args_for(rules, Path::new(cwd), &self.env).map_err(invalid)?);
        args.extend(["--resume".to_string(), session_id.to_string()]);
        // Named like a new Session's (`new` → its Holder ID once `ready` says it); gui.log has the arguments.
        self.spawn(sink, "new", &args, "new")
    }

    /// Starts ccrpc `sub` with `args` and relays it to `sink` as a new connection.
    fn spawn(&self, sink: Sink, sub: &str, args: &[String], dir_name: &str) -> R<String> {
        let id = format!("c{}", self.next());
        let dir = logs::conn_dir(&self.log_root, dir_name)
            .map_err(|e| io(format!("建不了日志目录：{e}")))?;
        let wire = Arc::new(Mutex::new(
            Wire::open(dir.join("wire.jsonl")).map_err(|e| io(e.to_string()))?,
        ));
        let gui = Arc::new(Mutex::new(
            logs::append(&dir.join("gui.log")).map_err(|e| io(e.to_string()))?,
        ));
        let mut cmd = self.command();
        cmd.arg(sub)
            .arg("--co-display")
            .arg("--log")
            .arg(dir.join("ccrpc.log"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        logs::line(
            &mut gui.lock(),
            &format!("spawn {} {sub} {}", self.ccrpc.display(), args.join(" ")),
        );
        let mut child = cmd
            .spawn()
            .map_err(|e| io(format!("运行不了 {}：{e}", self.ccrpc.display())))?;
        let stdout = child.stdout.take().ok_or_else(|| io("没有 stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| io("没有 stderr"))?;
        self.gui_line(&format!(
            "{id} {sub} {} → {}",
            args.join(" "),
            dir.display()
        ));
        let dir = Arc::new(Mutex::new(dir));
        let stdin = Arc::new(Mutex::new(child.stdin.take()));
        let child = Arc::new(Mutex::new(child));
        self.conns.lock().insert(
            id.clone(),
            Conn {
                stdin,
                child: child.clone(),
                wire: wire.clone(),
                gui: gui.clone(),
                dir: dir.clone(),
            },
        );

        // Each reader drops its sender when its pipe ends; stdout's notes when it last relayed a line.
        let (err_done_tx, err_done) = mpsc::channel::<()>();
        let (out_done_tx, out_done) = mpsc::channel::<()>();
        let last_line = Arc::new(Mutex::new(std::time::Instant::now()));
        let err_tail = Arc::new(Mutex::new(String::new()));
        {
            let (err_tail, gui) = (err_tail.clone(), gui.clone());
            std::thread::spawn(move || {
                let _done = err_done_tx;
                // Logged as it comes (a crash of the app keeps what was written); only the tail is
                // kept for the exit event.
                let mut reader = BufReader::new(stderr);
                let mut buf = Vec::new();
                while matches!(reader.read_until(b'\n', &mut buf), Ok(n) if n > 0) {
                    let line = String::from_utf8_lossy(&buf);
                    let line = line.trim_end_matches(['\n', '\r']);
                    logs::line(&mut gui.lock(), &format!("stderr {line}"));
                    let mut tail = err_tail.lock();
                    tail.push_str(line);
                    tail.push('\n');
                    if tail.len() > 4 * STDERR_TAIL_CHARS {
                        *tail = last_chars(&tail, STDERR_TAIL_CHARS).to_string();
                    }
                    buf.clear();
                }
            });
        }
        // Lines are relayed until the exit is: the exit event is always the connection's last.
        let open = Arc::new(Mutex::new(true));
        {
            let (open, conn, sink) = (open.clone(), id.clone(), sink.clone());
            let last_line = last_line.clone();
            let rename = sub == "new";
            std::thread::spawn(move || {
                let _done = out_done_tx;
                let mut renamed = !rename;
                let mut reader = BufReader::new(stdout);
                let mut buf = Vec::new();
                loop {
                    buf.clear();
                    match reader.read_until(b'\n', &mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    let line = String::from_utf8_lossy(&buf)
                        .trim_end_matches(['\n', '\r'])
                        .to_string();
                    // Nothing is logged or relayed once the exit was reported.
                    let open = open.lock();
                    if !*open {
                        break;
                    }
                    wire.lock().write("recv", &line);
                    if !renamed {
                        renamed = rename_new_dir(&line, &dir, &wire);
                    }
                    sink(Out::Line {
                        conn: conn.clone(),
                        line,
                    });
                    *last_line.lock() = std::time::Instant::now();
                }
            });
        }
        // The exit is reported once ccrpc is gone, whether or not its stdout has ended.
        let (conns, main_gui, conn) = (self.conns.clone(), self.gui.clone(), id.clone());
        std::thread::spawn(move || {
            let code = loop {
                match child.lock().try_wait() {
                    Ok(Some(status)) => break status.code(),
                    Ok(None) => {}
                    Err(_) => break None,
                }
                std::thread::sleep(EXIT_POLL);
            };
            // Lines still coming are relayed (however slow the sink); stdout is given up only once
            // DRAIN_WAIT passes with no new line.
            *last_line.lock() = std::time::Instant::now();
            loop {
                let quiet = last_line.lock().elapsed();
                let Some(left) = DRAIN_WAIT.checked_sub(quiet) else {
                    break;
                };
                match out_done.recv_timeout(left) {
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    _ => break,
                }
            }
            let _ = err_done.recv_timeout(DRAIN_WAIT);
            logs::line(&mut gui.lock(), &format!("exit code={code:?}"));
            logs::line(&mut main_gui.lock(), &format!("{conn} exit code={code:?}"));
            conns.lock().remove(&conn);
            let stderr = last_chars(&err_tail.lock(), STDERR_TAIL_CHARS).to_string();
            *open.lock() = false;
            sink(Out::Exit(ExitEvent { conn, code, stderr }));
        });
        Ok(id)
    }

    /// One command line to ccrpc (newlines would split it: replaced by spaces).
    pub fn send(&self, conn: &str, line: &str) -> R<()> {
        let (stdin, wire) = {
            let conns = self.conns.lock();
            let c = conns.get(conn).ok_or_else(|| invalid("ccrpc 已退出"))?;
            (c.stdin.clone(), c.wire.clone())
        };
        let one = line.replace(['\n', '\r'], " ");
        let mut stdin = stdin.lock();
        let w = stdin.as_mut().ok_or_else(|| invalid("已断开"))?;
        // Logged before it is written, so the response never precedes its command in the wire.
        wire.lock().write("send", &one);
        w.write_all(format!("{one}\n").as_bytes())
            .and_then(|_| w.flush())
            .map_err(|e| io(format!("写不进 ccrpc：{e}")))
    }

    /// Disconnect: closes ccrpc's stdin, so it detaches and exits; the Session keeps running. A
    /// ccrpc that cannot take the close (a `send` stuck writing to it holds its stdin) or is still
    /// running [`DISCONNECT_GRACE`] later gets SIGTERM, on which it detaches after the command in
    /// progress; [`KILL_AFTER`] later SIGKILL. False when the connection is unknown (its ccrpc
    /// already exited).
    pub fn disconnect(&self, conn: &str) -> bool {
        let Some((stdin, gui)) = self
            .conns
            .lock()
            .get(conn)
            .map(|c| (c.stdin.clone(), c.gui.clone()))
        else {
            return false;
        };
        logs::line(&mut gui.lock(), "disconnect");
        let grace = match stdin.try_lock_for(STDIN_LOCK_WAIT) {
            Some(mut stdin) => {
                stdin.take();
                self.disconnect_grace
            }
            None => Duration::ZERO,
        };
        let (conns, conn, kill_after) = (self.conns.clone(), conn.to_string(), self.kill_after);
        std::thread::spawn(move || {
            std::thread::sleep(grace);
            let why = if grace.is_zero() {
                "stdin busy"
            } else {
                "still running after Disconnect"
            };
            if terminate(&conns, &conn, libc::SIGTERM, why) {
                std::thread::sleep(kill_after);
                terminate(&conns, &conn, libc::SIGKILL, "still running after SIGTERM");
            }
        });
        true
    }

    /// Closes every connection (the app is quitting); a ccrpc whose stdin is held by a stuck
    /// `send` gets SIGTERM.
    pub fn disconnect_all(&self) {
        let all: Vec<_> = self
            .conns
            .lock()
            .iter()
            .map(|(id, c)| (id.clone(), c.stdin.clone()))
            .collect();
        for (id, s) in all {
            match s.try_lock_for(STDIN_LOCK_WAIT) {
                Some(mut s) => drop(s.take()),
                None => {
                    terminate(&self.conns, &id, libc::SIGTERM, "stdin busy");
                }
            }
        }
    }

    pub fn gui_log(&self, conn: Option<&str>, text: &str) {
        let text = text.replace('\n', " ");
        match conn.and_then(|c| self.conns.lock().get(c).map(|c| c.gui.clone())) {
            Some(g) => logs::line(&mut g.lock(), &text),
            None => self.gui_line(&text),
        }
    }

    /// The connection's log directory (while it is connected).
    pub fn log_dir(&self, conn: &str) -> Option<PathBuf> {
        self.conns.lock().get(conn).map(|c| c.dir.lock().clone())
    }

    pub fn save_image(&self, base64: &str, ext: &str) -> R<String> {
        use base64::Engine;
        let ext = ext.to_ascii_lowercase();
        // About 20 MB once decoded.
        if base64.len() > 28 * 1024 * 1024 {
            return Err(invalid("图片超过 20 MB"));
        }
        if !["png", "jpg", "jpeg", "gif", "webp"].contains(&ext.as_str()) {
            return Err(invalid(format!("不支持的图片格式 {ext}")));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(base64.as_bytes())
            .map_err(|e| invalid(format!("图片数据不对：{e}")))?;
        logs::mkdir(&self.images).map_err(|e| io(e.to_string()))?;
        let path = self
            .images
            .join(format!("{}-{}.{ext}", logs::now_ms(), self.next()));
        let mut f = logs::append(&path).map_err(|e| io(e.to_string()))?;
        f.write_all(&bytes).map_err(|e| io(e.to_string()))?;
        Ok(path.display().to_string())
    }
}

/// `sig` (SIGTERM, SIGKILL) to the connection's ccrpc while it still runs (ccrpc only detaches from
/// the Session in clay; the Session is not touched). Checked under the connection table's lock and
/// then the child's own: a ccrpc already reaped (its pid possibly reused) is never signalled. True
/// when sent.
fn terminate(
    conns: &Mutex<HashMap<String, Conn>>,
    conn: &str,
    sig: libc::c_int,
    why: &str,
) -> bool {
    let conns = conns.lock();
    let Some(c) = conns.get(conn) else {
        return false;
    };
    let mut child = c.child.lock();
    if !matches!(child.try_wait(), Ok(None)) {
        return false;
    }
    let pid = child.id();
    logs::line(
        &mut c.gui.lock(),
        &format!("signal {sig} to ccrpc pid={pid} ({why})"),
    );
    // SAFETY: plain kill(2) on our own child, not reaped yet (checked under its lock just now).
    unsafe { libc::kill(pid as libc::pid_t, sig) };
    true
}

/// A new Session's directory is named after its Holder ID once `ready` gives it. True once that
/// is settled (renamed, or it could not be).
fn rename_new_dir(line: &str, dir: &Arc<Mutex<PathBuf>>, wire: &Arc<Mutex<Wire>>) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return false;
    };
    if v["type"] != "ready" {
        return false;
    }
    // Used in a path: only a Holder ID of the expected form (as `connect` requires).
    let Some(h) = v["holder_id"].as_str().filter(|h| valid_holder_id(h)) else {
        return true;
    };
    let mut d = dir.lock();
    let Some(name) = d.file_name().and_then(|n| n.to_str()) else {
        return true;
    };
    let to = d.with_file_name(name.replacen("new", h, 1));
    if !to.exists() && std::fs::rename(&*d, &to).is_ok() {
        wire.lock().moved(&to);
        *d = to;
    }
    true
}

/// The last `n` characters of `s`.
fn last_chars(s: &str, n: usize) -> &str {
    if n == 0 {
        return "";
    }
    s.char_indices()
        .rev()
        .nth(n - 1)
        .map_or(s, |(i, _)| &s[i..])
}

/// `cmd`'s output, without stdin; killed (`TimedOut`) with everything it started when it has not
/// finished within `timeout`. It runs in its own process group, so a hung `toh` it launched goes
/// too (and with it the pipes the reader threads wait on).
fn output_within(mut cmd: Command, timeout: Duration) -> std::io::Result<Output> {
    use std::os::unix::process::CommandExt;
    let deadline = std::time::Instant::now() + timeout;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    let group = child.id() as libc::pid_t;
    let kill_group = |child: &mut std::process::Child| {
        // SAFETY: plain syscall on the group this function created.
        unsafe { libc::killpg(group, libc::SIGKILL) };
        let _ = child.wait();
    };
    let (mut out, mut err) = (child.stdout.take(), child.stderr.take());
    let (tx, rx) = std::sync::mpsc::channel();
    // Detached: after a kill, a grandchild may still hold the pipes open.
    std::thread::spawn(move || {
        let errs = std::thread::spawn(move || {
            let mut e = Vec::new();
            if let Some(err) = err.as_mut() {
                let _ = err.read_to_end(&mut e);
            }
            e
        });
        let mut o = Vec::new();
        if let Some(out) = out.as_mut() {
            let _ = out.read_to_end(&mut o);
        }
        let _ = tx.send((o, errs.join().unwrap_or_default()));
    });
    let Ok((stdout, stderr)) = rx.recv_timeout(timeout) else {
        kill_group(&mut child);
        return Err(std::io::ErrorKind::TimedOut.into());
    };
    // Its output is complete; it may still linger (closed its pipes without exiting).
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Output {
                status,
                stdout,
                stderr,
            });
        }
        if std::time::Instant::now() >= deadline {
            kill_group(&mut child);
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn valid_holder_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// `--hidden` when the setting asks for new Panes hidden.
fn hidden_arg(hidden: bool) -> Vec<String> {
    if hidden {
        vec!["--hidden".to_string()]
    } else {
        Vec::new()
    }
}

/// A Session ID as Claude Code writes it: a lowercase or uppercase UUID (`8-4-4-4-12` hex digits).
fn is_session_id(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    /// 「新建 Session」 and Resume hand the matching launch rule's arguments to `ccrpc new` after `--`.
    #[test]
    fn new_and_resume_pass_launch_rule_args_to_ccrpc() {
        let d = tempfile::tempdir().unwrap();
        let argv = d.path().join("argv");
        let fake = d.path().join("ccrpc");
        std::fs::write(
            &fake,
            format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\n", argv.display()),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cwd = d.path().join("work").join("app");
        std::fs::create_dir_all(&cwd).unwrap();
        let app = App::new(
            HashMap::new(),
            fake,
            d.path().join("logs"),
            d.path().join("img"),
        )
        .unwrap();
        let rules = [LaunchRule {
            dir: d.path().join("work").display().to_string(),
            args: "--add-dir '/x y'".into(),
        }];
        let (tx, rx) = std::sync::mpsc::channel();
        let sink: Sink = Arc::new(move |o| {
            if let Out::Exit(_) = o {
                let _ = tx.send(());
            }
        });
        let run = |rules: &[LaunchRule]| {
            app.new_session(sink.clone(), cwd.to_str().unwrap(), rules, false)
                .unwrap();
            rx.recv_timeout(Duration::from_secs(10)).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Ok(s) = std::fs::read_to_string(&argv) {
                    let _ = std::fs::remove_file(&argv);
                    // Skip `new --co-display --log <file>`.
                    return s.lines().skip(4).map(str::to_string).collect::<Vec<_>>();
                }
                assert!(Instant::now() < deadline, "fake ccrpc did not run");
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        let cwd_s = cwd.display().to_string();
        assert_eq!(run(&rules), ["--cwd", &cwd_s, "--", "--add-dir", "/x y"]);
        assert_eq!(run(&[]), ["--cwd", &cwd_s]);
        // 「新开的 Pane：隐藏」: `--hidden` before the rest.
        app.new_session(sink.clone(), cwd.to_str().unwrap(), &[], true)
            .unwrap();
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let args = loop {
            if let Ok(s) = std::fs::read_to_string(&argv) {
                let _ = std::fs::remove_file(&argv);
                break s.lines().skip(4).map(str::to_string).collect::<Vec<_>>();
            }
            assert!(Instant::now() < deadline, "fake ccrpc did not run");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(args, ["--hidden", "--cwd", &cwd_s]);

        // Resume: the rule's arguments, then `--resume <id>`; only a Session ID gets through.
        let id = "d30b178c-94da-426f-a80a-2f1ce4b4b40f";
        app.resume_session(sink.clone(), id, cwd.to_str().unwrap(), &rules, true)
            .unwrap();
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let args = loop {
            if let Ok(s) = std::fs::read_to_string(&argv) {
                break s.lines().skip(4).map(str::to_string).collect::<Vec<_>>();
            }
            assert!(Instant::now() < deadline, "fake ccrpc did not run");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(
            args,
            [
                "--hidden",
                "--cwd",
                &cwd_s,
                "--",
                "--add-dir",
                "/x y",
                "--resume",
                id
            ]
        );
        for bad in [
            "x; rm -rf ~",
            "d30b178c-94da-426f-a80a-2f1ce4b4b40",
            "d30b178c 94da-426f-a80a-2f1ce4b4b40f",
        ] {
            assert!(
                app.resume_session(sink.clone(), bad, cwd.to_str().unwrap(), &[], false)
                    .is_err()
            );
        }
    }

    fn fake_ccrpc(dir: &Path, script: &str) -> App {
        let fake = dir.join("ccrpc");
        std::fs::write(&fake, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        App::new(HashMap::new(), fake, dir.join("logs"), dir.join("img")).unwrap()
    }

    /// A hung `ccrpc ls` is killed and reported instead of stalling the Live poll for good.
    #[test]
    fn a_hung_subcommand_is_killed_after_the_timeout() {
        let d = tempfile::tempdir().unwrap();
        let mut app = fake_ccrpc(d.path(), "sleep 30");
        app.sub_timeout = Duration::from_millis(300);
        let t = Instant::now();
        let err = app.list_sessions().unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
        assert_eq!(err.code, ErrorCode::Io);
        assert!(err.message.contains("没有在"), "{err}");
        assert_eq!(app.doctor()[0].level, "fail");

        let app = fake_ccrpc(d.path(), r#"echo '{"sessions":[]}'"#);
        assert_eq!(
            app.list_sessions().unwrap(),
            serde_json::json!({"sessions": []})
        );
    }

    /// What a hung subcommand started goes with it: nothing keeps the pipes (and a reader
    /// thread) alive after the timeout.
    #[test]
    fn a_timed_out_subcommand_takes_its_children_with_it() {
        let d = tempfile::tempdir().unwrap();
        let pidfile = d.path().join("grandchild.pid");
        let mut app = fake_ccrpc(
            d.path(),
            &format!("sleep 30 &\necho $! > {}\nwait", pidfile.display()),
        );
        // Long enough for the script to start its child under load.
        app.sub_timeout = Duration::from_millis(1500);
        assert!(app.list_sessions().is_err());
        let pid: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        // SAFETY: signal 0 only checks existence.
        while unsafe { libc::kill(pid, 0) } == 0 {
            assert!(Instant::now() < deadline, "grandchild {pid} still running");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A failing subcommand's stderr: its first line is the message, all of it goes to gui.log;
    /// what the caller got wrong is `InvalidQuery`.
    #[test]
    fn errors_say_what_kind_and_one_line() {
        let d = tempfile::tempdir().unwrap();
        let app = fake_ccrpc(
            d.path(),
            "printf 'ccrpc: 找不到 Holder m1\\n  caused by: no such file\\n' >&2; exit 1",
        );
        let err = app.set_pane_hidden("m1", true).unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal);
        assert_eq!(err.message, "找不到 Holder m1");
        let log = std::fs::read_to_string(d.path().join("logs/gui.log")).unwrap();
        assert!(log.contains("caused by: no such file"), "{log}");
        let err = app.list_sessions().unwrap_err();
        assert_eq!(err.message, "找不到 Holder m1");

        assert_eq!(
            app.set_pane_hidden("x; rm", true).unwrap_err().code,
            ErrorCode::InvalidQuery
        );
        assert_eq!(
            app.save_image("AAAA", "exe").unwrap_err().code,
            ErrorCode::InvalidQuery
        );
        let gone = app.send("c404", "{}").unwrap_err();
        assert_eq!(
            (gone.code, gone.message.as_str()),
            (ErrorCode::InvalidQuery, "ccrpc 已退出")
        );
    }

    /// Waits for `conn`'s exit event; then checks no second one follows.
    fn exit_once(rx: &mpsc::Receiver<Out>, conn: &str, within: Duration) -> ExitEvent {
        let deadline = Instant::now() + within;
        let exit = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(Out::Exit(e)) if e.conn == conn => break e,
                Ok(_) => {}
                Err(_) => panic!("no exit event within {within:?}"),
            }
        };
        while let Ok(o) = rx.recv_timeout(Duration::from_millis(700)) {
            assert!(!matches!(o, Out::Exit(_)), "a second exit event");
        }
        exit
    }

    fn channel_sink() -> (Sink, mpsc::Receiver<Out>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        (Arc::new(move |o| drop(tx.lock().send(o))), rx)
    }

    /// Disconnect closes stdin; a ccrpc that neither reads it nor exits gets SIGTERM after the
    /// grace period, and its exit is reported once.
    #[test]
    fn disconnect_ends_a_ccrpc_that_does_not_exit() {
        let d = tempfile::tempdir().unwrap();
        let mut app = fake_ccrpc(d.path(), "echo '{\"type\":\"ready\"}'; exec sleep 30");
        app.disconnect_grace = Duration::from_millis(300);
        let (sink, rx) = channel_sink();
        let conn = app.connect(sink, "m1").unwrap();
        let t = Instant::now();
        assert!(app.disconnect(&conn));
        let exit = exit_once(&rx, &conn, Duration::from_secs(5));
        assert_eq!(exit.code, None, "ended by a signal");
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
        assert!(!app.disconnect(&conn), "the connection is gone");
    }

    /// A ccrpc that does not end on SIGTERM (stuck in a command) gets SIGKILL after the longer
    /// bound.
    #[test]
    fn a_ccrpc_ignoring_sigterm_is_killed_later() {
        let d = tempfile::tempdir().unwrap();
        let mut app = fake_ccrpc(d.path(), "trap '' TERM; exec sleep 30");
        app.disconnect_grace = Duration::from_millis(200);
        app.kill_after = Duration::from_millis(300);
        let (sink, rx) = channel_sink();
        let conn = app.connect(sink, "m1").unwrap();
        assert!(app.disconnect(&conn));
        let exit = exit_once(&rx, &conn, Duration::from_secs(5));
        assert_eq!(exit.code, None, "ended by a signal");
    }

    /// A `send` blocked on a full pipe holds ccrpc's stdin: Disconnect does not wait for it.
    #[test]
    fn disconnect_does_not_wait_for_a_stuck_send() {
        let d = tempfile::tempdir().unwrap();
        let app = fake_ccrpc(d.path(), "exec sleep 30");
        let (sink, rx) = channel_sink();
        let conn = app.connect(sink, "m1").unwrap();
        std::thread::scope(|s| {
            let stuck = s.spawn(|| app.send(&conn, &"x".repeat(1 << 20)));
            std::thread::sleep(Duration::from_millis(300));
            assert!(!stuck.is_finished(), "the pipe should be full");
            let t = Instant::now();
            assert!(app.disconnect(&conn));
            assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
            exit_once(&rx, &conn, Duration::from_secs(5));
            assert!(
                stuck.join().unwrap().is_err(),
                "the write fails once ccrpc is gone"
            );
        });
    }

    /// A slow sink still gets every line ccrpc wrote before exiting, then the exit.
    #[test]
    fn a_slow_sink_gets_the_last_lines_before_the_exit() {
        let d = tempfile::tempdir().unwrap();
        let app = fake_ccrpc(
            d.path(),
            "for i in 1 2 3 4; do echo \"{\\\"n\\\":$i}\"; done",
        );
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let sink: Sink = Arc::new(move |o| {
            if matches!(o, Out::Line { .. }) {
                std::thread::sleep(Duration::from_millis(600));
            }
            drop(tx.lock().send(o));
        });
        let conn = app.connect(sink, "m1").unwrap();
        let mut lines = Vec::new();
        let exit = loop {
            match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
                Out::Line { line, .. } => lines.push(line),
                Out::Exit(e) => break e,
            }
        };
        assert_eq!(exit.conn, conn);
        assert_eq!(
            lines,
            [r#"{"n":1}"#, r#"{"n":2}"#, r#"{"n":3}"#, r#"{"n":4}"#]
        );
    }

    /// The exit event does not wait for stdout to end: a process ccrpc left behind may hold it.
    #[test]
    fn exit_is_reported_while_a_leftover_process_holds_stdout() {
        let d = tempfile::tempdir().unwrap();
        let app = fake_ccrpc(d.path(), "echo '{\"type\":\"ready\"}'; sleep 20 & exit 3");
        let (sink, rx) = channel_sink();
        let conn = app.connect(sink, "m1").unwrap();
        let t = Instant::now();
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Out::Line { line, .. }) => assert_eq!(line, r#"{"type":"ready"}"#),
            _ => panic!("the line comes first"),
        }
        let exit = exit_once(&rx, &conn, Duration::from_secs(5));
        assert_eq!(exit.code, Some(3));
        assert!(t.elapsed() < Duration::from_secs(4), "{:?}", t.elapsed());
    }

    #[test]
    fn last_chars_counts_characters() {
        assert_eq!(last_chars("ab令牌桶", 3), "令牌桶");
        assert_eq!(last_chars("ab", 5), "ab");
        assert_eq!(last_chars("ab", 0), "");
    }
}
