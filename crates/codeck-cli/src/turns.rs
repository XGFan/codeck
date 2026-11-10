//! Turn numbers: the one numbering `show --turn` and `search` hits share.
//!
//! A turn starts at each Human or Command prompt (in a Subagent Run, also a Coordinator one), as
//! in the viewer's turn outline; 1 is the first. What precedes the first prompt is turn 0.
//! Always computed over a Transcript with its hidden nodes, so a number does not depend on them.

use std::collections::HashMap;

use transcript::model::{Node, NodeBody, PromptOrigin, Transcript};

pub struct Turn<'a> {
    pub number: u32,
    pub prompt: Option<&'a Node>,
    pub nodes: Vec<&'a Node>,
}

fn starts_turn(n: &Node) -> bool {
    matches!(
        &n.body,
        NodeBody::UserPrompt {
            origin: PromptOrigin::Human | PromptOrigin::Command { .. } | PromptOrigin::Coordinator,
            ..
        }
    )
}

pub fn split(t: &Transcript) -> Vec<Turn<'_>> {
    let mut turns: Vec<Turn<'_>> = Vec::new();
    for n in &t.nodes {
        if starts_turn(n) {
            let number = turns.last().map_or(1, |l| l.number + 1);
            turns.push(Turn {
                number,
                prompt: Some(n),
                nodes: vec![n],
            });
        } else if let Some(last) = turns.last_mut() {
            last.nodes.push(n);
        } else {
            turns.push(Turn {
                number: 0,
                prompt: None,
                nodes: vec![n],
            });
        }
    }
    turns
}

/// Display node id → turn number.
pub fn numbers(t: &Transcript) -> HashMap<&str, u32> {
    split(t)
        .iter()
        .flat_map(|turn| turn.nodes.iter().map(|n| (n.id.as_str(), turn.number)))
        .collect()
}
