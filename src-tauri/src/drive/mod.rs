//! Driving Claude Code through `ccrpc` (one child process per connection, its lines relayed to the
//! web view). The state is built lazily by the first command: reading the login shell's environment
//! at startup would slow CoDeck down.

pub mod env;
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
use crate::events::{DriveExitEvent, DriveLineEvent};

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
pub struct DriveState(Arc<Cell>);

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

impl DriveState {
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
        home.join("Library/Logs/dev.joy.codeck/drive"),
        home.join("Library/Caches/dev.joy.codeck/images"),
    )
}

/// Runs `f` with the drive state, initialized on first use, on the blocking pool.
async fn with_drive<T, F>(state: &State<'_, DriveState>, f: F) -> CmdResult<T>
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
            Out::Line { conn, line } => DriveLineEvent { conn, line }.emit(&handle),
            Out::Exit(e) => DriveExitEvent { conn: e.conn, code: e.code, stderr: e.stderr }.emit(&handle),
        };
    })
}

#[derive(Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DriveInfo {
    pub ccrpc_path: String,
    pub doctor: Vec<relay::DoctorLine>,
    pub analyzer: AnalyzerInfo,
    pub log_root: String,
}

/// ccrpc's screen analyzer (an Anthropic Messages endpoint): what 「设置 › 驾驶」 saved, and what the next
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
    let filled = |v: &Option<String>| v.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
    let shell = |k: &str| app.env.get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let (key, key_source) = match (filled(&saved.api_key), shell("CCRPC_ANALYZER_API_KEY")) {
        (Some(k), _) => (Some(k), Some("settings".to_string())),
        (None, Some(k)) => (Some(k), Some("shell".to_string())),
        (None, None) => (None, None),
    };
    AnalyzerInfo {
        enabled: key.is_some(),
        key_hint: key.map(|k| k.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect()),
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
pub async fn drive_set_analyzer(
    state: State<'_, DriveState>,
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
) -> CmdResult<AnalyzerInfo> {
    with_drive(&state, move |app| {
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

#[tauri::command]
#[specta::specta]
pub async fn drive_info(state: State<'_, DriveState>) -> CmdResult<DriveInfo> {
    with_drive(&state, |app| {
        Ok(DriveInfo {
            ccrpc_path: app.ccrpc.display().to_string(),
            doctor: app.refresh_doctor(),
            analyzer: analyzer_info(app),
            log_root: app.log_root.display().to_string(),
        })
    })
    .await
}

/// `ccrpc ls --json`, serialized back to its text (the front end parses it).
#[tauri::command]
#[specta::specta]
pub async fn drive_list(state: State<'_, DriveState>) -> CmdResult<String> {
    with_drive(&state, |app| {
        let v = app.list_sessions()?;
        serde_json::to_string(&v).map_err(|e| format!("ccrpc ls 的输出不是 JSON：{e}"))
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn drive_connect(handle: AppHandle, state: State<'_, DriveState>, holder_id: String) -> CmdResult<String> {
    let sink = sink(&handle);
    with_drive(&state, move |app| app.connect(sink, &holder_id)).await
}

#[tauri::command]
#[specta::specta]
pub async fn drive_new(handle: AppHandle, state: State<'_, DriveState>, cwd: String) -> CmdResult<String> {
    let sink = sink(&handle);
    with_drive(&state, move |app| app.new_session(sink, &cwd)).await
}

#[tauri::command]
#[specta::specta]
pub async fn drive_send(state: State<'_, DriveState>, conn: String, line: String) -> CmdResult<()> {
    with_drive(&state, move |app| app.send(&conn, &line)).await
}

#[tauri::command]
#[specta::specta]
pub async fn drive_disconnect(state: State<'_, DriveState>, conn: String) -> CmdResult<()> {
    with_drive(&state, move |app| if app.disconnect(&conn) { Ok(()) } else { Err("ccrpc 已退出".into()) })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn drive_gui_log(state: State<'_, DriveState>, conn: Option<String>, text: String) -> CmdResult<()> {
    with_drive(&state, move |app| {
        app.gui_log(conn.as_deref(), &text);
        Ok(())
    })
    .await
}

/// The connection's log directory, `None` once its ccrpc has exited.
#[tauri::command]
#[specta::specta]
pub async fn drive_log_dir(state: State<'_, DriveState>, conn: String) -> CmdResult<Option<String>> {
    with_drive(&state, move |app| Ok(app.log_dir(&conn).map(|p| p.display().to_string()))).await
}

#[tauri::command]
#[specta::specta]
pub async fn drive_save_image(state: State<'_, DriveState>, base64: String, ext: String) -> CmdResult<String> {
    with_drive(&state, move |app| app.save_image(&base64, &ext)).await
}
