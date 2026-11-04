//! Tolerant serde structs for Claude Code JSONL entries.
//!
//! Every field is optional, unknown fields are ignored, and a field with an unexpected JSON type
//! becomes `None` instead of failing the line (see [`lenient`]). Heavy payloads (`input`,
//! tool_result `content`, `text`, `thinking`, image `source`, `toolUseResult`, `attachment`) stay
//! as [`RawValue`] so the light parse never builds a tree for them (A6).

use serde::Deserialize;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::value::RawValue;

/// One JSONL line.
#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct RawEntry {
    #[serde(rename = "type", default, deserialize_with = "lenient::string")]
    pub entry_type: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub subtype: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub uuid: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub parent_uuid: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub logical_parent_uuid: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: Option<String>,
    /// RFC 3339; see [`RawEntry::timestamp_ms`].
    #[serde(default, deserialize_with = "lenient::string")]
    pub timestamp: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub cwd: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub git_branch: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub version: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub agent_id: Option<String>,
    #[serde(default, deserialize_with = "lenient::boolean")]
    pub is_sidechain: Option<bool>,
    #[serde(default, deserialize_with = "lenient::boolean")]
    pub is_meta: Option<bool>,
    #[serde(default, deserialize_with = "lenient::boolean")]
    pub is_compact_summary: Option<bool>,
    #[serde(default, deserialize_with = "lenient::boolean")]
    pub is_api_error_message: Option<bool>,
    /// Cost Claude Code itself logged on assistant entries (older versions only).
    #[serde(rename = "costUSD", default, deserialize_with = "lenient::number")]
    pub cost_usd: Option<f64>,
    /// `system` entries.
    #[serde(default, deserialize_with = "lenient::string")]
    pub level: Option<String>,
    /// `system` / `queue-operation` content (usually a string).
    #[serde(default)]
    pub content: Option<Box<RawValue>>,
    #[serde(default, deserialize_with = "lenient::object")]
    pub message: Option<RawMessage>,
    /// Object, string or array depending on the tool; read lazily via [`RawEntry::tool_use_result_fields`].
    #[serde(default)]
    pub tool_use_result: Option<Box<RawValue>>,
    #[serde(
        rename = "sourceToolAssistantUUID",
        default,
        deserialize_with = "lenient::string"
    )]
    pub source_tool_assistant_uuid: Option<String>,
    #[serde(default, deserialize_with = "lenient::object")]
    pub origin: Option<RawOrigin>,
    #[serde(default)]
    pub attachment: Option<Box<RawValue>>,
    /// `last-prompt`.
    #[serde(default, deserialize_with = "lenient::string")]
    pub leaf_uuid: Option<String>,
    /// `custom-title`.
    #[serde(default, deserialize_with = "lenient::string")]
    pub custom_title: Option<String>,
    /// `ai-title`.
    #[serde(default, deserialize_with = "lenient::string")]
    pub ai_title: Option<String>,
    /// `agent-name`.
    #[serde(default, deserialize_with = "lenient::string")]
    pub agent_name: Option<String>,
    #[serde(default, deserialize_with = "lenient::object")]
    pub forked_from: Option<RawForkedFrom>,
    /// `compact_boundary`.
    #[serde(default, deserialize_with = "lenient::object")]
    pub compact_metadata: Option<RawCompactMetadata>,
}

/// `attachment` of a `queued_command` entry: input that arrived while Claude was busy (a prompt
/// typed mid-turn, a background-task notification, a teammate message), delivered on the next
/// turn. Claude Code records it only here, never as a `user` entry.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawQueuedCommand {
    #[serde(rename = "type", default, deserialize_with = "lenient::string")]
    attachment_type: Option<String>,
    /// The message content: a string or content blocks (text + images).
    #[serde(default)]
    prompt: Option<Box<RawValue>>,
    /// `prompt`, `task-notification`, …
    #[serde(default, deserialize_with = "lenient::string")]
    command_mode: Option<String>,
    #[serde(default, deserialize_with = "lenient::object")]
    origin: Option<RawOrigin>,
}

impl RawEntry {
    pub fn entry_type(&self) -> &str {
        self.entry_type.as_deref().unwrap_or("")
    }

