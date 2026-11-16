//! The terminal window (ADR-0015): for a screen ccrpc cannot read, a window of its own showing the
//! Connected Session's Pane, attached straight to its ToH Holder. Transitional and self-contained: the
//! app registers [`init`], the front end opens `terminal.html` through it; removing it touches nothing else.
//!
//! The main window opens, closes and lists terminal windows (one per Pane, label `term-<Holder ID>`) and
//! tells one what the Session is doing ([`STATUS_EVENT`]); a terminal window attaches to its own Pane only.

pub mod holder;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::plugin::{Builder, TauriPlugin};
use tauri::{
    AppHandle, Emitter, Manager, RunEvent, Runtime, State, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

use crate::holder::Link;

/// The event a terminal window gets with the Session's status (`"screen"` / `"input"` / `""`).
pub const STATUS_EVENT: &str = "codeck-term:status";
const PREFIX: &str = "term-";

#[derive(Default)]
struct Links {
    env: OnceLock<HashMap<String, String>>,
    /// Attached connections, by terminal window label.
    by_window: Mutex<HashMap<String, Link>>,
    statuses: Statuses,
}

impl Links {
    fn take(&self, label: &str) -> Option<Link> {
        self.by_window
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(label)
    }
}

/// The last status the main window set for each terminal window: a window that attaches after it
/// was sent (just opened, or reloaded) gets it from `attach`.
#[derive(Default)]
struct Statuses(Mutex<HashMap<String, String>>);

impl Statuses {
    fn set(&self, label: &str, status: &str) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(label.to_string(), status.to_string());
    }

    fn get(&self, label: &str) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(label)
            .cloned()
    }

    fn forget(&self, label: &str) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(label);
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("codeck-term")
        .invoke_handler(tauri::generate_handler![
            open_window,
            close_window,
            set_status,
            windows,
            attach,
            input
        ])
        .setup(|app, _| {
            app.manage(Links::default());
            Ok(())
        })
        .on_event(|app, event| {
            let RunEvent::WindowEvent {
                label,
                event: WindowEvent::Destroyed,
                ..
            } = event
            else {
                return;
            };
            let links = app.state::<Links>();
            drop(links.take(label));
            links.statuses.forget(label);
            // Without the main window nothing keeps them in step: they go with it.
            if label == "main" {
                for (l, w) in app.webview_windows() {
                    if l.starts_with(PREFIX) {
                        let _ = w.close();
                    }
                }
            }
        })
        .build()
}

/// The window label for a Holder ID (letters, digits, `_` and `-`, as ccrpc takes them).
fn label(holder_id: &str) -> Result<String, String> {
    if holder_id.is_empty()
        || holder_id.len() > 64
        || !holder_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(format!("不是 Holder ID：{holder_id}"));
    }
    Ok(format!("{PREFIX}{holder_id}"))
}

/// Opens the terminal window for a Pane, or brings the one already open to the front.
#[tauri::command]
async fn open_window<R: Runtime>(
    app: AppHandle<R>,
    holder_id: String,
    title: String,
) -> Result<(), String> {
    let label = label(&holder_id)?;
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.unminimize();
        let _ = w.show();
        return w.set_focus().map_err(|e| e.to_string());
    }
    let url = WebviewUrl::App(format!("terminal.html?holder={holder_id}").into());
    WebviewWindowBuilder::new(&app, &label, url)
        .title(format!("终端 · {title}"))
        .inner_size(960.0, 560.0)
        .min_inner_size(360.0, 200.0)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn close_window<R: Runtime>(app: AppHandle<R>, holder_id: String) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(&label(&holder_id)?) {
        w.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn set_status<R: Runtime>(
    app: AppHandle<R>,
    links: State<'_, Links>,
    holder_id: String,
    status: String,
) -> Result<(), String> {
    let label = label(&holder_id)?;
    // Kept only for a window that is open (one closed by hand is still tracked by the main window).
    if app.get_webview_window(&label).is_none() {
        return Ok(());
    }
    links.statuses.set(&label, &status);
    app.emit_to(label, STATUS_EVENT, status)
        .map_err(|e| e.to_string())
}

/// The Holder IDs of the terminal windows open now.
#[tauri::command]
fn windows<R: Runtime>(app: AppHandle<R>) -> Vec<String> {
    app.webview_windows()
        .keys()
        .filter_map(|l| l.strip_prefix(PREFIX).map(String::from))
        .collect()
}

/// Attaches the calling terminal window to its own Pane; `on_data` gets the messages of [`holder`].
/// Attaching again (the page reloaded) replaces the earlier connection. Returns the status last set
/// for the window, if any.
#[tauri::command]
async fn attach<R: Runtime>(
    window: WebviewWindow<R>,
    links: State<'_, Links>,
    holder_id: String,
    on_data: Channel<InvokeResponseBody>,
) -> Result<Option<String>, String> {
    let label = label(&holder_id)?;
    if window.label() != label {
        return Err("只能连接本窗口的 Pane".into());
    }
    let env = match links.env.get() {
        Some(env) => env.clone(),
        None => {
            // Reading the login shell's environment takes a moment: off the async runtime.
            let env = tauri::async_runtime::spawn_blocking(codeck_host::ccrpc::env::login_env)
                .await
                .map_err(|e| e.to_string())?;
            links.env.get_or_init(|| env).clone()
        }
    };
    let link = tauri::async_runtime::spawn_blocking(move || {
        let socket = holder::locate(&env, &holder_id)?;
        Link::attach(&socket, move |m| {
            on_data.send(InvokeResponseBody::Raw(m)).is_ok()
        })
    })
    .await
    .map_err(|e| e.to_string())??;
    let status = links.statuses.get(&label);
    let mut by_window = links.by_window.lock().unwrap_or_else(|e| e.into_inner());
    // Closed while attaching: its Destroyed came first, so nothing would detach this link later.
    if window.app_handle().get_webview_window(&label).is_none() {
        return Ok(None);
    }
    let old = by_window.insert(label, link);
    drop(by_window);
    drop(old);
    Ok(status)
}

/// Queues bytes for the Pane (sync and non-blocking: typed keys keep their order).
#[tauri::command]
fn input<R: Runtime>(
    window: WebviewWindow<R>,
    links: State<'_, Links>,
    data: Vec<u8>,
) -> Result<(), String> {
    let by_window = links.by_window.lock().unwrap_or_else(|e| e.into_inner());
    let link = by_window.get(window.label()).ok_or("还没有连上 Pane")?;
    link.input(data)
}

#[cfg(test)]
mod tests {
    use super::{Statuses, label};

    #[test]
    fn keeps_the_last_status_until_the_window_goes() {
        let s = Statuses::default();
        assert_eq!(s.get("term-a"), None);
        s.set("term-a", "screen");
        s.set("term-a", "input");
        assert_eq!(s.get("term-a").as_deref(), Some("input"));
        assert_eq!(s.get("term-b"), None);
        s.forget("term-a");
        assert_eq!(s.get("term-a"), None);
    }

    #[test]
    fn labels_only_plain_holder_ids() {
        assert_eq!(label("m6697efb865bb").unwrap(), "term-m6697efb865bb");
        assert_eq!(label("kitty-3_a").unwrap(), "term-kitty-3_a");
        for bad in ["", "a/b", "../x", "m 1", "a?b"] {
            assert!(label(bad).is_err(), "{bad}");
        }
    }
}
