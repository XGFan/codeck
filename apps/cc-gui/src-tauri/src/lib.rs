//! cc-gui backend: Tauri commands over [`relay::App`] (the ccrpc processes and logs); ccrpc's
//! lines and exits go to the web view as `ccrpc-line` / `ccrpc-exit`. No protocol logic here.

pub mod env;
pub mod logs;
pub mod relay;

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, RunEvent, State};

use crate::relay::{App, DoctorLine, Out, R, Sink};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    ccrpc_path: String,
    doctor: Vec<DoctorLine>,
    analyzer: bool,
    log_root: String,
}

#[derive(Serialize, Clone)]
struct LineEvent {
    conn: String,
    line: String,
}

fn sink(handle: &AppHandle) -> Sink {
    let handle = handle.clone();
    Arc::new(move |o| {
        let _ = match o {
            Out::Line { conn, line } => handle.emit("ccrpc-line", LineEvent { conn, line }),
            Out::Exit(e) => handle.emit("ccrpc-exit", e),
        };
    })
}

#[tauri::command]
async fn app_info(app: State<'_, App>) -> R<AppInfo> {
    Ok(AppInfo {
        ccrpc_path: app.ccrpc.display().to_string(),
        doctor: app.doctor(),
        analyzer: app.env.get("CCRPC_ANALYZER_API_KEY").is_some_and(|k| !k.is_empty()),
        log_root: app.log_root.display().to_string(),
    })
}

#[tauri::command]
async fn list_sessions(app: State<'_, App>) -> R<serde_json::Value> {
    app.list_sessions()
}

#[tauri::command]
async fn connect(handle: AppHandle, app: State<'_, App>, holder_id: String) -> R<String> {
    app.connect(sink(&handle), &holder_id)
}

#[tauri::command]
async fn new_session(handle: AppHandle, app: State<'_, App>, cwd: String) -> R<String> {
    app.new_session(sink(&handle), &cwd)
}

#[tauri::command]
async fn send(app: State<'_, App>, conn: String, line: String) -> R<()> {
    app.send(&conn, &line)
}

#[tauri::command]
async fn disconnect(app: State<'_, App>, conn: String) -> R<()> {
    app.disconnect(&conn);
    Ok(())
}

#[tauri::command]
async fn gui_log(app: State<'_, App>, conn: Option<String>, text: String) -> R<()> {
    app.gui_log(conn.as_deref(), &text);
    Ok(())
}

/// The connection's log directory, `None` once its ccrpc has exited.
#[tauri::command]
async fn log_dir(app: State<'_, App>, conn: String) -> R<Option<String>> {
    Ok(app.log_dir(&conn).map(|p| p.display().to_string()))
}

#[tauri::command]
async fn save_image(app: State<'_, App>, base64: String, ext: String) -> R<String> {
    app.save_image(&base64, &ext)
}

pub fn run() {
    let env = env::login_env();
    let home = PathBuf::from(env.get("HOME").cloned().unwrap_or_default());
    let ccrpc = env::ccrpc_path(&env);
    let app = App::new(env, ccrpc, home.join("Library/Logs/cc-gui"), home.join("Library/Caches/cc-gui/images"))
        .expect("cc-gui: 启动失败");
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(app)
        .invoke_handler(tauri::generate_handler![
            app_info,
            list_sessions,
            connect,
            new_session,
            send,
            disconnect,
            gui_log,
            log_dir,
            save_image
        ])
        .build(tauri::generate_context!())
        .expect("cc-gui: 启动失败")
        .run(|handle, ev| {
            if let RunEvent::Exit = ev {
                handle.state::<App>().disconnect_all();
            }
        });
}
