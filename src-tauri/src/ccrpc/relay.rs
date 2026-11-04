//! The ccrpc processes, without Tauri: one `ccrpc` child per connection, its stdout relayed line
//! by line to a sink, everything written to the connection's logs (docs/spec.md §7).

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;

use super::logs::{self, Wire};
use crate::settings::LaunchRule;

pub type R<T> = Result<T, String>;

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
            .map_err(|e| format!("建不了日志目录 {}：{e}", log_root.display()))?;
        logs::prune(&log_root, chrono::Local::now().date_naive());
        logs::prune_files(&images);
        let gui =
            logs::append(&log_root.join("gui.log")).map_err(|e| format!("打不开 gui.log：{e}"))?;
        let app = App {
            env,
            ccrpc,
            log_root,
            images,
            conns: Arc::new(Mutex::new(HashMap::new())),
            gui: Arc::new(Mutex::new(gui)),
            seq: Mutex::new(0),
            doctor: Mutex::new(None),
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
        let d = match self.command().arg("doctor").stdin(Stdio::null()).output() {
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
                text: format!("fail\tccrpc\t运行不了 {}：{e}", self.ccrpc.display()),
            }],
        };
        for l in d.iter().filter(|l| l.level != "ok") {
            self.gui_line(&format!("doctor {}", l.text));
        }
        *self.doctor.lock() = Some(d.clone());
        d
    }

    pub fn list_sessions(&self) -> R<serde_json::Value> {
        let out = self
            .command()
            .args(["ls", "--json"])
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("运行不了 {}：{e}", self.ccrpc.display()))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        serde_json::from_slice(&out.stdout).map_err(|e| format!("ccrpc ls 的输出不是 JSON：{e}"))
    }

    /// `ccrpc hide|show <holder>`: hides the Pane on the ToH Mac Client, or shows it again (ToH ADR-0050).
    pub fn set_pane_hidden(&self, holder_id: &str, hidden: bool) -> R<()> {
        if !valid_holder_id(holder_id) {
            return Err("Holder ID 不对".into());
        }
        let verb = if hidden { "hide" } else { "show" };
        let out = self
            .command()
            .args([verb, holder_id])
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("运行不了 {}：{e}", self.ccrpc.display()))?;
        self.gui_line(&format!("{verb} {holder_id} → {}", out.status));
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr)
                .trim()
                .trim_start_matches("ccrpc: ")
                .to_string());
        }
        Ok(())
    }

    /// `ccrpc attach --co-display <holder>`.
    pub fn connect(&self, sink: Sink, holder_id: &str) -> R<String> {
        if !valid_holder_id(holder_id) {
            return Err("Holder ID 不对".into());
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
            return Err(format!("{cwd} 不是目录"));
        }
        let mut args = hidden_arg(hidden);
        args.extend(["--cwd".to_string(), cwd.to_string()]);
        let extra = super::launch::args_for(rules, Path::new(cwd), &self.env)?;
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
            return Err("Session ID 不对".into());
        }
        if !Path::new(cwd).is_dir() {
            return Err(format!("{cwd} 不是目录"));
        }
        let mut args = hidden_arg(hidden);
        args.extend(["--cwd".to_string(), cwd.to_string(), "--".into()]);
        args.extend(super::launch::args_for(rules, Path::new(cwd), &self.env)?);
        args.extend(["--resume".to_string(), session_id.to_string()]);
        // Named like a new Session's (`new` → its Holder ID once `ready` says it); gui.log has the arguments.
        self.spawn(sink, "new", &args, "new")
    }

    /// Starts ccrpc `sub` with `args` and relays it to `sink` as a new connection.
    fn spawn(&self, sink: Sink, sub: &str, args: &[String], dir_name: &str) -> R<String> {
        let id = format!("c{}", self.next());
        let dir =
            logs::conn_dir(&self.log_root, dir_name).map_err(|e| format!("建不了日志目录：{e}"))?;
        let wire = Arc::new(Mutex::new(
            Wire::open(dir.join("wire.jsonl")).map_err(|e| e.to_string())?,
        ));
        let gui = Arc::new(Mutex::new(
            logs::append(&dir.join("gui.log")).map_err(|e| e.to_string())?,
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
            .map_err(|e| format!("运行不了 {}：{e}", self.ccrpc.display()))?;
        let stdout = child.stdout.take().ok_or("没有 stdout")?;
        let mut stderr = child.stderr.take().ok_or("没有 stderr")?;
        self.gui_line(&format!(
            "{id} {sub} {} → {}",
            args.join(" "),
            dir.display()
        ));
        let dir = Arc::new(Mutex::new(dir));
        self.conns.lock().insert(
            id.clone(),
            Conn {
                stdin: Arc::new(Mutex::new(child.stdin.take())),
                wire: wire.clone(),
                gui: gui.clone(),
                dir: dir.clone(),
            },
        );

        let err_tail = Arc::new(Mutex::new(None::<String>));
        let err_thread = {
            let (err_tail, gui) = (err_tail.clone(), gui.clone());
            std::thread::spawn(move || {
                let mut s = String::new();
                let _ = stderr.read_to_string(&mut s);
                for l in s.lines() {
                    logs::line(&mut gui.lock(), &format!("stderr {l}"));
                }
                let n = s.chars().count();
                *err_tail.lock() = Some(s.chars().skip(n.saturating_sub(4000)).collect());
            })
        };
        let (conns, main_gui, conn) = (self.conns.clone(), self.gui.clone(), id.clone());
        let rename = sub == "new";
        std::thread::spawn(move || {
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
                wire.lock().write("recv", &line);
                if !renamed {
                    renamed = rename_new_dir(&line, &dir, &wire);
                }
                sink(Out::Line {
                    conn: conn.clone(),
                    line,
                });
            }
            let code = child.wait().ok().and_then(|s| s.code());
            let _ = err_thread.join();
            logs::line(&mut gui.lock(), &format!("exit code={code:?}"));
            logs::line(&mut main_gui.lock(), &format!("{conn} exit code={code:?}"));
            conns.lock().remove(&conn);
            let stderr = err_tail.lock().take().unwrap_or_default();
            sink(Out::Exit(ExitEvent { conn, code, stderr }));
        });
        Ok(id)
    }

    /// One command line to ccrpc (newlines would split it: replaced by spaces).
    pub fn send(&self, conn: &str, line: &str) -> R<()> {
        let (stdin, wire) = {
            let conns = self.conns.lock();
            let c = conns.get(conn).ok_or("ccrpc 已退出")?;
            (c.stdin.clone(), c.wire.clone())
        };
        let one = line.replace(['\n', '\r'], " ");
        let mut stdin = stdin.lock();
        let w = stdin.as_mut().ok_or("已断开")?;
        // Logged before it is written, so the response never precedes its command in the wire.
        wire.lock().write("send", &one);
        w.write_all(format!("{one}\n").as_bytes())
            .and_then(|_| w.flush())
            .map_err(|e| format!("写不进 ccrpc：{e}"))
    }

    /// Closes ccrpc's stdin: it detaches and exits; the Session keeps running.
    /// False when the connection is unknown (its ccrpc already exited).
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
        stdin.lock().take();
        true
    }

    /// Closes every connection (the app is quitting).
    pub fn disconnect_all(&self) {
        let all: Vec<_> = self
            .conns
            .lock()
            .values()
            .map(|c| c.stdin.clone())
            .collect();
        for s in all {
            s.lock().take();
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
            return Err("图片超过 20 MB".into());
        }
        if !["png", "jpg", "jpeg", "gif", "webp"].contains(&ext.as_str()) {
            return Err(format!("不支持的图片格式 {ext}"));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(base64.as_bytes())
            .map_err(|e| format!("图片数据不对：{e}"))?;
        logs::mkdir(&self.images).map_err(|e| e.to_string())?;
        let path = self
            .images
            .join(format!("{}-{}.{ext}", logs::now_ms(), self.next()));
        let mut f = logs::append(&path).map_err(|e| e.to_string())?;
        f.write_all(&bytes).map_err(|e| e.to_string())?;
        Ok(path.display().to_string())
    }
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
    let Some(h) = v["holder_id"]
        .as_str()
        .filter(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
    else {
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
}
