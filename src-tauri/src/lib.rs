//! CoDeck desktop app: tauri shell around `codeck_host::Host`.

pub mod commands;
pub mod events;

use std::sync::Arc;

use codeck_host::lan::{DEFAULT_PORT, LanConfig, server};
use codeck_host::{EventSink, Host, HostConfig, HostEvent};
use tauri::Manager;
use tauri_specta::{Builder, ErrorHandlingMode, Event, collect_events};

use crate::events::{
    CcrpcClosingEvent, CcrpcExitEvent, CcrpcLineEvent, CcrpcLsEndedEvent, CcrpcLsEvent,
    CcrpcRemoteConnEvent, IndexStatusEvent, LiveChangedEvent, PricesChangedEvent,
    SessionsChangedEvent,
};

/// The single source of commands and events, shared by the app and the bindings export.
pub fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(commands::collect())
        .events(collect_events![
            IndexStatusEvent,
            SessionsChangedEvent,
            LiveChangedEvent,
            PricesChangedEvent,
            CcrpcLineEvent,
            CcrpcExitEvent,
            CcrpcLsEvent,
            CcrpcLsEndedEvent,
            CcrpcRemoteConnEvent,
            CcrpcClosingEvent
        ])
        .error_handling(ErrorHandlingMode::Throw)
}

pub fn run() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();
    let builder = specta_builder();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            let (lan_events, lan_sink) = server::event_channel();
            let window_sink = sink(app.handle().clone());
            let events: EventSink = Arc::new(move |e: HostEvent| {
                lan_sink(e.clone());
                window_sink(e);
            });
            // The port can be moved only for development, beside a daemon on the default one.
            let port = std::env::var("CODECK_LAN_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_PORT);
            let config = HostConfig {
                reveal_path: Some(Arc::new(|p| {
                    tauri_plugin_opener::reveal_item_in_dir(p).map_err(|e| e.to_string())
                })),
                open_path: Some(Arc::new(|p| {
                    tauri_plugin_opener::open_path(p, None::<&str>).map_err(|e| e.to_string())
                })),
                // The window's front end keeps its own state, but LAN access needs the connections it already has.
                track_connect: true,
                lan: Some(LanConfig::new(
                    lan_events,
                    app.path().resource_dir()?.join("web"),
                    port,
                )),
                ..HostConfig::desktop()
            };
            let host = Host::start(config, events)?;
            app.manage(host.clone());
            tauri::async_runtime::spawn(async move { host.lan_restore().await });
            Ok(())
        })
        .on_window_event(|window, event| {
            let tauri::WindowEvent::CloseRequested { api, .. } = event else {
                return;
            };
            let app = window.app_handle();
            if !app
                .try_state::<Arc<Host>>()
                .is_some_and(|host| host.lan_enabled())
            {
                return;
            }
            // Keeps serving the LAN; only ⌘Q quits.
            api.prevent_close();
            hide(window);
        })
        .build(tauri::generate_context!())
        .expect("error while running CoDeck")
        .run(|handle, ev| match ev {
            tauri::RunEvent::Exit => {
                // Closes every ccrpc; the Sessions themselves keep running.
                if let Some(host) = handle.try_state::<Arc<Host>>() {
                    host.shutdown();
                }
            }
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => {
                // A click on the Dock icon brings a hidden window back.
                let _ = handle.show();
                if let Some(window) = handle.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            _ => {}
        });
}

/// A window in full screen is on a Space of its own: hidden alone it would leave that Space empty, so the whole app
/// hides instead.
fn hide(window: &tauri::Window) {
    #[cfg(target_os = "macos")]
    if window.is_fullscreen().unwrap_or(false) {
        let _ = window.app_handle().hide();
        return;
    }
    let _ = window.hide();
}

