//! Connecting to Claude Code through `ccrpc` (one child process per connection, its lines relayed as
//! Host events). The state is built lazily by the first command: reading the login shell's environment
//! at startup would slow CoDeck down.

pub mod env;
pub mod launch;
pub mod logs;
pub mod relay;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::Duration;

use parking_lot::Mutex;

use serde::Serialize;

use self::relay::{App, LsOut, LsSink, Out, Sink};
use crate::Host;
use crate::commands::{CmdResult, internal};
use crate::connect::{ConnectHub, Writer};
use crate::events::{EventSink, HostEvent};

/// How long the Host's own `ccrpc ls --json --watch` waits before starting again, at first and at most.
const LS_RETRY_MIN: Duration = Duration::from_secs(1);
const LS_RETRY_MAX: Duration = Duration::from_secs(60);

/// ccrpc, its logs and its pasted images, set up by the first command that needs them. A failed
/// setup is not kept: the next command tries again (e.g. after ccrpc is installed). `Arc` so a
/// command can take it into the blocking pool.
pub struct CcrpcState(Arc<Cell>);

struct Cell {
    app: OnceLock<App>,
    /// One setup at a time.
    init: Mutex<()>,
    paths: Paths,
}

/// Where ccrpc's logs and pasted images go; `None` = under the login shell's `HOME`, as the desktop app
/// has them.
struct Paths {
    logs: Option<PathBuf>,
    images: Option<PathBuf>,
}

impl Cell {
    fn get(&self) -> relay::R<&App> {
        if let Some(app) = self.app.get() {
            return Ok(app);
        }
        let _one = self.init.lock();
        if let Some(app) = self.app.get() {
            return Ok(app);
        }
        let app = init(&self.paths)?;
        Ok(self.app.get_or_init(|| app))
    }
}

impl CcrpcState {
    /// `logs` / `images`: see `HostConfig`.
    pub fn new(logs: Option<PathBuf>, images: Option<PathBuf>) -> Self {
        CcrpcState(Arc::new(Cell {
            app: OnceLock::new(),
            init: Mutex::new(()),
            paths: Paths { logs, images },
        }))
    }

    /// All connections' ccrpc processes are closed (the app is quitting); no-op when the state was
    /// never set up.
    pub fn disconnect_all(&self) {
        if let Some(app) = self.0.app.get() {
            app.disconnect_all();
        }
    }

    /// Writes a command line to a connection's ccrpc (for the Connect hub).
    pub(crate) fn writer(&self) -> Writer {
        let cell = self.0.clone();
        Arc::new(move |conn, line| cell.get()?.send(conn, line))
    }

    /// Keeps `ccrpc ls --json --watch` running for `hub` (started again after it ends or cannot start, waiting
    /// longer each time it failed without a list), its lists also as events, until `hub` is stopped or
    /// [`Self::stop_keeping`].
    pub(crate) fn keep_watching(&self, hub: Arc<ConnectHub>, events: EventSink) {
        let cell = self.0.clone();
        let generation = hub.begin_keep();
        std::thread::spawn(move || {
            let mut wait = LS_RETRY_MIN;
            while hub.keep_current(generation) {
                let listed = Arc::new(AtomicBool::new(false));
                let (ended_tx, ended) = mpsc::channel::<()>();
                let sink: LsSink = {
                    let (hub, events, listed) = (hub.clone(), events.clone(), listed.clone());
                    Arc::new(move |o| match o {
                        _ if !hub.keep_current(generation) => {}
                        LsOut::List(ls) => {
                            listed.store(true, Ordering::SeqCst);
                            hub.set_ls(Ok(ls.clone()));
                            events(HostEvent::CcrpcLs { ls });
                        }
                        LsOut::Ended(message) => {
                            hub.set_ls(Err(message.clone()));
                            events(HostEvent::CcrpcLsKeptEnded { message });
                            let _ = ended_tx.send(());
                        }
                    })
                };
                // Setting up may run a login shell: outside the lock of `start_if_current`.
                let started = cell.get().and_then(|app| {
                    hub.start_if_current(generation, || app.watch_sessions(true, sink))
                });
                match started {
                    // Until it ends (or is stopped, or was not started: the sink is dropped).
                    Ok(()) => {
                        let _ = ended.recv();
                    }
                    Err(e) if hub.keep_current(generation) => hub.set_ls(Err(e.message)),
                    Err(_) => {}
                }
                if listed.load(Ordering::SeqCst) {
                    wait = LS_RETRY_MIN;
                }
                std::thread::sleep(wait);
                wait = (wait * 2).min(LS_RETRY_MAX);
            }
        });
    }

