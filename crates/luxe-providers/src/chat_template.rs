//! Chat-template arguments — the `chat_template_kwargs` payload, described rather than hardcoded.
//!
//! Servers that render a Jinja chat template (raw `llama-server`, vLLM, SGLang) take their thinking
//! controls as template arguments rather than as wire fields, and every template names them
//! differently: Qwen3.8 accepts `enable_thinking`, `preserve_thinking` and `reasoning_effort`, the
//! next one will accept something else. So the shape is configuration — a map of literal values
//! and `$var` references resolved against the turn's thinking level.
//!
//! Note that LM Studio's HTTP layer *drops* `chat_template_kwargs` entirely; a model served through
//! it wants `thinking_format: "openai"` and a top-level `reasoning_effort` instead.

use serde_json::{Map, Value};
use tracing::warn;

/// The payload a template with nothing configured gets: Qwen's two flags, which is what Luxe sent
/// for every chat-template model before the map existed.
fn default_kwargs(enabled: bool) -> Value {
    serde_json::json!({ "enable_thinking": enabled, "preserve_thinking": true })
}

/// Build the `chat_template_kwargs` object for one request.
///
/// `template` is the configured map; an empty one falls back to [`default_kwargs`], so a config
/// written without one gets the defaults. Returns `None` when every configured key resolved away,
/// in which case the field is omitted rather than sent empty.
///
/// Args: `(template, enabled, effort)` — `enabled` is whether this turn thinks at all, `effort` the
/// provider value for its level (already clamped to one the model supports).
pub(crate) fn kwargs(template: &Map<String, Value>, enabled: bool, effort: &str) -> Option<Value> {
    if template.is_empty() {
        return Some(default_kwargs(enabled));
    }
    let resolved: Map<String, Value> = template
        .iter()
        .filter_map(|(key, value)| Some((key.clone(), resolve(key, value, enabled, effort)?)))
        .collect();
    (!resolved.is_empty()).then_some(Value::Object(resolved))
}

/// Resolve one configured value, or `None` to leave the key out.
///
/// Anything that isn't a `$var` reference is a literal and travels verbatim — a template argument
/// can be any JSON the template understands, and Luxe has no business inspecting it.
fn resolve(key: &str, value: &Value, enabled: bool, effort: &str) -> Option<Value> {
    let Some(reference) = value.as_object().filter(|object| object.contains_key("$var")) else {
        return Some(value.clone());
    };
    // A level-shaped argument is meaningless with thinking off, and some templates raise on one.
    let omit_when_off = ["omit_when_off", "omitWhenOff"]
        .iter()
        .any(|name| reference.get(*name).and_then(Value::as_bool).unwrap_or(false));
    if omit_when_off && !enabled {
        return None;
    }
    match reference.get("$var").and_then(Value::as_str) {
        Some("thinking.enabled") => Some(Value::Bool(enabled)),
        // `thinking.effort` is Pi's spelling of the same reference, accepted so a model entry can
        // be carried across without translation.
        Some("thinking.level" | "thinking.effort") => Some(Value::String(effort.to_string())),
        other => {
            // Skipped rather than guessed: sending the wrong argument is a template that renders
            // the wrong prompt, which reads as a bad model rather than a bad config.
            warn!(key, reference = other.unwrap_or("(not a string)"), "unknown chat-template $var");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template(json: Value) -> Map<String, Value> {
        match json {
            Value::Object(map) => map,
            _ => unreachable!("fixture is an object"),
        }
    }

    /// A config written before the map existed must send exactly what it always sent.
    #[test]
    fn nothing_configured_keeps_the_payload_luxe_always_sent() {
        assert_eq!(
            kwargs(&Map::new(), true, "high"),
            Some(serde_json::json!({ "enable_thinking": true, "preserve_thinking": true }))
        );
        assert_eq!(
            kwargs(&Map::new(), false, "none"),
            Some(serde_json::json!({ "enable_thinking": false, "preserve_thinking": true }))
        );
    }

    /// The Qwen3.8 template's real vocabulary: a flag, a literal, and a level.
    #[test]
    fn references_resolve_against_the_turns_thinking_level() {
        let configured = template(serde_json::json!({
            "enable_thinking": { "$var": "thinking.enabled" },
            "preserve_thinking": false,
            "reasoning_effort": { "$var": "thinking.level", "omit_when_off": true },
        }));

        assert_eq!(
            kwargs(&configured, true, "xhigh"),
            Some(serde_json::json!({
                "enable_thinking": true,
                "preserve_thinking": false,
                "reasoning_effort": "xhigh",
            })),
            "a literal `false` is a value the map is there to express"
        );
        assert_eq!(
            kwargs(&configured, false, "none"),
            Some(serde_json::json!({ "enable_thinking": false, "preserve_thinking": false })),
            "an effort argument is dropped when the turn does not think"
        );
    }

    /// Pi writes the same two references in camelCase; a model entry should carry across whole.
    #[test]
    fn pis_spelling_of_the_same_references_is_accepted() {
        let configured = template(serde_json::json!({
            "reasoning_effort": { "$var": "thinking.effort", "omitWhenOff": true },
        }));
        assert_eq!(
            kwargs(&configured, true, "medium"),
            Some(serde_json::json!({ "reasoning_effort": "medium" }))
        );
        assert_eq!(kwargs(&configured, false, "none"), None, "and the whole field goes with it");
    }

    /// Guessing at a misspelled reference would render the wrong prompt, which looks like a bad
    /// model rather than a bad config.
    #[test]
    fn an_unknown_reference_is_skipped_rather_than_guessed() {
        let configured = template(serde_json::json!({
            "reasoning_effort": { "$var": "thinking.levl" },
            "enable_thinking": { "$var": "thinking.enabled" },
        }));
        assert_eq!(
            kwargs(&configured, true, "high"),
            Some(serde_json::json!({ "enable_thinking": true }))
        );
    }

    /// A template argument can be any JSON the template understands.
    #[test]
    fn a_value_that_is_not_a_reference_travels_verbatim() {
        let configured = template(serde_json::json!({
            "tools_in_user_message": true,
            "budget": 4096,
            "persona": "terse",
            "stops": ["</think>"],
            "nested": { "a": 1 },
            "nothing": null,
        }));
        assert_eq!(kwargs(&configured, true, "high"), Some(Value::Object(configured)));
    }
}
