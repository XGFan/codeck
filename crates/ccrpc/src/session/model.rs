//! The Session's model as `session_info`, `ready` and `get_state` give it.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct Model {
    /// The main line's last reply's model id, before any reply the one the Session started with;
    /// none while `/model` has set a model that has not answered yet.
    pub(super) id: Option<String>,
    /// The display name: what `/model` said, else made from the id.
    pub(super) name: String,
}

impl Model {
    /// `/model` said it set `name`.
    pub(super) fn set(name: String) -> Model {
        Model { id: None, name }
    }

    /// The model a reply was made by, or the Session started with. A name `/model` gave stays for
    /// the first reply that follows it; a reply from another model than the last one (not through
    /// `/model`) names its own.
    pub(super) fn replied(current: Option<&Model>, id: &str) -> Model {
        match current {
            Some(m) if m.id.as_deref() == Some(id) => m.clone(),
            Some(Model { id: None, name }) => Model {
                id: Some(id.to_string()),
                name: name.clone(),
            },
            _ => Model {
                id: Some(id.to_string()),
                name: display_name(id),
            },
        }
    }
}

/// `claude-opus-4-7` → `Opus 4.7`, `claude-fable-5-1[1m]` → `Fable 5.1 (1M)`,
/// `claude-sonnet-4-5-20250929` → `Sonnet 4.5`; an id of another shape is its own name.
/// (`^claude-([a-z]+)-(\d+)(?:-(\d{1,2}))?(?:-\d{8})?(\[1m\])?$`, by hand: no regex dependency.)
pub(super) fn display_name(id: &str) -> String {
    parse_id(id).unwrap_or_else(|| id.to_string())
}

fn parse_id(id: &str) -> Option<String> {
    let rest = id.strip_prefix("claude-")?;
    let (rest, one_m) = match rest.strip_suffix("[1m]") {
        Some(r) => (r, true),
        None => (rest, false),
    };
    let digits = |s: &str, max: usize| {
        !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_digit())
    };
    let mut parts = rest.split('-');
    let family = parts
        .next()
        .filter(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_lowercase()))?;
    let major = parts.next().filter(|m| digits(m, usize::MAX))?;
    let tail: Vec<&str> = parts.collect();
    let minor = match tail.as_slice() {
        [] => None,
        [d] if digits(d, 8) && d.len() == 8 => None,
        [m] if digits(m, 2) => Some(*m),
        [m, d] if digits(m, 2) && d.len() == 8 && digits(d, 8) => Some(*m),
        _ => return None,
    };
    let mut family = family.to_string();
    family[..1].make_ascii_uppercase();
    let mut name = format!("{family} {major}");
    if let Some(m) = minor {
        name.push('.');
        name.push_str(m);
    }
    if one_m {
        name.push_str(" (1M)");
    }
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_come_from_ids() {
        assert_eq!(display_name("claude-opus-4-7"), "Opus 4.7");
        assert_eq!(display_name("claude-fable-5-1[1m]"), "Fable 5.1 (1M)");
        assert_eq!(display_name("claude-sonnet-4-5-20250929"), "Sonnet 4.5");
        assert_eq!(display_name("claude-opus-4-20250514"), "Opus 4");
        assert_eq!(display_name("claude-haiku-4"), "Haiku 4");
        // Another shape is its own name.
        assert_eq!(display_name("gpt-5"), "gpt-5");
        assert_eq!(display_name("claude-3-5-sonnet"), "claude-3-5-sonnet");
        assert_eq!(display_name("claude-opus-4-7-123"), "claude-opus-4-7-123");
    }

    #[test]
    fn a_set_name_stays_for_the_first_reply_and_a_new_id_renames() {
        let set = Model::set("Sonnet 4.5".into());
        assert_eq!(set.id, None);
        // The reply after `/model` keeps what `/model` said.
        let first = Model::replied(Some(&set), "claude-sonnet-4-5-20250929");
        assert_eq!(first.name, "Sonnet 4.5");
        assert_eq!(first.id.as_deref(), Some("claude-sonnet-4-5-20250929"));
        assert_eq!(
            Model::replied(Some(&first), "claude-sonnet-4-5-20250929"),
            first
        );
        // Another id without `/model` in between names itself.
        let other = Model::replied(Some(&first), "claude-opus-4-7");
        assert_eq!(other.name, "Opus 4.7");
        // Nothing seen before: from the id.
        assert_eq!(Model::replied(None, "claude-opus-4-7"), other);
    }
}
