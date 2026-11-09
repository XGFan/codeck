//! The Host's commands on the Engine and the settings. All are async and run the blocking Engine call
//! on the blocking pool.

use codeck_core::model::{
    AppError, AppInfo, Diagnostics, ErrorCode, FindRequest, FindResult, ImageData, ImageRequest,
    IndexStatus, JumpRequest, JumpTarget, ProjectSummary, SearchRequest, SearchResponse,
    SessionDetail, SessionQuery, SessionSummary, SlashCommand, Stats, StatsRequest, ToolDetail,
    ToolDetailRequest, Transcript, TranscriptRequest,
};
use codeck_core::{CoreError, CoreResult, Engine};

use crate::Host;
use crate::settings::SidebarProject;
use crate::worker::Job;

pub type CmdResult<T> = Result<T, AppError>;

/// The blocking task itself failed.
pub(crate) fn internal(e: impl std::fmt::Display) -> AppError {
    AppError {
        code: ErrorCode::Internal,
        message: format!("后台任务失败：{e}"),
    }
}

impl Host {
    /// Runs `f` with the current Engine on the blocking pool.
    async fn with_engine<T, F>(&self, f: F) -> CmdResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&Engine) -> CoreResult<T> + Send + 'static,
    {
        let engine = self.state.engine();
        tokio::task::spawn_blocking(move || f(&engine))
            .await
            .map_err(internal)?
            .map_err(AppError::from)
    }

    pub async fn get_app_info(&self) -> CmdResult<AppInfo> {
        self.with_engine(|e| Ok(e.app_info())).await
    }

    /// Persists the data root override (`None` resets to `$CLAUDE_CONFIG_DIR` / `~/.claude`), swaps
    /// the Engine and triggers a rescan.
    pub async fn set_data_root(&self, path: Option<String>) -> CmdResult<AppInfo> {
        let path = path.map(|p| p.trim().to_owned()).filter(|p| !p.is_empty());
        if let Some(p) = &path
            && !std::path::Path::new(p).is_dir()
        {
            return Err(CoreError::InvalidQuery(format!("目录不存在：{p}")).into());
        }
        // The worker swaps the Engine between jobs, so nothing is writing to the index meanwhile.
        let (reply, rx) = crossbeam_channel::bounded(1);
        if !self.state.jobs.send(Job::SwapRoot {
            data_root: path,
            reply,
        }) {
            return Err(internal("worker stopped"));
        }
        tokio::task::spawn_blocking(move || rx.recv())
            .await
            .map_err(internal)?
            .map_err(internal)?
            .map_err(AppError::from)
    }

    pub async fn get_index_status(&self) -> CmdResult<IndexStatus> {
        self.with_engine(|e| Ok(e.index_status())).await
    }

    pub async fn rebuild_index(&self) -> CmdResult<()> {
        self.state.jobs.send(Job::Rebuild);
        Ok(())
    }

    pub async fn list_projects(&self) -> CmdResult<Vec<ProjectSummary>> {
        self.with_engine(|e| e.list_projects()).await
    }

    pub async fn list_sessions(&self, query: SessionQuery) -> CmdResult<Vec<SessionSummary>> {
        self.with_engine(move |e| e.list_sessions(&query)).await
    }

    pub async fn get_session(&self, session_id: String) -> CmdResult<SessionDetail> {
        self.with_engine(move |e| e.get_session(&session_id)).await
    }

    pub async fn get_transcript(&self, req: TranscriptRequest) -> CmdResult<Transcript> {
        self.with_engine(move |e| e.get_transcript(&req)).await
    }

    pub async fn get_tool_detail(&self, req: ToolDetailRequest) -> CmdResult<ToolDetail> {
        self.with_engine(move |e| e.get_tool_detail(&req)).await
    }

    pub async fn get_image(&self, req: ImageRequest) -> CmdResult<ImageData> {
        self.with_engine(move |e| e.get_image(&req)).await
    }

    pub async fn search(&self, req: SearchRequest) -> CmdResult<SearchResponse> {
        self.with_engine(move |e| e.search(&req)).await
    }

    pub async fn resolve_jump(&self, req: JumpRequest) -> CmdResult<JumpTarget> {
        self.with_engine(move |e| e.resolve_jump(&req)).await
    }

    pub async fn find_in_session(&self, req: FindRequest) -> CmdResult<FindResult> {
        self.with_engine(move |e| e.find_in_session(&req)).await
    }

    /// Syncs the Session's main file now (a prompt was just sent to it), without waiting for the watcher's debounce: a
    /// `Job::Fs` like the watcher's, so it merges with any batch queued.
    pub async fn sync_session(&self, session_id: String) -> CmdResult<()> {
        let path = self
            .with_engine(move |e| e.session_file_path(&session_id, None))
            .await?;
        self.state.jobs.send(Job::Fs(vec![path]));
        Ok(())
    }

    /// Reveals the Session's main file (or the Subagent Run's file) in Finder.
    pub async fn reveal_session_file(
        &self,
        session_id: String,
        agent_id: Option<String>,
    ) -> CmdResult<()> {
        let reveal = self.config.reveal_path.clone();
        self.with_engine(move |e| {
            let path = e.session_file_path(&session_id, agent_id.as_deref())?;
            let reveal =
                reveal.ok_or_else(|| CoreError::Internal("这个 Host 不能显示文件".into()))?;
            reveal(&path)
                .map_err(|err| CoreError::Internal(format!("无法在 Finder 中显示文件：{err}")))
        })
        .await
    }

    pub async fn get_stats(&self, req: StatsRequest) -> CmdResult<Stats> {
        self.with_engine(move |e| e.stats(&req)).await
    }

    pub async fn get_diagnostics(&self) -> CmdResult<Diagnostics> {
        self.with_engine(|e| e.diagnostics()).await
    }

    /// Slash commands for completion while connected to a Session started in `cwd`.
    pub async fn list_slash_commands(&self, cwd: Option<String>) -> CmdResult<Vec<SlashCommand>> {
        self.with_engine(move |e| Ok(e.slash_commands(cwd.as_deref())))
            .await
    }

    /// Paths to complete `partial` with in a Session started in `cwd`: directories (`/add-dir`), or files too (`@`).
    pub async fn complete_paths(
        &self,
        cwd: Option<String>,
        partial: String,
        files: bool,
    ) -> CmdResult<Vec<String>> {
        self.with_engine(move |e| Ok(e.complete_paths(cwd.as_deref(), &partial, files)))
            .await
    }

    /// The Projects in the sidebar; `None` before any were added.
    pub async fn get_sidebar_projects(&self) -> CmdResult<Option<Vec<SidebarProject>>> {
        tokio::task::spawn_blocking(|| crate::settings::load().sidebar_projects)
            .await
            .map_err(internal)
    }

    /// Saves the Projects in the sidebar (paths trimmed, empty and repeated ones dropped); returns what was saved.
    pub async fn set_sidebar_projects(
        &self,
        projects: Vec<SidebarProject>,
    ) -> CmdResult<Vec<SidebarProject>> {
        let projects = crate::settings::normalize_sidebar(projects);
        tokio::task::spawn_blocking(move || {
            crate::settings::update(|s| s.sidebar_projects = Some(projects.clone()))
                .map(|_| projects)
                .map_err(|e| AppError {
                    code: ErrorCode::Internal,
                    message: format!("保存设置失败：{e}"),
                })
        })
        .await
        .map_err(internal)?
    }
}
