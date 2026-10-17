//! Sampling passthrough — a model's declared sampler recipe, merged into the request body.
//!
//! Modern open-weight models ship a *required* sampling recipe rather than a suggestion, and
//! three of the keys a typical one names (`top_k`, `min_p`, `repeat_penalty`) have no field in the
//! OpenAI wire schema. Worse, the defaults are not neutral: llama.cpp's `min_p` is `0.05`, so a
//! request that simply omits it runs the model off-spec on every turn. So the map is passthrough,
//! not a modelled set of options — Luxe carries whatever the server understands and validates none
//! of it.
//!
//! OpenAI-shaped bodies only. Anthropic has no equivalent surface and ignores it.

use serde_json::{Map, Value};
use tracing::warn;

/// Fields a sampling map may not set, because they decide *what request this is* rather than how
/// it samples. Silently overwriting `stream` leaves the SSE reader waiting on a body that will
/// never stream, which is a hang with no diagnostic.
const RESERVED: [&str; 5] = ["model", "messages", "input", "stream", "tools"];

/// Merge a model's sampling parameters into a request body, last, so its keys win.
///
/// Called after every field the adapter sets itself — `temperature`, the output cap, the thinking
/// payload — which is the whole point: a model card's recipe is the final word on how that model
/// is sampled, and a key here beats whatever Luxe would otherwise have sent.
pub(crate) fn apply(body: &mut Value, params: &Map<String, Value>) {
    if params.is_empty() {
        return;
    }
    let Some(body) = body.as_object_mut() else { return };
    for (key, value) in params {
        if RESERVED.contains(&key.as_str()) {
            warn!(key, "ignoring a reserved key in sampling_params");
            continue;
        }
        body.insert(key.clone(), value.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(json: Value) -> Map<String, Value> {
        match json {
            Value::Object(map) => map,
            _ => unreachable!("fixture is an object"),
        }
    }

    /// The keys that have no OpenAI field are exactly the reason this exists, so they have to
    /// arrive verbatim — unrecognized names and all.
    #[test]
    fn unmodelled_samplers_reach_the_body_untouched() {
        let mut body = serde_json::json!({ "model": "qwen3.8-27b", "temperature": 0.2 });
        apply(
            &mut body,
            &params(serde_json::json!({
                "temperature": 1.0, "top_p": 0.95, "top_k": 20,
                "min_p": 0.0, "repeat_penalty": 1.0, "mirostat_tau": 5,
            })),
        );

        assert_eq!(body["temperature"], 1.0, "the recipe outranks what Luxe set");
        assert_eq!(body["top_k"], 20);
        assert_eq!(body["min_p"], 0.0);
        assert_eq!(body["repeat_penalty"], 1.0);
        assert_eq!(body["mirostat_tau"], 5, "nothing is validated; unknown keys pass through");
    }

    /// `min_p: 0` is the case that motivated the whole feature: llama.cpp's default is 0.05, so an
    /// omitted key is not a neutral one. A falsy value must survive the merge.
    #[test]
    fn a_zero_is_a_value_not_an_absence() {
        let mut body = serde_json::json!({});
        apply(&mut body, &params(serde_json::json!({ "min_p": 0.0, "presence_penalty": 0 })));
        assert_eq!(body["min_p"], 0.0);
        assert_eq!(body["presence_penalty"], 0);
    }

    /// A sampling map is not a way to rewrite the request. Turning off `stream` would leave the
    /// SSE reader waiting forever on a body that never streams.
    #[test]
    fn the_fields_that_decide_what_request_this_is_are_refused() {
        let mut body = serde_json::json!({
            "model": "real", "messages": ["real"], "stream": true, "tools": ["real"],
        });
        apply(
            &mut body,
            &params(serde_json::json!({
                "model": "other", "messages": [], "input": [], "stream": false,
                "tools": [], "top_k": 20,
            })),
        );

        assert_eq!(body["model"], "real");
        assert_eq!(body["messages"], serde_json::json!(["real"]));
        assert_eq!(body["stream"], true);
        assert_eq!(body["tools"], serde_json::json!(["real"]));
        assert!(body.get("input").is_none());
        assert_eq!(body["top_k"], 20, "the rest of the map still applies");
    }

    #[test]
    fn an_empty_map_changes_nothing() {
        let mut body = serde_json::json!({ "model": "m" });
        apply(&mut body, &Map::new());
        assert_eq!(body, serde_json::json!({ "model": "m" }));
    }
}
