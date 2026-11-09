//! The method table (ADR-0011): every command the Host serves, defined once. `for_each_method!(cb)`
//! hands the whole table to the macro `cb`; this module builds `dispatch` from it, the desktop app its
//! tauri commands (same names, parameters and docs, in this order).
//!
//! Each entry is `<scope> fn <name>(<param>: <type>, …) -> <type>;`, the `Host` method of that name
//! returning `Result<type, AppError>`. `remote` methods are served to the daemon's clients too; `local`
//! ones only to the desktop app (settings, data root, index, Finder, ccrpc setup and logs).

use codeck_core::model::{AppError, ErrorCode};
use serde_json::Value;

use crate::Host;
use crate::commands::internal;

#[macro_export]
macro_rules! for_each_method {
    ($cb:ident) => {
        $cb! {
            remote fn get_app_info() -> $crate::types::AppInfo;
            /// Persists the data root override (`None` resets to `$CLAUDE_CONFIG_DIR` / `~/.claude`), swaps
            /// the Engine and triggers a rescan.
            local fn set_data_root(path: Option<String>) -> $crate::types::AppInfo;
            remote fn get_index_status() -> $crate::types::IndexStatus;
            local fn rebuild_index() -> ();
            remote fn list_projects() -> Vec<$crate::types::ProjectSummary>;
            remote fn list_sessions(query: $crate::types::SessionQuery) -> Vec<$crate::types::SessionSummary>;
            remote fn get_session(session_id: String) -> $crate::types::SessionDetail;
            remote fn get_transcript(req: $crate::types::TranscriptRequest) -> $crate::types::Transcript;
            remote fn get_tool_detail(req: $crate::types::ToolDetailRequest) -> $crate::types::ToolDetail;
            remote fn get_image(req: $crate::types::ImageRequest) -> $crate::types::ImageData;
            remote fn search(req: $crate::types::SearchRequest) -> $crate::types::SearchResponse;
            remote fn resolve_jump(req: $crate::types::JumpRequest) -> $crate::types::JumpTarget;
            remote fn find_in_session(req: $crate::types::FindRequest) -> $crate::types::FindResult;
            /// Reveals the Session's main file (or the Subagent Run's file) in Finder.
            local fn reveal_session_file(session_id: String, agent_id: Option<String>) -> ();
            /// Syncs the Session's main file now (a prompt was just sent to it), without waiting for the watcher's debounce: a
            /// `Job::Fs` like the watcher's, so it merges with any batch queued.
            remote fn sync_session(session_id: String) -> ();
            local fn get_stats(req: $crate::types::StatsRequest) -> $crate::types::Stats;
            local fn get_diagnostics() -> $crate::types::Diagnostics;
            /// Slash commands for completion while connected to a Session started in `cwd`.
            remote fn list_slash_commands(cwd: Option<String>) -> Vec<$crate::types::SlashCommand>;
            /// Paths to complete `partial` with in a Session started in `cwd`: directories (`/add-dir`), or files too (`@`).
            remote fn complete_paths(cwd: Option<String>, partial: String, files: bool) -> Vec<String>;
            /// The Projects in the sidebar; `None` before any were added.
            remote fn get_sidebar_projects() -> Option<Vec<$crate::types::SidebarProject>>;
            /// Saves the Projects in the sidebar (paths trimmed, empty and repeated ones dropped); returns what was saved.
            local fn set_sidebar_projects(projects: Vec<$crate::types::SidebarProject>) -> Vec<$crate::types::SidebarProject>;
            local fn ccrpc_info() -> $crate::types::CcrpcInfo;
            /// Saves the analyzer's settings (blank = not set here). `api_key: None` keeps the saved key; `Some("")`
            /// removes it. Applies to the connections made from now on.
            local fn ccrpc_set_analyzer(api_key: Option<String>, base_url: Option<String>, model: Option<String>) -> $crate::types::AnalyzerInfo;
            /// Tests the analyzer with these values (as `ccrpc_set_analyzer` takes them, `api_key: None` = the saved
            /// key) without saving them: `ccrpc analyzer test`, one analysis of a built-in dialog.
            local fn ccrpc_test_analyzer(api_key: Option<String>, base_url: Option<String>, model: Option<String>) -> $crate::types::AnalyzerTest;
            /// ccrpc's analysis records (`ccrpc analyzer records`).
            local fn ccrpc_analyzer_records() -> $crate::types::AnalyzerRecords;
            /// Opens the analysis records' directory in Finder (ccrpc says where it is and creates it).
            local fn ccrpc_open_analyzer_records() -> ();
            /// Saves the launch rules (trimmed; rules without arguments dropped) and returns what was saved.
            local fn ccrpc_set_launch_rules(rules: Vec<$crate::types::LaunchRule>) -> Vec<$crate::types::LaunchRule>;
            /// Saves 「新开的 Pane」: whether the Sessions CoDeck starts from now on open hidden on the ToH Mac Client.
            local fn ccrpc_set_hide_new_panes(hide: bool) -> bool;
            /// Hides a clay Session's Pane on the ToH Mac Client, or shows it again (`ccrpc hide|show`).
            local fn ccrpc_set_pane_hidden(holder_id: String, hidden: bool) -> ();
            /// Starts (`on`) or stops `ccrpc ls --json --watch`; its lists come as `CcrpcLsEvent`, its end by
            /// itself as `CcrpcLsEndedEvent`. Starting again restarts it (the current list comes at once).
            remote fn ccrpc_watch_sessions(on: bool) -> ();
            remote fn ccrpc_connect(holder_id: String) -> String;
            remote fn ccrpc_new(cwd: String) -> String;
            /// Resume: starts the ended Session `session_id` again in a new Pane (`claude --resume`) and connects to it.
            remote fn ccrpc_resume(session_id: String, cwd: String) -> String;
            /// Fork: starts a Fork of the Session `session_id` in a new Pane (before the Main Line prompt `at`, or
            /// all of it), named `title`, and connects to it; the result comes as ccrpc's `fork` event.
            remote fn ccrpc_fork(session_id: String, cwd: String, at: Option<String>, restore: Option<String>, title: String) -> String;
            remote fn ccrpc_send(conn: String, line: String) -> ();
            remote fn ccrpc_disconnect(conn: String) -> ();
            local fn ccrpc_gui_log(conn: Option<String>, text: String) -> ();
            /// The connection's log directory, `None` once its ccrpc has exited.
            local fn ccrpc_log_dir(conn: String) -> Option<String>;
            remote fn ccrpc_save_image(base64: String, ext: String) -> String;
        }
    };
}