    /// Ends [`Self::keep_watching`]: its watch stops, and the front end's starts again if it still wants one.
    pub(crate) async fn stop_keeping(&self, hub: Arc<ConnectHub>, events: EventSink) {
        let cell = self.0.clone();
        let done = tokio::task::spawn_blocking(move || {
            // Under `end_keep`'s lock, like the loop's start. No app yet: nothing was started, and the loop's setup
            // starts nothing once its generation is over.
            hub.end_keep(
                || {
                    if let Some(app) = cell.app.get() {
                        let _ = app.watch_sessions(false, ls_sink(events.clone()));
                    }
                },
                || {
                    if let Some(app) = cell.app.get()
                        && let Err(e) = app.watch_sessions(true, ls_sink(events.clone()))
                    {
                        events(HostEvent::CcrpcLsEnded { message: e.message });
                    }
                },
            );
        })
        .await;
        if let Err(e) = done {
            tracing::warn!("stopping the ls watch failed: {e}");
        }
    }
}

/// `ccrpc ls --json --watch`'s output, as Host events.
fn ls_sink(events: EventSink) -> LsSink {
    Arc::new(move |o| {
        events(match o {
            LsOut::List(ls) => HostEvent::CcrpcLs { ls },
            LsOut::Ended(message) => HostEvent::CcrpcLsEnded { message },
        })
    })
}

fn init(paths: &Paths) -> relay::R<App> {
    let env = env::login_env();
    let home = PathBuf::from(env.get("HOME").cloned().unwrap_or_default());
    let ccrpc = env::ccrpc_path(&env);
    let logs = paths
        .logs
        .clone()
        .unwrap_or_else(|| home.join("Library/Logs/dev.joy.codeck/ccrpc"));
    let images = paths
        .images
        .clone()
        .unwrap_or_else(|| home.join("Library/Caches/dev.joy.codeck/images"));
    App::new(env, ccrpc, logs, images)
}

/// ccrpc's lines and exits, as Host events (through the Connect hub when there is one).
fn sink(events: &EventSink, hub: Option<&Arc<ConnectHub>>) -> Sink {
    if let Some(hub) = hub {
        return hub.sink();
    }
    let events = events.clone();
    Arc::new(move |o| {
        events(match o {
            Out::Line { conn, line } => HostEvent::CcrpcLine { conn, line },
            Out::Exit(e) => HostEvent::CcrpcExit {
                conn: e.conn,
                code: e.code,
                stderr: e.stderr,
            },
        })
    })
}

#[derive(Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CcrpcInfo {
    pub ccrpc_path: String,
    pub doctor: Vec<relay::DoctorLine>,
    pub analyzer: AnalyzerInfo,
    pub launch_rules: Vec<crate::settings::LaunchRule>,
    /// Sessions CoDeck starts open hidden on the ToH Mac Client.
    pub hide_new_panes: bool,
    pub log_root: String,
}

