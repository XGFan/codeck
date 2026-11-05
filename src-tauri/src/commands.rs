//! IPC commands. All are async and run the blocking Engine call on the blocking pool
//! (a sync command would run on the main thread).

use codeck_core::model::{
    AppError, AppInfo, Diagnostics, ErrorCode, FindRequest, FindResult, ImageData, ImageRequest,
    IndexStatus, JumpRequest, JumpTarget, ProjectSummary, SearchRequest, SearchResponse,
    SessionDetail, SessionQuery, SessionSummary, SlashCommand, Stats, StatsRequest, ToolDetail,
    ToolDetailRequest, Transcript, TranscriptRequest,
};
use codeck_core::{CoreError, CoreResult, Engine};
use tauri::State;

use crate::state::AppState;
use crate::worker::Job;

type CmdResult<T> = Result<T, AppError>;

/// The blocking task itself failed.
pub(crate) fn internal(e: impl std::fmt::Display) -> AppError {
    AppError {
        code: ErrorCode::Internal,
        message: format!("后台任务失败：{e}"),
    }
}

/// Runs `f` with the current Engine on the blocking pool.
async fn with_engine<T, F>(state: &State<'_, AppState>, f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce(&Engine) -> CoreResult<T> + Send + 'static,
{
    let engine = state.engine();
    tauri::async_runtime::spawn_blocking(move || f(&engine))
        .await
        .map_err(internal)?
        .map_err(AppError::from)
}

#[tauri::command]
#[specta::specta]
pub async fn get_app_info(state: State<'_, AppState>) -> CmdResult<AppInfo> {
    with_engine(&state, |e| Ok(e.app_info())).await
}

/// Persists the data root override (`None` resets to `$CLAUDE_CONFIG_DIR` / `~/.claude`), swaps
/// the Engine and triggers a rescan.
#[tauri::command]
#[specta::specta]
pub async fn set_data_root(state: State<'_, AppState>, path: Option<String>) -> CmdResult<AppInfo> {
    let path = path.map(|p| p.trim().to_owned()).filter(|p| !p.is_empty());
    if let Some(p) = &path
        && !std::path::Path::new(p).is_dir()
    {
        return Err(CoreError::InvalidQuery(format!("目录不存在：{p}")).into());
    }
    // The worker swaps the Engine between jobs, so nothing is writing to the index meanwhile.
    let (reply, rx) = crossbeam_channel::bounded(1);
    if !state.jobs.send(Job::SwapRoot {
        data_root: path,
        reply,
    }) {
        return Err(internal("worker stopped"));
    }
    tauri::async_runtime::spawn_blocking(move || rx.recv())
        .await
        .map_err(internal)?
        .map_err(internal)?
        .map_err(AppError::from)
}

#[tauri::command]
#[specta::specta]
pub async fn get_index_status(state: State<'_, AppState>) -> CmdResult<IndexStatus> {
    with_engine(&state, |e| Ok(e.index_status())).await
}

#[tauri::command]
#[specta::specta]
pub async fn rebuild_index(state: State<'_, AppState>) -> CmdResult<()> {
    state.jobs.send(Job::Rebuild);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn list_projects(state: State<'_, AppState>) -> CmdResult<Vec<ProjectSummary>> {
    with_engine(&state, |e| e.list_projects()).await
}

#[tauri::command]
#[specta::specta]
pub async fn list_sessions(
    state: State<'_, AppState>,
    query: SessionQuery,
) -> CmdResult<Vec<SessionSummary>> {
    with_engine(&state, move |e| e.list_sessions(&query)).await
}

#[tauri::command]
#[specta::specta]
pub async fn get_session(
    state: State<'_, AppState>,
    session_id: String,
) -> CmdResult<SessionDetail> {
    with_engine(&state, move |e| e.get_session(&session_id)).await
}

#[tauri::command]
#[specta::specta]
pub async fn get_transcript(
    state: State<'_, AppState>,
    req: TranscriptRequest,
) -> CmdResult<Transcript> {
    with_engine(&state, move |e| e.get_transcript(&req)).await
}

#[tauri::command]
#[specta::specta]
pub async fn get_tool_detail(
    state: State<'_, AppState>,
    req: ToolDetailRequest,
) -> CmdResult<ToolDetail> {
    with_engine(&state, move |e| e.get_tool_detail(&req)).await
}

#[tauri::command]
#[specta::specta]
pub async fn get_image(state: State<'_, AppState>, req: ImageRequest) -> CmdResult<ImageData> {
    with_engine(&state, move |e| e.get_image(&req)).await
}

#[tauri::command]
#[specta::specta]
pub async fn search(state: State<'_, AppState>, req: SearchRequest) -> CmdResult<SearchResponse> {
    with_engine(&state, move |e| e.search(&req)).await
}

#[tauri::command]
#[specta::specta]
pub async fn resolve_jump(state: State<'_, AppState>, req: JumpRequest) -> CmdResult<JumpTarget> {
    with_engine(&state, move |e| e.resolve_jump(&req)).await
}

#[tauri::command]
#[specta::specta]
pub async fn find_in_session(
    state: State<'_, AppState>,
    req: FindRequest,
) -> CmdResult<FindResult> {
    with_engine(&state, move |e| e.find_in_session(&req)).await
}

/// Reveals the Session's main file (or the Subagent Run's file) in Finder.
#[tauri::command]
#[specta::specta]
pub async fn reveal_session_file(
    state: State<'_, AppState>,
    session_id: String,
    agent_id: Option<String>,
) -> CmdResult<()> {
    with_engine(&state, move |e| {
        let path = e.session_file_path(&session_id, agent_id.as_deref())?;
        tauri_plugin_opener::reveal_item_in_dir(&path)
            .map_err(|err| CoreError::Internal(format!("无法在 Finder 中显示文件：{err}")))
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn get_stats(state: State<'_, AppState>, req: StatsRequest) -> CmdResult<Stats> {
    with_engine(&state, move |e| e.stats(&req)).await
}

#[tauri::command]
#[specta::specta]
pub async fn get_diagnostics(state: State<'_, AppState>) -> CmdResult<Diagnostics> {
    with_engine(&state, |e| e.diagnostics()).await
}

/// Slash commands for completion while connected to a Session started in `cwd`.
#[tauri::command]
#[specta::specta]
pub async fn list_slash_commands(
    state: State<'_, AppState>,
    cwd: Option<String>,
) -> CmdResult<Vec<SlashCommand>> {
    with_engine(&state, move |e| Ok(e.slash_commands(cwd.as_deref()))).await
}
