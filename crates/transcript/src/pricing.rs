//! API cost of a message, following ccusage (`ccusage-core/src/cost.rs`) over LiteLLM's
//! `model_prices_and_context_window.json`:
//! - each token bucket at its own rate: input, output, cache read, cache write — 5-minute writes
//!   at `cache_creation_input_token_cost`, 1-hour writes at `…_above_1hr` (else 2× input); without
//!   the 5m/1h breakdown every cache write is a 5-minute one;
//! - `*_above_200k_tokens` rates apply to the part of a bucket beyond 200K tokens;
//! - fast mode (`usage.speed == "fast"`) multiplies the whole message by the model's fast factor;
//! - a cost Claude Code logged itself (`costUSD`, older versions) wins over the calculation.
//!
//! Only `claude-*` models are kept. The table comes from the cache of the last online fetch or
//! the embedded snapshot (`pricing-snapshot.json`, refreshed with `examples/update_pricing.rs`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

const SNAPSHOT: &str = include_str!("pricing-snapshot.json");
const LONG_CONTEXT_THRESHOLD: f64 = 200_000.0;
const CACHE_WRITE_1H_INPUT_MULTIPLIER: f64 = 2.0;
/// Fast-mode factors for models whose LiteLLM entry lacks `provider_specific_entry.fast`
/// (ccusage's `fast-multiplier-overrides.json`), matched by prefix.
const FAST_FALLBACK: &[(&str, f64)] = &[
    ("claude-opus-4-6", 6.0),
    ("claude-opus-4-7", 6.0),
    ("claude-opus-4-8", 2.0),
];

/// Per-token USD rates of one model (LiteLLM field names).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ModelPrice {
    #[serde(default)]
    pub input_cost_per_token: f64,
    #[serde(default)]
    pub output_cost_per_token: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_token_cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_token_cost_above_1hr: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_token_cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_cost_per_token_above_200k_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_cost_per_token_above_200k_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_token_cost_above_200k_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_token_cost_above_200k_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_specific_entry: Option<ProviderSpecific>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ProviderSpecific {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast: Option<f64>,
}

/// Token counts of one assistant message.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
    pub fast: bool,
}

impl Usage {
    fn is_empty(&self) -> bool {
        self.input + self.output + self.cache_read + self.cache_write_5m + self.cache_write_1h
            <= 0.0
    }
}

/// `claude-*` model → rates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PriceTable {
    models: BTreeMap<String, ModelPrice>,
}

impl PriceTable {
    /// The snapshot compiled into the app.
    pub fn embedded() -> Self {
        Self::from_litellm(SNAPSHOT).expect("embedded pricing snapshot parses")
    }

    /// Parses LiteLLM's price JSON (or a subset of it), keeping `claude-*` entries only.
    /// `None` when nothing usable is in it.
    pub fn from_litellm(json: &str) -> Option<Self> {
        let all: BTreeMap<String, serde_json::Value> = serde_json::from_str(json).ok()?;
        let models: BTreeMap<String, ModelPrice> = all
            .into_iter()
            .filter(|(k, _)| k.starts_with("claude-"))
            .filter_map(|(k, v)| Some((k, serde_json::from_value::<ModelPrice>(v).ok()?)))
            .filter(|(_, p)| p.input_cost_per_token > 0.0 || p.output_cost_per_token > 0.0)
            .collect();
        (!models.is_empty()).then_some(PriceTable { models })
    }