/// The Host's events, as the typed events.
fn sink(handle: tauri::AppHandle) -> codeck_host::EventSink {
    Arc::new(move |e| {
        let _ = match e {
            HostEvent::IndexStatus(s) => IndexStatusEvent(s).emit(&handle),
            HostEvent::SessionsChanged(s) => SessionsChangedEvent(s).emit(&handle),
            HostEvent::LiveChanged(l) => LiveChangedEvent(l).emit(&handle),
            HostEvent::PricesChanged => PricesChangedEvent.emit(&handle),
            HostEvent::CcrpcLine { conn, line } => CcrpcLineEvent { conn, line }.emit(&handle),
            HostEvent::CcrpcExit { conn, code, stderr } => {
                CcrpcExitEvent { conn, code, stderr }.emit(&handle)
            }
            HostEvent::CcrpcLs { ls } => CcrpcLsEvent { ls }.emit(&handle),
            HostEvent::CcrpcLsEnded { message } => CcrpcLsEndedEvent { message }.emit(&handle),
            HostEvent::CcrpcLsKeptEnded { .. } => Ok(()),
            HostEvent::CcrpcRemoteConn { conn, holder_id } => {
                CcrpcRemoteConnEvent { conn, holder_id }.emit(&handle)
            }
            HostEvent::CcrpcClosing { conn } => CcrpcClosingEvent { conn }.emit(&handle),
            // For the LAN's clients: the window folds ccrpc's lines itself.
            HostEvent::ConnectState(_) | HostEvent::ConnectRemoved { .. } => Ok(()),
        };
    })
}

/// Writes `src/ipc/bindings.ts`. Run with `pnpm bindings`.
#[cfg(test)]
#[test]
fn export_bindings() {
    specta_builder()
        .export(
            specta_typescript::Typescript::default(),
            concat!(env!("CARGO_MANIFEST_DIR"), "/../src/ipc/bindings.ts"),
        )
        .expect("failed to export typescript bindings");
}

/// LAN clients get `HostEvent::name()` / `payload()`: they must be what the typed events emit.
#[cfg(test)]
#[test]
fn host_events_match_the_typed_events() {
    use serde::Serialize;
    fn same<E: Event + Serialize>(host: HostEvent, typed: E) {
        assert_eq!(host.name(), E::NAME);
        assert_eq!(host.payload(), serde_json::to_value(typed).unwrap());
    }
    let s = codeck_core::model::SessionsChanged {
        changed: vec!["a".into()],
        removed: vec!["b".into()],
        projects_changed: true,
    };
    same(
        HostEvent::IndexStatus(Default::default()),
        IndexStatusEvent(Default::default()),
    );
    same(
        HostEvent::SessionsChanged(s.clone()),
        SessionsChangedEvent(s),
    );
    same(
        HostEvent::LiveChanged(Default::default()),
        LiveChangedEvent(Default::default()),
    );
    same(HostEvent::PricesChanged, PricesChangedEvent);
    let (conn, line, stderr) = ("c1".to_string(), "{}".to_string(), "e".to_string());
    same(
        HostEvent::CcrpcLine {
            conn: conn.clone(),
            line: line.clone(),
        },
        CcrpcLineEvent {
            conn: conn.clone(),
            line,
        },
    );
    same(
        HostEvent::CcrpcExit {
            conn: conn.clone(),
            code: Some(1),
            stderr: stderr.clone(),
        },
        CcrpcExitEvent {
            conn,
            code: Some(1),
            stderr,
        },
    );
    same(
        HostEvent::CcrpcLs { ls: "[]".into() },
        CcrpcLsEvent { ls: "[]".into() },
    );
    same(
        HostEvent::CcrpcLsEnded {
            message: "m".into(),
        },
        CcrpcLsEndedEvent {
            message: "m".into(),
        },
    );
    same(
        HostEvent::CcrpcRemoteConn {
            conn: "c1".into(),
            holder_id: Some("H1".into()),
        },
        CcrpcRemoteConnEvent {
            conn: "c1".into(),
            holder_id: Some("H1".into()),
        },
    );
    same(
        HostEvent::CcrpcClosing { conn: "c1".into() },
        CcrpcClosingEvent { conn: "c1".into() },
    );
}