    /// Turns a `queued_command` attachment into the `user` entry it stands for (same uuid,
    /// parent and timestamp), so it is classified, rendered, counted and indexed like any
    /// other user message. Older entries without `origin` get it from `commandMode`.
    pub fn normalize_queued_command(&mut self) {
        if self.entry_type() != "attachment" {
            return;
        }
        let Some(raw) = self.attachment.as_deref().map(RawValue::get) else {
            return;
        };
        if !raw.contains("\"queued_command\"") {
            return;
        }
        let Ok(q) = serde_json::from_str::<RawQueuedCommand>(raw) else {
            return;
        };
        let Some(prompt) = q
            .prompt
            .filter(|_| q.attachment_type.as_deref() == Some("queued_command"))
        else {
            return;
        };
        let Ok(message) = serde_json::from_str::<RawMessage>(&format!(
            r#"{{"role":"user","content":{}}}"#,
            prompt.get()
        )) else {
            return;
        };
        self.entry_type = Some("user".to_owned());
        self.message = Some(message);
        self.origin = q.origin.or_else(|| {
            (q.command_mode.as_deref() == Some("task-notification")).then(|| RawOrigin {
                kind: Some("task-notification".to_owned()),
            })
        });
        self.attachment = None;
    }

    /// Turns a `system` `local_command` entry (newer Claude Code records local slash commands
    /// such as `/rename` and their output this way) into the `user` entry older versions wrote
    /// for the same thing, so the command and its output render, count and index alike.
    pub fn normalize_local_command(&mut self) {
        if self.entry_type() != "system" || self.subtype.as_deref() != Some("local_command") {
            return;
        }
        let Some(content) = self.content.take() else {
            return;
        };
        let raw = content.get();
        let known = ["<command-name>", "<command-message>", "<local-command-std"]
            .iter()
            .any(|p| raw.starts_with(&format!("\"{p}")));
        let message = known
            .then(|| {
                serde_json::from_str::<RawMessage>(&format!(r#"{{"role":"user","content":{raw}}}"#))
                    .ok()
            })
            .flatten();
        let Some(message) = message else {
            self.content = Some(content);
            return;
        };
        self.entry_type = Some("user".to_owned());
        self.subtype = None;
        self.message = Some(message);
    }

    /// Unix milliseconds parsed from `timestamp`.
    pub fn timestamp_ms(&self) -> Option<f64> {
        parse_timestamp_ms(self.timestamp.as_deref()?)
    }

    /// Content blocks of `message.content` (empty when content is a plain string or absent).
    pub fn blocks(&self) -> &[RawBlock] {
        match self.message.as_ref().and_then(|m| m.content.as_ref()) {
            Some(RawContent::Blocks(b)) => b,
            _ => &[],
        }
    }

    /// `message.content` when it is a plain string.
    pub fn content_text(&self) -> Option<&str> {
        match self.message.as_ref().and_then(|m| m.content.as_ref()) {
            Some(RawContent::Text(t)) => Some(t),
            _ => None,
        }
    }

    /// A user entry carrying a tool result (`toolUseResult` present or a `tool_result` block).
    pub fn is_tool_result(&self) -> bool {
        self.tool_use_result.is_some()
            || self
                .blocks()
                .iter()
                .any(|b| b.block_type() == "tool_result")
    }

    /// The few `toolUseResult` fields assembly needs; `None` unless it is a JSON object.
    pub fn tool_use_result_fields(&self) -> Option<RawToolUseResult> {
        let raw = self.tool_use_result.as_ref()?;
        if !raw.get().starts_with('{') {
            return None;
        }
        serde_json::from_str(raw.get()).ok()
    }
}

/// Parses an RFC 3339 timestamp to unix milliseconds.
pub fn parse_timestamp_ms(s: &str) -> Option<f64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp_millis() as f64)
}

#[derive(Deserialize, Debug, Default)]
pub struct RawMessage {
    #[serde(default, deserialize_with = "lenient::string")]
    pub role: Option<String>,
    /// Assistant `message.id`; split fragments of one reply share it.
    #[serde(default, deserialize_with = "lenient::string")]
    pub id: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub model: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub stop_reason: Option<String>,
    #[serde(default, deserialize_with = "deserialize_content")]
    pub content: Option<RawContent>,
    #[serde(default, deserialize_with = "lenient::object")]
    pub usage: Option<RawUsage>,
}

