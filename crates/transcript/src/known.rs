//! Known entry types, content block types, system subtypes and tool names.
//!
//! Anything not listed here is still shown (generic view) and counted in Diagnostics with its
//! Claude Code version. Lists reflect Claude Code 2.1.x data.

/// Top-level `type` values of JSONL entries.
pub const ENTRY_TYPES: &[&str] = &[
    "user",
    "assistant",
    "system",
    "attachment",
    "summary",
    "last-prompt",
    "permission-mode",
    "mode",
    "ai-title",
    "custom-title",
    "agent-name",
    "queue-operation",
    "atis-latch",
    "file-history-snapshot",
    "file-history-delta",
    "bridge-session",
    "pr-link",
    "frame-link",
    "cost-state",
    "relocated",
    "worktree-state",
    "fork-context-ref",
    "continued-in",
    "tools-used",
    "api-request",
    "api-request-blob",
    "api-request-shape",
];

/// Entry types matched by prefix (e.g. `artifact-autoreact-ledger`).
pub const ENTRY_TYPE_PREFIXES: &[&str] = &["artifact-"];

/// `message.content[].type` values.
pub const BLOCK_TYPES: &[&str] = &[
    "text",
    "thinking",
    "redacted_thinking",
    "tool_use",
    "tool_result",
    "image",
    "document",
    "fallback",
    "tool_addition",
];

/// `subtype` values of `system` entries.
pub const SYSTEM_SUBTYPES: &[&str] = &[
    "stop_hook_summary",
    "turn_duration",
    "away_summary",
    "compact_boundary",
    "microcompact_boundary",
    "local_command",
    "scheduled_task_fire",
    "agents_killed",
    "api_error",
    "informational",
    "model_refusal_fallback",
    "model_consent_fallback",
    "bridge_status",
];

/// System subtypes hidden unless `include_hidden` (§3.4 step 6).
pub const HIDDEN_SYSTEM_SUBTYPES: &[&str] = &["stop_hook_summary", "turn_duration", "away_summary"];

/// Built-in Claude Code tool names. `mcp__*` tools are a known class (see [`is_known_tool`]).
pub const TOOLS: &[&str] = &[
    "Bash",
    "BashOutput",
    "KillShell",
    "KillBash",
    "Read",
    "Write",
    "Edit",
    "MultiEdit",
    "NotebookEdit",
    "Glob",
    "Grep",
    "LS",
    "WebFetch",
    "WebSearch",
    "Task",
    "Agent",
    "TodoWrite",
    "TaskCreate",
    "TaskUpdate",
    "TaskList",
    "TaskGet",
    "TaskOutput",
    "TaskStop",
    "AskUserQuestion",
    "ToolSearch",
    "Skill",
    "SendMessage",
    "ListAgents",
    "StructuredOutput",
    "Monitor",
    "Workflow",
    "ScheduleWakeup",
    "CronCreate",
    "CronDelete",
    "TeamCreate",
    "TeamDelete",
    "EnterPlanMode",
    "ExitPlanMode",
    "EnterWorktree",
    "ExitWorktree",
    "SlashCommand",
    "Artifact",
    "SendUserFile",
    "DesignSync",
    "ReportFindings",
    "SendFeedback",
];

/// Tools that spawn a Subagent Run.
pub const AGENT_TOOLS: &[&str] = &["Agent", "Task"];

pub fn is_known_entry_type(t: &str) -> bool {
    ENTRY_TYPES.contains(&t) || ENTRY_TYPE_PREFIXES.iter().any(|p| t.starts_with(p))
}

pub fn is_known_block_type(t: &str) -> bool {
    BLOCK_TYPES.contains(&t)
}

pub fn is_known_system_subtype(t: &str) -> bool {
    SYSTEM_SUBTYPES.contains(&t)
}

pub fn is_hidden_system_subtype(t: &str) -> bool {
    HIDDEN_SYSTEM_SUBTYPES.contains(&t)
}

pub fn is_known_tool(name: &str) -> bool {
    name.starts_with("mcp__") || TOOLS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_known_names() {
        assert!(is_known_entry_type("artifact-comment-monitor"));
        assert!(!is_known_entry_type("brand-new-type"));
        assert!(is_known_tool("mcp__github__create_issue"));
        assert!(!is_known_tool("SomeFutureTool"));
        assert!(is_hidden_system_subtype("turn_duration"));
        assert!(!is_hidden_system_subtype("api_error"));
    }
}