    /// The kept entries in LiteLLM's shape (what the snapshot and the fetch cache store).
    pub fn to_litellm_json(&self) -> String {
        serde_json::to_string_pretty(&self.models).expect("price table serializes")
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Exact name, else without a `[1m]`-style or `@region` suffix, else without a date suffix.
    pub fn find(&self, model: &str) -> Option<&ModelPrice> {
        let lower = model.trim().to_ascii_lowercase();
        let base = lower
            .split(['[', '@'])
            .next()
            .unwrap_or_default()
            .trim_end_matches('-');
        let undated = strip_date(base);
        [lower.as_str(), base, undated]
            .into_iter()
            .find_map(|m| self.models.get(m))
    }

    /// USD cost of one message; `None` when its model has no price (and nothing was logged).
    /// A message without tokens costs 0 whatever its model.
    pub fn cost(&self, model: Option<&str>, usage: &Usage, logged: Option<f64>) -> Option<f64> {
        if let Some(c) = logged {
            return Some(c);
        }
        if usage.is_empty() {
            return Some(0.0);
        }
        let name = model?;
        let p = self.find(name)?;
        let input = p.input_cost_per_token;
        let write_5m = p.cache_creation_input_token_cost.unwrap_or(input * 1.25);
        let write_1h = p
            .cache_creation_input_token_cost_above_1hr
            .unwrap_or(input * CACHE_WRITE_1H_INPUT_MULTIPLIER);
        let write_1h_above = p
            .input_cost_per_token_above_200k_tokens
            .map(|c| c * CACHE_WRITE_1H_INPUT_MULTIPLIER);
        let read = p.cache_read_input_token_cost.unwrap_or(input * 0.1);
        let cost = tiered(usage.input, input, p.input_cost_per_token_above_200k_tokens)
            + tiered(
                usage.output,
                p.output_cost_per_token,
                p.output_cost_per_token_above_200k_tokens,
            )
            + tiered(
                usage.cache_write_5m,
                write_5m,
                p.cache_creation_input_token_cost_above_200k_tokens,
            )
            + tiered(usage.cache_write_1h, write_1h, write_1h_above)
            + tiered(
                usage.cache_read,
                read,
                p.cache_read_input_token_cost_above_200k_tokens,
            );
        Some(if usage.fast {
            cost * fast_multiplier(name, p)
        } else {
            cost
        })
    }
}

fn fast_multiplier(model: &str, p: &ModelPrice) -> f64 {
    p.provider_specific_entry
        .as_ref()
        .and_then(|e| e.fast)
        .or_else(|| {
            FAST_FALLBACK
                .iter()
                .find(|(prefix, _)| model.starts_with(prefix))
                .map(|&(_, m)| m)
        })
        .unwrap_or(1.0)
}

/// `claude-haiku-4-5-20251001` → `claude-haiku-4-5`.
fn strip_date(m: &str) -> &str {
    match m.rsplit_once('-') {
        Some((head, tail)) if tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => m,
    }
}

/// The part beyond 200K tokens at `above` (when the model has such a rate).
fn tiered(tokens: f64, base: f64, above: Option<f64>) -> f64 {
    if tokens <= 0.0 {
        return 0.0;
    }
    match above {
        Some(above) if tokens > LONG_CONTEXT_THRESHOLD => {
            LONG_CONTEXT_THRESHOLD * base + (tokens - LONG_CONTEXT_THRESHOLD) * above
        }
        _ => tokens * base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> PriceTable {
        PriceTable::from_litellm(
            r#"{
                "claude-test": {
                    "input_cost_per_token": 1.0,
                    "output_cost_per_token": 10.0,
                    "cache_creation_input_token_cost": 1.25,
                    "cache_read_input_token_cost": 0.1,
                    "input_cost_per_token_above_200k_tokens": 2.0,
                    "provider_specific_entry": { "fast": 2.0 }
                },
                "claude-1h": {
                    "input_cost_per_token": 1.0,
                    "output_cost_per_token": 10.0,
                    "cache_creation_input_token_cost": 1.25,
                    "cache_creation_input_token_cost_above_1hr": 3.0
                },
                "claude-opus-4-7": { "input_cost_per_token": 1.0, "output_cost_per_token": 1.0 },
                "claude-haiku-9": { "input_cost_per_token": 1.0, "output_cost_per_token": 1.0 },
                "gpt-x": { "input_cost_per_token": 1.0, "output_cost_per_token": 1.0 }
            }"#,
        )
        .unwrap()
    }

    fn cost(model: &str, u: Usage) -> f64 {
        table().cost(Some(model), &u, None).unwrap()
    }

    #[test]
    fn prices_each_bucket_and_splits_cache_writes_by_duration() {
        let u = Usage {
            input: 1.0,
            output: 2.0,
            cache_read: 30.0,
            cache_write_5m: 10.0,
            cache_write_1h: 20.0,
            fast: false,
        };
        // 1 + 20 + 3 + 12.5 + 20 × (2 × input)
        assert_eq!(cost("claude-test", u), 76.5);
        // An explicit 1-hour rate wins over 2× input.
        assert_eq!(
            cost(
                "claude-1h",
                Usage {
                    cache_write_1h: 10.0,
                    ..Usage::default()
                }
            ),
            30.0
        );
    }

    #[test]
    fn bills_the_part_beyond_200k_at_the_long_context_rate() {
        let u = Usage {
            input: 300_000.0,
            ..Usage::default()
        };
        assert_eq!(cost("claude-test", u), 200_000.0 + 100_000.0 * 2.0);
        // No long-context rate: flat.
        assert_eq!(cost("claude-1h", u), 300_000.0);
    }

    #[test]
    fn fast_mode_multiplies_the_whole_message() {
        let u = Usage {
            output: 1.0,
            fast: true,
            ..Usage::default()
        };
        assert_eq!(cost("claude-test", u), 20.0);
        // Missing from LiteLLM: ccusage's override table.
        assert_eq!(
            cost(
                "claude-opus-4-7",
                Usage {
                    input: 1.0,
                    fast: true,
                    ..Usage::default()
                }
            ),
            6.0
        );
        assert_eq!(
            cost(
                "claude-haiku-9",
                Usage {
                    input: 1.0,
                    fast: true,
                    ..Usage::default()
                }
            ),
            1.0
        );
    }

    #[test]
    fn matches_dated_and_suffixed_names_and_keeps_claude_only() {
        let t = table();
        assert!(t.find("claude-test-20250101").is_some());
        assert!(t.find("claude-test[1m]").is_some());
        assert!(t.find("Claude-Test@eu").is_some());
        assert!(t.find("claude-unknown").is_none());
        assert!(t.find("gpt-x").is_none());
        assert_eq!(t.len(), 4);
    }

    #[test]
    fn unpriced_models_and_logged_costs() {
        let t = table();
        let u = Usage {
            input: 1.0,
            ..Usage::default()
        };
        assert_eq!(t.cost(Some("claude-unknown"), &u, None), None);
        assert_eq!(t.cost(None, &u, None), None);
        assert_eq!(t.cost(Some("claude-unknown"), &u, Some(0.5)), Some(0.5));
        assert_eq!(t.cost(Some("claude-test"), &u, Some(0.5)), Some(0.5));
        // `<synthetic>` error messages carry no tokens.
        assert_eq!(
            t.cost(Some("<synthetic>"), &Usage::default(), None),
            Some(0.0)
        );
    }

    #[test]
    fn embedded_snapshot_prices_current_models() {
        let t = PriceTable::embedded();
        for m in [
            "claude-opus-4-8",
            "claude-sonnet-4-6",
            "claude-haiku-4-5-20251001",
        ] {
            assert!(t.find(m).is_some(), "{m}");
        }
        assert_eq!(PriceTable::from_litellm(&t.to_litellm_json()), Some(t));
    }
}
