//! Connecting to Claude Code through `ccrpc` (one child process per connection, its lines relayed to the
//! web view). The state is built lazily by the first command: reading the login shell's environment
//! at startup would slow CoDeck down.

pub mod env;
pub mod launch;
pub mod logs;
pub mod relay;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;

use codeck_core::model::{AppError, ErrorCode};
use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_specta::Event;

use self::relay::{App, Out, Sink};
use crate::events::{CcrpcExitEvent, CcrpcLineEvent};
use crate::state::AppState;

type CmdResult<T> = Result<T, AppError>;

fn internal(e: impl std::fmt::Display) -> AppError {
    AppError {
        code: ErrorCode::Internal,
        message: e.to_string(),
    }
}

/// ccrpc, its logs and its pasted images, set up by the first command that needs them. A failed
/// setup is not kept: the next command tries again (e.g. after ccrpc is installed). `Arc` so a
/// command can take it into the blocking pool.
#[derive(Default)]
pub struct CcrpcState(Arc<Cell>);

#[derive(Default)]
struct Cell {
    app: OnceLock<App>,
    /// One setup at a time.
    init: Mutex<()>,
}

impl Cell {
    fn get(&self) -> Result<&App, String> {
        if let Some(app) = self.app.get() {
            return Ok(app);
        }
        let _one = self.init.lock();
        if let Some(app) = self.app.get() {
            return Ok(app);
        }
        let app = init()?;
        Ok(self.app.get_or_init(|| app))
    }
}

impl CcrpcState {
    /// All connections' ccrpc processes are closed (the app is quitting); no-op when the state was
    /// never set up.
    pub fn disconnect_all(&self) {
        if let Some(app) = self.0.app.get() {
            app.disconnect_all();
        }
    }
}

fn init() -> Result<App, String> {
    let env = env::login_env();
    let home = PathBuf::from(env.get("HOME").cloned().unwrap_or_default());
    let ccrpc = env::ccrpc_path(&env);
    App::new(
        env,
        ccrpc,
        home.join("Library/Logs/dev.joy.codeck/ccrpc"),
        home.join("Library/Caches/dev.joy.codeck/images"),
    )
}

/// Runs `f` with the ccrpc state, initialized on first use, on the blocking pool.
async fn with_ccrpc<T, F>(state: &State<'_, CcrpcState>, f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce(&App) -> Result<T, String> + Send + 'static,
{
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || match cell.get() {
        Ok(app) => f(app).map_err(internal),
        Err(e) => Err(internal(e)),
    })
    .await
    .map_err(internal)?
}

