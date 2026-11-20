//! Code Mode (`CLAUDE_CODE_EXPERIMENTAL_CODE_MODE`): Claude's code (a `REPL` call) makes the tool
//! calls, and Claude Code writes them to the transcript only once the code's result is delivered. So
//! while the code runs no `block` says what it is doing. The plugin's hooks do: this follows the main
//! line's and yields the `code_call` / `code_call_end` events.

use std::collections::VecDeque;

use serde_json::{Value, json};

/// The tool Claude's code runs in.
const REPL: &str = "REPL";
/// Running code kept (a script can run on after its call returned; several can run at once).
const MAX_RUNNING: usize = 16;
/// Calls reported and not ended kept.
const MAX_OPEN: usize = 256;
/// A `tool_response` larger than this is left out of `code_call_end`.
const RESPONSE_BYTES: usize = 16 * 1024;

/// Whether `tool_use_id` is of a call made from code: Claude Code gives these ids of its own
/// (`repl_<uuid>`), the model's calls have the API's (`toolu_…`).
pub(super) fn from_code(tool_use_id: &str) -> bool {
    tool_use_id.starts_with("repl_")
}

#[derive(Default)]
pub(super) struct CodeCalls {
    /// `REPL` calls whose code is running (PreToolUse seen, PostToolUse not yet: it comes when the
    /// code ends, not when the call returns), oldest first.
    running: Vec<String>,
    /// Calls reported with `code_call` and not ended.
    open: VecDeque<String>,
}

impl CodeCalls {
    /// A main-line hook event; the event to emit for it, if any. `code` of a `code_call` is the
    /// latest code still running: with several running at once nothing says which one made the call.
    pub(super) fn on_hook(&mut self, event: &str, input: &Value) -> Option<Value> {
        let id = input["tool_use_id"].as_str()?;
        match event {
            "PreToolUse" if from_code(id) => {
                self.open.push_back(id.to_owned());
                if self.open.len() > MAX_OPEN {
                    self.open.pop_front();
                }
                Some(json!({"type": "code_call", "tool_use_id": id,
                    "name": input["tool_name"], "input": input["tool_input"],
                    "code": self.running.last()}))
            }
            "PreToolUse" if input["tool_name"] == REPL => {
                self.running.push(id.to_owned());
                if self.running.len() > MAX_RUNNING {
                    self.running.remove(0);
                }
                None
            }
            "PostToolUse" | "PostToolUseFailure" | "PermissionDenied" => {
                self.running.retain(|r| r != id);
                let at = self.open.iter().position(|o| o == id)?;
                self.open.remove(at);
                let response = Some(&input["tool_response"])
                    .filter(|r| r.to_string().len() <= RESPONSE_BYTES)
                    .cloned()
                    .unwrap_or(Value::Null);
                Some(json!({"type": "code_call_end", "tool_use_id": id,
                    "is_error": event != "PostToolUse", "response": response}))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pre(name: &str, id: &str) -> Value {
        json!({"tool_name": name, "tool_use_id": id, "tool_input": {"command": "ls"}})
    }

    fn post(id: &str) -> Value {
        json!({"tool_use_id": id, "tool_response": {"stdout": "ok"}})
    }

    /// The hook order of a real Session (claude 2.1.327): the code's PreToolUse, its calls, a `Poll`
    /// made meanwhile, and the code's PostToolUse once it ended.
    #[test]
    fn calls_are_reported_with_the_code_that_runs() {
        let mut c = CodeCalls::default();
        assert_eq!(c.on_hook("PreToolUse", &pre("REPL", "toolu_r1")), None);
        let ev = c.on_hook("PreToolUse", &pre("Bash", "repl_a")).unwrap();
        assert_eq!(
            ev,
            json!({"type": "code_call", "tool_use_id": "repl_a", "name": "Bash",
                "input": {"command": "ls"}, "code": "toolu_r1"})
        );
        assert_eq!(c.on_hook("PreToolUse", &pre("Poll", "toolu_p1")), None);
        assert_eq!(
            c.on_hook("PostToolUse", &post("repl_a")),
            Some(
                json!({"type": "code_call_end", "tool_use_id": "repl_a", "is_error": false,
                "response": {"stdout": "ok"}})
            )
        );
        // Ended once: a second end of the same call says nothing.
        assert_eq!(c.on_hook("PostToolUse", &post("repl_a")), None);
        let failed = c.on_hook("PreToolUse", &pre("Read", "repl_b")).unwrap();
        assert_eq!(failed["code"], "toolu_r1");
        assert_eq!(
            c.on_hook("PostToolUseFailure", &json!({"tool_use_id": "repl_b"})),
            Some(
                json!({"type": "code_call_end", "tool_use_id": "repl_b", "is_error": true,
                "response": null})
            )
        );
        // The code ended: a call after it (a script still running, or one ccrpc attached during)
        // names no code.
        assert_eq!(c.on_hook("PostToolUse", &post("toolu_r1")), None);
        assert_eq!(c.on_hook("PostToolUse", &post("toolu_p1")), None);
        let late = c.on_hook("PreToolUse", &pre("Bash", "repl_c")).unwrap();
        assert_eq!(late["code"], Value::Null);
    }

    #[test]
    fn a_large_response_is_left_out() {
        let mut c = CodeCalls::default();
        c.on_hook("PreToolUse", &pre("Read", "repl_a"));
        let big = json!({"tool_use_id": "repl_a", "tool_response": "x".repeat(RESPONSE_BYTES + 1)});
        let ev = c.on_hook("PostToolUse", &big).unwrap();
        assert_eq!(ev["response"], Value::Null);
    }
}
