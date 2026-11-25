//! The Main Line's teammates: who was spawned, and whether they work or wait.

use std::collections::HashMap;

use serde_json::Value;
use transcript::assemble::{prompt, subagent};
use transcript::raw::RawEntry;

#[derive(Debug, Default)]
pub(super) struct Teammates {
    /// The teammates, by what the Main Line says of them.
    teammates: subagent::Teammates,
    /// Teammate names by the `description` they were spawned with: how Stop's list names them.
    names: HashMap<String, Vec<String>>,
}

impl Teammates {
    /// Whether the teammates spawned with `description` that have not exited are all idle.
    pub(super) fn idle(&self, description: &str) -> bool {
        let Some(names) = self.names.get(description) else {
            return false;
        };
        let mut live = names
            .iter()
            .filter_map(|n| self.teammates.state(n))
            .filter(|s| *s != "exited")
            .peekable();
        live.peek().is_some() && live.all(|s| s == "idle")
    }

    /// A Main Line entry: the teammate messages it relays, the calls that spawn or message one.
    pub(super) fn note_entry(&mut self, e: &RawEntry) {
        self.note_teammate_messages(e);
        self.note_teammate_calls(e);
    }

    /// A Main Line entry relaying messages from teammates, delivered at once or from the queue.
    fn note_teammate_messages(&mut self, e: &RawEntry) {
        let text = match e.entry_type() {
            "user" if !prompt::is_human_prompt(e) => prompt::user_text(e),
            "attachment" => e
                .queued_command()
                .and_then(|q| q.text())
                .unwrap_or_default(),
            _ => return,
        };
        if text.contains("<teammate-message") {
            for m in prompt::teammate_messages(&text) {
                self.teammates.message(&m);
            }
        }
    }

    /// A Main Line entry's tool calls that spawn a teammate or send one a message.
    fn note_teammate_calls(&mut self, e: &RawEntry) {
        if e.entry_type() != "assistant" {
            return;
        }
        for b in e.blocks() {
            if b.block_type() != "tool_use" {
                continue;
            }
            let tool = b.name.as_deref().unwrap_or_default();
            if !matches!(tool, "Agent" | "Task" | "SendMessage") {
                continue;
            }
            let Some(input) = b
                .input
                .as_deref()
                .and_then(|r| serde_json::from_str::<Value>(r.get()).ok())
            else {
                continue;
            };
            let id = b.id.as_deref().unwrap_or_default();
            self.note_teammate_call(id, tool, &input, e.timestamp.as_deref());
        }
    }

    fn note_teammate_call(&mut self, id: &str, tool: &str, input: &Value, at: Option<&str>) {
        let field = |k: &str| input.get(k).and_then(Value::as_str);
        match tool {
            "Agent" | "Task" => {
                let Some(name) = field("name") else { return };
                self.teammates.spawned(name, id);
                if let Some(description) = field("description") {
                    let names = self.names.entry(description.to_string()).or_default();
                    if !names.iter().any(|n| n == name) {
                        names.push(name.to_string());
                    }
                }
            }
            "SendMessage" => {
                if let Some(to) = field("to") {
                    self.teammates.sent(to, at);
                }
            }
            _ => {}
        }
    }
}