#[derive(Debug)]
pub enum RawContent {
    Text(String),
    Blocks(Vec<RawBlock>),
}

/// Shallow content block: identifying fields parsed, payloads kept raw.
#[derive(Deserialize, Debug, Default)]
pub struct RawBlock {
    #[serde(rename = "type", default, deserialize_with = "lenient::string")]
    pub block_type: Option<String>,
    /// tool_use id.
    #[serde(default, deserialize_with = "lenient::string")]
    pub id: Option<String>,
    /// tool_use name.
    #[serde(default, deserialize_with = "lenient::string")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub tool_use_id: Option<String>,
    #[serde(default, deserialize_with = "lenient::boolean")]
    pub is_error: Option<bool>,
    #[serde(default)]
    pub input: Option<Box<RawValue>>,
    /// tool_result content: a string or an array of blocks.
    #[serde(default)]
    pub content: Option<Box<RawValue>>,
    #[serde(default)]
    pub text: Option<Box<RawValue>>,
    #[serde(default)]
    pub thinking: Option<Box<RawValue>>,
    /// image source (`{type, media_type, data}`).
    #[serde(default)]
    pub source: Option<Box<RawValue>>,
}

impl RawBlock {
    pub fn block_type(&self) -> &str {
        self.block_type.as_deref().unwrap_or("")
    }

    /// Decodes `text` when it is a JSON string.
    pub fn text_str(&self) -> Option<String> {
        decode_str(self.text.as_deref())
    }

    /// Decodes `thinking` when it is a JSON string.
    pub fn thinking_str(&self) -> Option<String> {
        decode_str(self.thinking.as_deref())
    }
}

/// Decodes a raw JSON value when it is a string.
pub fn decode_str(raw: Option<&RawValue>) -> Option<String> {
    let raw = raw?;
    if !raw.get().starts_with('"') {
        return None;
    }
    serde_json::from_str(raw.get()).ok()
}

#[derive(Deserialize, Debug, Default, Clone, PartialEq)]
pub struct RawUsage {
    #[serde(default, deserialize_with = "lenient::number")]
    pub input_tokens: Option<f64>,
    #[serde(default, deserialize_with = "lenient::number")]
    pub output_tokens: Option<f64>,
    #[serde(default, deserialize_with = "lenient::number")]
    pub cache_read_input_tokens: Option<f64>,
    #[serde(default, deserialize_with = "lenient::number")]
    pub cache_creation_input_tokens: Option<f64>,
    /// 5-minute / 1-hour split of `cache_creation_input_tokens` (priced differently).
    #[serde(default, deserialize_with = "lenient::object")]
    pub cache_creation: Option<RawCacheCreation>,
    /// `standard` or `fast` (fast mode is billed at a model-specific multiple).
    #[serde(default, deserialize_with = "lenient::string")]
    pub speed: Option<String>,
}

#[derive(Deserialize, Debug, Default, Clone, PartialEq)]
pub struct RawCacheCreation {
    #[serde(default, deserialize_with = "lenient::number")]
    pub ephemeral_5m_input_tokens: Option<f64>,
    #[serde(default, deserialize_with = "lenient::number")]
    pub ephemeral_1h_input_tokens: Option<f64>,
}

#[derive(Deserialize, Debug, Default, Clone, PartialEq)]
pub struct RawOrigin {
    /// `human`, `task-notification`, `peer`, …
    #[serde(default, deserialize_with = "lenient::string")]
    pub kind: Option<String>,
}

#[derive(Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RawForkedFrom {
    #[serde(default, deserialize_with = "lenient::string")]
    pub session_id: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub message_uuid: Option<String>,
}

#[derive(Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RawCompactMetadata {
    #[serde(default, deserialize_with = "lenient::string")]
    pub trigger: Option<String>,
    #[serde(default, deserialize_with = "lenient::number")]
    pub pre_tokens: Option<f64>,
}

/// Subset of an object-shaped `toolUseResult`.
#[derive(Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RawToolUseResult {
    /// Agent results (`async_launched` / `completed`).
    #[serde(default, deserialize_with = "lenient::string")]
    pub agent_id: Option<String>,
    /// Teammate results (`teammate_spawned`) use snake case.
    #[serde(rename = "agent_id", default, deserialize_with = "lenient::string")]
    pub agent_id_snake: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub status: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub persisted_output_path: Option<String>,
    #[serde(default, deserialize_with = "lenient::number")]
    pub persisted_output_size: Option<f64>,
    /// Workflow results.
    #[serde(default, deserialize_with = "lenient::string")]
    pub run_id: Option<String>,
}