/// ccrpc's lines and exits, as typed events.
fn sink(handle: &AppHandle) -> Sink {
    let handle = handle.clone();
    Arc::new(move |o| {
        let _ = match o {
            Out::Line { conn, line } => CcrpcLineEvent { conn, line }.emit(&handle),
            Out::Exit(e) => CcrpcExitEvent {
                conn: e.conn,
                code: e.code,
                stderr: e.stderr,
            }
            .emit(&handle),
        };
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

/// Saves the analyzer's settings (blank = not set here). `api_key: None` keeps the saved key; `Some("")`
/// removes it. Applies to the connections made from now on.
#[tauri::command]
#[specta::specta]
pub async fn ccrpc_set_analyzer(
    state: State<'_, CcrpcState>,
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
) -> CmdResult<AnalyzerInfo> {
    with_ccrpc(&state, move |app| {
        let mut s = crate::settings::load();
        let blank = |v: Option<String>| v.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        if let Some(k) = api_key {
            s.analyzer.api_key = blank(Some(k));
        }
        s.analyzer.base_url = blank(base_url);
        s.analyzer.model = blank(model);
        crate::settings::save(&s).map_err(|e| format!("保存设置失败：{e}"))?;
        app.gui_line("analyzer settings saved");
        Ok(analyzer_info(app))
    })
    .await
}

/// Saves the launch rules (trimmed; rules without arguments dropped) and returns what was saved.
#[tauri::command]
#[specta::specta]
pub async fn ccrpc_set_launch_rules(
    state: State<'_, CcrpcState>,
    rules: Vec<crate::settings::LaunchRule>,
) -> CmdResult<Vec<crate::settings::LaunchRule>> {
    with_ccrpc(&state, move |app| {
        let rules = launch::normalize(rules, &app.env)?;
        let mut s = crate::settings::load();
        s.launch_rules = rules.clone();
        crate::settings::save(&s).map_err(|e| format!("保存设置失败：{e}"))?;
        app.gui_line(&format!("launch rules saved ({})", rules.len()));
        Ok(rules)
    })
    .await
}

/// Saves 「新开的 Pane」: whether the Sessions CoDeck starts from now on open hidden on the ToH Mac Client.
#[tauri::command]
#[specta::specta]
pub async fn ccrpc_set_hide_new_panes(state: State<'_, CcrpcState>, hide: bool) -> CmdResult<bool> {
    with_ccrpc(&state, move |app| {
        let mut s = crate::settings::load();
        s.hide_new_panes = hide;
        crate::settings::save(&s).map_err(|e| format!("保存设置失败：{e}"))?;
        app.gui_line(&format!("hide new panes: {hide}"));
        Ok(hide)
    })
    .await
}

/// Hides a clay Session's Pane on the ToH Mac Client, or shows it again (`ccrpc hide|show`).
#[tauri::command]
#[specta::specta]
pub async fn ccrpc_set_pane_hidden(
    state: State<'_, CcrpcState>,
    holder_id: String,
    hidden: bool,
) -> CmdResult<()> {
    with_ccrpc(&state, move |app| app.set_pane_hidden(&holder_id, hidden)).await
}

#[tauri::command]
#[specta::specta]
pub async fn ccrpc_info(state: State<'_, CcrpcState>) -> CmdResult<CcrpcInfo> {
    with_ccrpc(&state, |app| {
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

/// `ccrpc ls --json`, serialized back to its text (the front end parses it).
#[tauri::command]
#[specta::specta]
pub async fn ccrpc_list(state: State<'_, CcrpcState>) -> CmdResult<String> {
    with_ccrpc(&state, |app| {
        let v = app.list_sessions()?;
        serde_json::to_string(&v).map_err(|e| format!("ccrpc ls 的输出不是 JSON：{e}"))
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn ccrpc_connect(
    handle: AppHandle,
    state: State<'_, CcrpcState>,
    holder_id: String,
) -> CmdResult<String> {
    let sink = sink(&handle);
    with_ccrpc(&state, move |app| app.connect(sink, &holder_id)).await
}

#[tauri::command]
#[specta::specta]
pub async fn ccrpc_new(
    handle: AppHandle,
    state: State<'_, CcrpcState>,
    cwd: String,
) -> CmdResult<String> {
    let sink = sink(&handle);
    with_ccrpc(&state, move |app| {
        let s = crate::settings::load();
        app.new_session(sink, &cwd, &s.launch_rules, s.hide_new_panes)
    })
    .await
}

/// Resume: starts the ended Session `session_id` again in a new Pane (`claude --resume`) and connects to it.
#[tauri::command]
#[specta::specta]
pub async fn ccrpc_resume(
    handle: AppHandle,
    state: State<'_, CcrpcState>,
    app_state: State<'_, AppState>,
    session_id: String,
    cwd: String,
) -> CmdResult<String> {
    // Checked again here, not from the last poll: two claude processes would write one transcript.
    if app_state.engine().is_live(&session_id) {
        return Err(internal("Session 仍在运行"));
    }
    let sink = sink(&handle);
    with_ccrpc(&state, move |app| {
        let s = crate::settings::load();
        app.resume_session(sink, &session_id, &cwd, &s.launch_rules, s.hide_new_panes)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn ccrpc_send(state: State<'_, CcrpcState>, conn: String, line: String) -> CmdResult<()> {
    with_ccrpc(&state, move |app| app.send(&conn, &line)).await
}

#[tauri::command]
#[specta::specta]
pub async fn ccrpc_disconnect(state: State<'_, CcrpcState>, conn: String) -> CmdResult<()> {
    with_ccrpc(&state, move |app| {
        if app.disconnect(&conn) {
            Ok(())
        } else {
            Err("ccrpc 已退出".into())
        }
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn ccrpc_gui_log(
    state: State<'_, CcrpcState>,
    conn: Option<String>,
    text: String,
) -> CmdResult<()> {
    with_ccrpc(&state, move |app| {
        app.gui_log(conn.as_deref(), &text);
        Ok(())
    })
    .await
}

/// The connection's log directory, `None` once its ccrpc has exited.
#[tauri::command]
#[specta::specta]
pub async fn ccrpc_log_dir(
    state: State<'_, CcrpcState>,
    conn: String,
) -> CmdResult<Option<String>> {
    with_ccrpc(&state, move |app| {
        Ok(app.log_dir(&conn).map(|p| p.display().to_string()))
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn ccrpc_save_image(
    state: State<'_, CcrpcState>,
    base64: String,
    ext: String,
) -> CmdResult<String> {
    with_ccrpc(&state, move |app| app.save_image(&base64, &ext)).await
}
