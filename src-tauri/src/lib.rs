//! CoDeck desktop app: tauri shell around `codeck_core::Engine`.

pub mod ccrpc;
pub mod commands;
pub mod events;
pub mod prices;
pub mod settings;
pub mod state;
pub mod watcher;
pub mod worker;

use tauri::Manager;
use tauri_specta::{Builder, ErrorHandlingMode, collect_commands, collect_events};

use crate::events::{
    CcrpcExitEvent, CcrpcLineEvent, IndexStatusEvent, LiveChangedEvent, PricesChangedEvent,
    SessionsChangedEvent,
};
use crate::state::{AppState, open_engine};

/// The single source of commands and events, shared by the app and the bindings export.
pub fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::get_app_info,
            commands::set_data_root,
            commands::get_index_status,
            commands::rebuild_index,
            commands::list_projects,
            commands::list_sessions,
            commands::get_session,
            commands::get_transcript,
            commands::get_tool_detail,
            commands::get_image,
            commands::search,
            commands::resolve_jump,
            commands::find_in_session,
            commands::reveal_session_file,
            commands::get_stats,
            commands::get_diagnostics,
            commands::list_slash_commands,
            ccrpc::ccrpc_info,
            ccrpc::ccrpc_set_analyzer,
            ccrpc::ccrpc_set_launch_rules,
            ccrpc::ccrpc_set_hide_new_panes,
            ccrpc::ccrpc_set_pane_hidden,
            ccrpc::ccrpc_list,
            ccrpc::ccrpc_connect,
            ccrpc::ccrpc_new,
            ccrpc::ccrpc_resume,
            ccrpc::ccrpc_send,
            ccrpc::ccrpc_disconnect,
            ccrpc::ccrpc_gui_log,
            ccrpc::ccrpc_log_dir,
            ccrpc::ccrpc_save_image,
        ])
        .events(collect_events![
            IndexStatusEvent,
            SessionsChangedEvent,
            LiveChangedEvent,
            PricesChangedEvent,
            CcrpcLineEvent,
            CcrpcExitEvent
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
        .manage(ccrpc::CcrpcState::default())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            let (jobs, rx, backlog_cancel) = worker::channel();
            app.manage(AppState::new(open_engine(&settings::load())?, jobs.clone()));
            worker::spawn(app.handle().clone(), rx, jobs.clone(), backlog_cancel);
            // Starts the watcher, then FullScan -> TextBacklog.
            jobs.send(worker::Job::SetRoot);
            jobs.send(worker::Job::RefreshPrices);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while running CoDeck")
        .run(|handle, ev| {
            if let tauri::RunEvent::Exit = ev {
                // Closes every ccrpc; the Sessions themselves keep running.
                handle.state::<ccrpc::CcrpcState>().disconnect_all();
            }
        });
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