fn deserialize_content<'de, D: Deserializer<'de>>(d: D) -> Result<Option<RawContent>, D::Error> {
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = Option<RawContent>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("message content")
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(Some(RawContent::Text(v.to_owned())))
        }
        fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
            Ok(Some(RawContent::Text(v)))
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut blocks = Vec::new();
            while let Some(LenientBlock(b)) = seq.next_element()? {
                blocks.extend(b);
            }
            Ok(Some(RawContent::Blocks(blocks)))
        }
        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
            lenient::drain_map(map)?;
            Ok(None)
        }
        fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<Self::Value, D2::Error> {
            d.deserialize_any(self)
        }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
            Ok(None)
        }
    }
    d.deserialize_any(V)
}

/// A content block element; non-object elements become `None`.
struct LenientBlock(Option<RawBlock>);

impl<'de> Deserialize<'de> for LenientBlock {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        lenient::object(d).map(LenientBlock)
    }
}

/// `deserialize_with` helpers that turn type mismatches into `None` instead of errors.
pub mod lenient {
    use std::marker::PhantomData;

    use serde::Deserialize;
    use serde::de::value::MapAccessDeserializer;
    use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};

    use super::IgnoredAny;

    enum Scalar {
        Str(String),
        Bool(bool),
        Num(f64),
        Other,
    }

    impl<'de> Deserialize<'de> for Scalar {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            d.deserialize_any(ScalarVisitor)
        }
    }

    struct ScalarVisitor;

    impl<'de> Visitor<'de> for ScalarVisitor {
        type Value = Scalar;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("any JSON value")
        }
        fn visit_bool<E: de::Error>(self, v: bool) -> Result<Scalar, E> {
            Ok(Scalar::Bool(v))
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Scalar, E> {
            Ok(Scalar::Num(v as f64))
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Scalar, E> {
            Ok(Scalar::Num(v as f64))
        }
        fn visit_f64<E: de::Error>(self, v: f64) -> Result<Scalar, E> {
            Ok(Scalar::Num(v))
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Scalar, E> {
            Ok(Scalar::Str(v.to_owned()))
        }
        fn visit_string<E: de::Error>(self, v: String) -> Result<Scalar, E> {
            Ok(Scalar::Str(v))
        }
        fn visit_unit<E: de::Error>(self) -> Result<Scalar, E> {
            Ok(Scalar::Other)
        }
        fn visit_none<E: de::Error>(self) -> Result<Scalar, E> {
            Ok(Scalar::Other)
        }
        fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Scalar, D::Error> {
            d.deserialize_any(self)
        }
        fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Scalar, A::Error> {
            drain_seq(seq)?;
            Ok(Scalar::Other)
        }
        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Scalar, A::Error> {
            drain_map(map)?;
            Ok(Scalar::Other)
        }
    }

    pub(super) fn drain_seq<'de, A: SeqAccess<'de>>(mut seq: A) -> Result<(), A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(())
    }

    pub(super) fn drain_map<'de, A: MapAccess<'de>>(mut map: A) -> Result<(), A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(())
    }

    pub fn string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
        Ok(match Scalar::deserialize(d)? {
            Scalar::Str(s) => Some(s),
            _ => None,
        })
    }

    pub fn boolean<'de, D: Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
        Ok(match Scalar::deserialize(d)? {
            Scalar::Bool(b) => Some(b),
            _ => None,
        })
    }

    pub fn number<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
        Ok(match Scalar::deserialize(d)? {
            Scalar::Num(n) => Some(n),
            _ => None,
        })
    }

    /// Deserializes `T` from a JSON object; any other JSON type yields `None`.
    pub fn object<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
        d: D,
    ) -> Result<Option<T>, D::Error> {
        struct V<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for V<T> {
            type Value = Option<T>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                T::deserialize(MapAccessDeserializer::new(map)).map(Some)
            }
            fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
                d.deserialize_any(self)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
                drain_seq(seq)?;
                Ok(None)
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(None)
            }
        }
        d.deserialize_any(V(PhantomData))
    }
}
