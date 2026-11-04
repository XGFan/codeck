use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

#[derive(Serialize, Deserialize, Type, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DataRootSource {
    /// Override saved in the app's settings.json.
    Settings,
    /// `$CLAUDE_CONFIG_DIR`.
    Env,
    /// `~/.claude`.
    Default,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub data_root: String,
    pub data_root_source: DataRootSource,
    pub data_root_exists: bool,
    pub cache_dir: String,
    pub schema_version: u32,
}

#[derive(Serialize, Deserialize, Type, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum IndexPhase {
    #[default]
    Idle,
    Scanning,
    IndexingText,
    Error,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatus {
    pub phase: IndexPhase,
    pub files_total: u32,
    pub files_done: u32,
    #[specta(type = Number)]
    pub bytes_total: f64,
    #[specta(type = Number)]
    pub bytes_done: f64,
    pub sessions_total: u32,
    /// Full-text backlog is empty; search results are complete.
    pub text_ready: bool,
    pub error: Option<String>,
}

/// Where a slash command comes from.
#[derive(Serialize, Deserialize, Type, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SlashSource {
    /// Claude Code's own command (a static list, see `codeck_core::slash`).
    Builtin,
    /// `<data root>/commands/*.md` or `<data root>/skills/*/SKILL.md`.
    User,
    /// `<cwd>/.claude/commands` or `<cwd>/.claude/skills`.
    Project,
    /// A command or skill of an enabled plugin.
    Plugin,
}

/// A slash command offered for completion while driving.
#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SlashCommand {
    /// As typed, without the slash: `compact`, `review`, `oh-my-claudecode:ask`.
    pub name: String,
    pub description: Option<String>,
    pub source: SlashSource,
    /// Opens a panel or picker in the terminal rather than running (built-in commands only).
    pub interactive: bool,
}