/// ccrpc's screen analyzer (an Anthropic Messages endpoint): what 「设置 › ccrpc」 saved, and what the next
/// ccrpc will get (settings over the login shell's `CCRPC_ANALYZER_*`).
#[derive(Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzerInfo {
    /// A key reaches ccrpc: the analyzer is on.
    pub enabled: bool,
    /// Where that key comes from: `settings` or `shell`; null when there is none.
    pub key_source: Option<String>,
    /// The key's last 4 characters.
    pub key_hint: Option<String>,
    /// Saved in the settings (null: not set here).
    pub base_url: Option<String>,
    pub model: Option<String>,
    /// From the login shell (null: not set there; ccrpc's default then applies).
    pub shell_base_url: Option<String>,
    pub shell_model: Option<String>,
}

fn analyzer_info(app: &App) -> AnalyzerInfo {
    let saved = crate::settings::load().analyzer;
    let filled = |v: &Option<String>| {
        v.as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let shell = |k: &str| {
        app.env
            .get(k)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let (key, key_source) = match (filled(&saved.api_key), shell("CCRPC_ANALYZER_API_KEY")) {
        (Some(k), _) => (Some(k), Some("settings".to_string())),
        (None, Some(k)) => (Some(k), Some("shell".to_string())),
        (None, None) => (None, None),
    };
    AnalyzerInfo {
        enabled: key.is_some(),
        key_hint: key.map(|k| {
            k.chars()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect()
        }),
        key_source,
        base_url: filled(&saved.base_url),
        model: filled(&saved.model),
        shell_base_url: shell("CCRPC_ANALYZER_BASE_URL"),
        shell_model: shell("CCRPC_ANALYZER_MODEL"),
    }
}

impl Host {
    /// Runs `f` with the ccrpc state, initialized on first use, on the blocking pool.
    async fn with_ccrpc<T, F>(&self, f: F) -> CmdResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&App) -> relay::R<T> + Send + 'static,
    {
        let cell = self.ccrpc.0.clone();
        tokio::task::spawn_blocking(move || f(cell.get()?))
            .await
            .map_err(internal)?
    }

    /// Saves the analyzer's settings (blank = not set here). `api_key: None` keeps the saved key; `Some("")`
    /// removes it. Applies to the connections made from now on.
    pub async fn ccrpc_set_analyzer(
        &self,
        api_key: Option<String>,
        base_url: Option<String>,
        model: Option<String>,
    ) -> CmdResult<AnalyzerInfo> {
        self.with_ccrpc(move |app| {
            let blank =
                |v: Option<String>| v.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
            crate::settings::update(|s| {
                if let Some(k) = api_key {
                    s.analyzer.api_key = blank(Some(k));
                }
                s.analyzer.base_url = blank(base_url);
                s.analyzer.model = blank(model);
            })
            .map_err(|e| relay::io(format!("保存设置失败：{e}")))?;
            app.gui_line("analyzer settings saved");
            Ok(analyzer_info(app))
        })
        .await
    }

    /// Tests the analyzer with these values (as `ccrpc_set_analyzer` takes them, `api_key: None` = the saved
    /// key) without saving them: `ccrpc analyzer test`, one analysis of a built-in dialog.
    pub async fn ccrpc_test_analyzer(
        &self,
        api_key: Option<String>,
        base_url: Option<String>,
        model: Option<String>,
    ) -> CmdResult<relay::AnalyzerTest> {
        self.with_ccrpc(move |app| {
            let mut a = crate::settings::load().analyzer;
            if let Some(k) = api_key {
                a.api_key = Some(k);
            }
            a.base_url = base_url;
            a.model = model;
            let r = app.test_analyzer(&a);
            app.gui_line(&match &r {
                Ok(t) => format!("analyzer test ok: {} {} ms", t.model, t.elapsed_ms),
                Err(e) => format!("analyzer test failed: {}", e.message),
            });
            r
        })
        .await
    }

    /// ccrpc's analysis records (`ccrpc analyzer records`).
    pub async fn ccrpc_analyzer_records(&self) -> CmdResult<relay::AnalyzerRecords> {
        self.with_ccrpc(|app| app.analyzer_records()).await
    }

    /// Opens the analysis records' directory in Finder (ccrpc says where it is and creates it).
    pub async fn ccrpc_open_analyzer_records(&self) -> CmdResult<()> {
        let open = self.config.open_path.clone();
        self.with_ccrpc(move |app| {
            let dir = app.analyzer_records()?.dir;
            let open = open.ok_or_else(|| relay::io("这个 Host 不能打开目录"))?;
            open(std::path::Path::new(&dir))
                .map_err(|e| relay::io(format!("打不开分析记录目录：{e}")))
        })
        .await
    }

    /// Saves the launch rules (trimmed; rules without arguments dropped) and returns what was saved.
    pub async fn ccrpc_set_launch_rules(
        &self,
        rules: Vec<crate::settings::LaunchRule>,
    ) -> CmdResult<Vec<crate::settings::LaunchRule>> {
        self.with_ccrpc(move |app| {
            let rules = launch::normalize(rules, &app.env).map_err(relay::invalid)?;
            crate::settings::update(|s| s.launch_rules = rules.clone())
                .map_err(|e| relay::io(format!("保存设置失败：{e}")))?;
            app.gui_line(&format!("launch rules saved ({})", rules.len()));
            Ok(rules)
        })
        .await
    }

    /// Saves 「新开的 Pane」: whether the Sessions CoDeck starts from now on open hidden on the ToH Mac Client.
    pub async fn ccrpc_set_hide_new_panes(&self, hide: bool) -> CmdResult<bool> {
        self.with_ccrpc(move |app| {
            crate::settings::update(|s| s.hide_new_panes = hide)
                .map_err(|e| relay::io(format!("保存设置失败：{e}")))?;
            app.gui_line(&format!("hide new panes: {hide}"));
            Ok(hide)
        })
        .await
    }

    /// Hides a clay Session's Pane on the ToH Mac Client, or shows it again (`ccrpc hide|show`).
    pub async fn ccrpc_set_pane_hidden(&self, holder_id: String, hidden: bool) -> CmdResult<()> {
        self.with_ccrpc(move |app| app.set_pane_hidden(&holder_id, hidden))
            .await
    }

    pub async fn ccrpc_info(&self) -> CmdResult<CcrpcInfo> {
        self.with_ccrpc(|app| {
            Ok(CcrpcInfo {
                ccrpc_path: app.ccrpc.display().to_string(),
                doctor: app.refresh_doctor(),
                analyzer: analyzer_info(app),
                launch_rules: crate::settings::load().launch_rules,
                hide_new_panes: crate::settings::load().hide_new_panes,
                log_root: app.log_root.display().to_string(),
            })
        })
        .await
    }

    /// Starts (`on`) or stops `ccrpc ls --json --watch`; its lists come as `CcrpcLsEvent`, its end by
    /// itself as `CcrpcLsEndedEvent`. Starting again restarts it (the current list comes at once).
    pub async fn ccrpc_watch_sessions(&self, on: bool) -> CmdResult<()> {
        let events = self.events.clone();
        let sink = ls_sink(events.clone());
        let hub = self.connect.clone();
        self.with_ccrpc(move |app| {
            let Some(hub) = hub else {
                return app.watch_sessions(on, sink);
            };
            if let Some(ls) = hub.front_end_watch(on, || app.watch_sessions(on, sink))? {
                events(HostEvent::CcrpcLs { ls });
            }
            Ok(())
        })
        .await
    }

    /// With Connect state, the connection the Host already has to `holder_id`, whoever started it.
    pub async fn ccrpc_connect(&self, holder_id: String) -> CmdResult<String> {
        match self.connect.clone() {
            Some(hub) => self.connect_shared(&hub, holder_id).await,
            None => self.attach(holder_id).await,
        }
    }

    /// `ccrpc attach`: a new connection to `holder_id`.
    pub(crate) async fn attach(&self, holder_id: String) -> CmdResult<String> {
        let sink = sink(&self.events, self.connect.as_ref());
        let holder = holder_id.clone();
        let conn = self
            .with_ccrpc(move |app| app.connect(sink, &holder_id))
            .await?;
        self.registered(&conn, Some(holder));
        Ok(conn)
    }

    /// A connection was started: its Connect state (when tracked) exists from now on.
    fn registered(&self, conn: &str, holder_id: Option<String>) {
        if let Some(hub) = &self.connect {
            hub.register(conn, holder_id);
        }
    }

    pub async fn ccrpc_new(&self, cwd: String) -> CmdResult<String> {
        let sink = sink(&self.events, self.connect.as_ref());
        let conn = self
            .with_ccrpc(move |app| {
                let s = crate::settings::load();
                app.new_session(sink, &cwd, &s.launch_rules, s.hide_new_panes)
            })
            .await?;
        self.registered(&conn, None);
        Ok(conn)
    }

    /// Resume: starts the ended Session `session_id` again in a new Pane (`claude --resume`) and connects to it.
    pub async fn ccrpc_resume(&self, session_id: String, cwd: String) -> CmdResult<String> {
        // Checked again here, not from the last poll: two claude processes would write one transcript.
        if self.state.engine().is_live(&session_id) {
            return Err(relay::invalid("Session 仍在运行"));
        }
        let sink = sink(&self.events, self.connect.as_ref());
        let conn = self
            .with_ccrpc(move |app| {
                let s = crate::settings::load();
                app.resume_session(sink, &session_id, &cwd, &s.launch_rules, s.hide_new_panes)
            })
            .await?;
        self.registered(&conn, None);
        Ok(conn)
    }

    /// Fork: starts a Fork of the Session `session_id` in a new Pane (before the Main Line prompt `at`, or
    /// all of it), named `title`, and connects to it; the result comes as ccrpc's `fork` event.
    pub async fn ccrpc_fork(
        &self,
        session_id: String,
        cwd: String,
        at: Option<String>,
        restore: Option<String>,
        title: String,
    ) -> CmdResult<String> {
        let sink = sink(&self.events, self.connect.as_ref());
        let f = relay::ForkArgs {
            session_id,
            cwd,
            at,
            restore,
            title,
        };
        let conn = self
            .with_ccrpc(move |app| {
                let s = crate::settings::load();
                app.fork_session(sink, &f, &s.launch_rules, s.hide_new_panes)
            })
            .await?;
        self.registered(&conn, None);
        Ok(conn)
    }

    pub async fn ccrpc_send(&self, conn: String, line: String) -> CmdResult<()> {
        self.with_ccrpc(move |app| app.send(&conn, &line)).await
    }

    pub async fn ccrpc_disconnect(&self, conn: String) -> CmdResult<()> {
        // Before ccrpc is asked to go: its exit's final state then says it was a Disconnect.
        if let Some(hub) = &self.connect {
            hub.mark_disconnected(&conn);
        }
        self.with_ccrpc(move |app| {
            if app.disconnect(&conn) {
                Ok(())
            } else {
                Err(relay::invalid("ccrpc 已退出"))
            }
        })
        .await
    }

    pub async fn ccrpc_gui_log(&self, conn: Option<String>, text: String) -> CmdResult<()> {
        self.with_ccrpc(move |app| {
            app.gui_log(conn.as_deref(), &text);
            Ok(())
        })
        .await
    }

    /// The connection's log directory, `None` once its ccrpc has exited.
    pub async fn ccrpc_log_dir(&self, conn: String) -> CmdResult<Option<String>> {
        self.with_ccrpc(move |app| Ok(app.log_dir(&conn).map(|p| p.display().to_string())))
            .await
    }

    pub async fn ccrpc_save_image(&self, base64: String, ext: String) -> CmdResult<String> {
        self.with_ccrpc(move |app| app.save_image(&base64, &ext))
            .await
    }
}