/// `true` for `remote`; anything but `remote` / `local` does not compile.
macro_rules! is_remote {
    (remote) => {
        true
    };
    (local) => {
        false
    };
}

macro_rules! gen_dispatch {
    ($( $(#[$meta:meta])* $scope:ident fn $name:ident ( $($p:ident : $t:ty),* ) -> $ret:ty; )*) => {
        /// Calls the method `method` (its snake_case name) with `args`, an object keyed by the parameters'
        /// camelCase names as tauri's `invoke` passes them; the result serialized as the desktop app gets it.
        /// With `remote_only`, `local` methods are refused.
        pub async fn dispatch(
            host: &Host,
            method: &str,
            args: Value,
            remote_only: bool,
        ) -> Result<Value, AppError> {
            let args = if args.is_null() { Value::Object(Default::default()) } else { args };
            match method {
                $(
                    stringify!($name) => {
                        if remote_only && !is_remote!($scope) {
                            return Err(local_only(method));
                        }
                        #[derive(serde::Deserialize)]
                        #[serde(rename_all = "camelCase")]
                        struct Args {
                            $($p: $t,)*
                        }
                        let Args { $($p),* } = serde_json::from_value(args).map_err(bad_args)?;
                        let out: $ret = host.$name($($p),*).await?;
                        serde_json::to_value(out).map_err(internal)
                    }
                )*
                _ => Err(AppError {
                    code: ErrorCode::NotFound,
                    message: format!("没有方法 {method}"),
                }),
            }
        }
    };
}

for_each_method!(gen_dispatch);

fn local_only(method: &str) -> AppError {
    AppError {
        code: ErrorCode::InvalidQuery,
        message: format!("方法 {method} 只供桌面应用使用"),
    }
}

fn bad_args(e: serde_json::Error) -> AppError {
    AppError {
        code: ErrorCode::InvalidQuery,
        message: format!("参数不对：{e}"),
    }
}
