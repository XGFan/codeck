//! The IPC contract. Every type here is exported to `src/ipc/bindings.ts` via specta.
//!
//! Rules: camelCase fields; data-carrying enums are tagged with `kind`; counts are `u32`,
//! timestamps (unix ms), tokens and byte sizes are `f64` (no bigint in TS) and always finite, so
//! each carries `#[specta(type = Number)]` to export as `number` instead of `number | null`;
//! JSON blobs are `String` fields named `*_json`.

pub mod app;
pub mod common;
pub mod diag;
pub mod events;
pub mod search;
pub mod session;
pub mod stats;
pub mod transcript;

pub use app::*;
pub use common::*;
pub use diag::*;
pub use events::*;
pub use search::*;
pub use session::*;
pub use stats::*;
pub use transcript::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The serde wire shape must match the exported TS types (`kind` tag, camelCase variant fields).
    #[test]
    fn tagged_enums_serialize_as_exported() {
        let scope = TranscriptScope::Subagent {
            agent_id: "a1".into(),
        };
        assert_eq!(
            serde_json::to_value(&scope).unwrap(),
            json!({"kind": "subagent", "agentId": "a1"})
        );

        let block = AssistantBlock::ToolCall(ToolCall {
            tool_use_id: "t1".into(),
            name: "Bash".into(),
            input_json: "{}".into(),
            input_truncated: false,
            result: None,
            subagent_id: None,
            workflow_run_id: None,
            notification_node_id: None,
        });
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(v["kind"], "toolCall");
        assert_eq!(v["toolUseId"], "t1");

        let origin = PromptOrigin::TaskNotification {
            task_id: Some("x".into()),
            tool_use_id: None,
            status: None,
            summary: None,
        };
        let v = serde_json::to_value(&origin).unwrap();
        assert_eq!(v["kind"], "taskNotification");
        assert_eq!(v["taskId"], "x");

        assert_eq!(
            serde_json::to_value(ErrorCode::InvalidQuery).unwrap(),
            json!("invalidQuery")
        );
        let back: TranscriptScope = serde_json::from_value(json!({"kind": "main"})).unwrap();
        assert_eq!(back, TranscriptScope::Main);
    }
}
