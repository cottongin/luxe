//! models.dev `api.json` schema — the subset Luxe actually uses.
//!
//! The wire shape is `{ providerId: { models: { modelId: {…} } } }`. Unknown fields are ignored
//! on the way in and omitted on the way out, so the same types parse the live API, the on-disk
//! cache, and the embedded snapshot, and re-serialize to the compact form we persist.

use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;

/// A models.dev catalog: provider id → provider block.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Catalog {
    /// Provider blocks keyed by models.dev provider id (`anthropic`, `openai`, …).
    #[serde(flatten)]
    pub providers: HashMap<String, Provider>,
}

/// One provider block.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Provider {
    /// Human provider name (e.g. `Anthropic`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Models keyed by model id.
    #[serde(
        default,
        deserialize_with = "lenient_models",
        skip_serializing_if = "HashMap::is_empty"
    )]
    pub models: HashMap<String, Model>,
}

/// Parse models one at a time, dropping any that don't fit the schema.
///
/// models.dev is community-maintained and occasionally ships an entry with, say, a `null` inside
/// a string array. Strict parsing would turn that into "Luxe knows nothing about any model" —
/// the exact failure this whole module exists to prevent. Losing one entry is a far better trade.
fn lenient_models<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<HashMap<String, Model>, D::Error> {
    let raw = HashMap::<String, serde_json::Value>::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .filter_map(|(id, value)| match serde_json::from_value(value) {
            Ok(model) => Some((id, model)),
            Err(error) => {
                tracing::debug!(model = %id, %error, "skipping malformed models.dev entry");
                None
            }
        })
        .collect())
}

/// One model entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Model {
    /// Display name (e.g. `Claude Opus 5`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether the model supports reasoning/thinking
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    /// Whether the model can call tools — a hard requirement for agentic use
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<bool>,
    /// Release date (`YYYY-MM-DD`), used to sort newest-first in the picker
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    /// Lifecycle marker (`alpha` / `beta` / `deprecated`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Context/output token limits
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<Limit>,
    /// Cost per million tokens
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
    /// Supported input/output modalities
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modalities: Option<Modalities>,
    /// Declared reasoning-effort options, used to derive a thinking-level map
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning_options: Vec<ReasoningOption>,
}

/// Context/output token limits.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Limit {
    /// Context window in tokens
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<u32>,
    /// Max output tokens
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<u32>,
}

/// Cost per million tokens.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Cost {
    /// Input cost per million tokens
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<f64>,
    /// Output cost per million tokens
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<f64>,
    /// Cache-hit read cost per million tokens
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    /// Cache-write cost per million tokens
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
    /// Dearer rates past a prompt size
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tiers: Vec<CostTier>,
    /// The older spelling of a single tier at 200k tokens
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_over_200k: Option<TierRates>,
}

/// One entry of `cost.tiers`: rates, and the prompt size they start past.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostTier {
    /// The rates themselves
    #[serde(flatten)]
    pub rates: TierRates,
    /// What the tier is measured on
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<TierBound>,
}

/// A tier's rates per million tokens.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TierRates {
    /// Input cost per million tokens
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<f64>,
    /// Output cost per million tokens
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<f64>,
    /// Cache-hit read cost per million tokens
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    /// Cache-write cost per million tokens
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}

/// Where a tier starts: `{"type": "context", "size": 100000}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TierBound {
    /// What is measured; only `context` is understood
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Tokens past which the tier applies
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

/// Input/output modality lists.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Modalities {
    /// Supported input modalities (`text`, `image`, `pdf`, …)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input: Vec<String>,
    /// Supported output modalities
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output: Vec<String>,
}

/// A declared reasoning-effort option group (`{type: "effort", values: […]}`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReasoningOption {
    /// Option kind (typically `effort`)
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Allowed values (e.g. `low`/`medium`/`high`/`xhigh`/`max`)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
}

impl Catalog {
    /// Look up one model by models.dev provider id + model id, retrying with a normalized id
    /// (see [`crate::catalog::alias::normalize_model_id`]) when the exact id is absent, then
    /// ignoring case.
    ///
    /// Case last, and only on a miss: models.dev ids are lowercase, but an Azure deployment of a
    /// partner model is reported as `DeepSeek-V4-Flash`, and that is the same model.
    pub fn model(&self, provider: &str, id: &str) -> Option<&Model> {
        let models = &self.providers.get(provider)?.models;
        let exact = |id: &str| {
            models
                .get(id)
                .or_else(|| super::alias::normalize_model_id(id).and_then(|base| models.get(base)))
        };
        exact(id).or_else(|| {
            let lower = id.to_ascii_lowercase();
            (lower != id).then(|| exact(&lower)).flatten()
        })
    }

    /// Iterate `(model_id, model)` for a provider, or nothing when it's absent.
    pub fn provider_models(&self, provider: &str) -> impl Iterator<Item = (&String, &Model)> {
        self.providers.get(provider).into_iter().flat_map(|block| block.models.iter())
    }

    /// Total model count across every provider.
    pub fn len(&self) -> usize {
        self.providers.values().map(|block| block.models.len()).sum()
    }

    /// Whether the catalog carries any provider at all.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

/// Parse a raw models.dev `api.json` body.
pub fn parse(json: &str) -> Result<Catalog, serde_json::Error> {
    serde_json::from_str(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "anthropic": {
            "id": "anthropic",
            "name": "Anthropic",
            "env": ["ANTHROPIC_API_KEY"],
            "models": {
                "claude-opus-5": {
                    "id": "claude-opus-5",
                    "name": "Claude Opus 5",
                    "description": "ignored",
                    "reasoning": true,
                    "tool_call": true,
                    "release_date": "2026-07-24",
                    "reasoning_options": [{"type":"effort","values":["low","high","max"]}],
                    "modalities": {"input":["text","image","pdf"],"output":["text"]},
                    "limit": {"context": 1000000, "output": 128000},
                    "cost": {"input": 5.0, "output": 25.0, "cache_read": 0.5, "cache_write": 6.25}
                }
            }
        }
    }"#;

    #[test]
    fn parses_the_real_nested_shape_and_ignores_unknown_fields() {
        let catalog = parse(SAMPLE).expect("real models.dev shape parses");
        let model = catalog.model("anthropic", "claude-opus-5").expect("model present");
        assert_eq!(model.name.as_deref(), Some("Claude Opus 5"));
        assert_eq!(model.reasoning, Some(true));
        assert_eq!(model.limit.as_ref().and_then(|l| l.context), Some(1_000_000));
        assert_eq!(model.cost.as_ref().and_then(|c| c.cache_read), Some(0.5));
        assert_eq!(model.reasoning_options[0].values, ["low", "high", "max"]);
        assert_eq!(model.reasoning_options[0].kind.as_deref(), Some("effort"));
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog.provider_models("missing").count(), 0);
    }

    #[test]
    fn round_trips_through_serialize_without_losing_fields() {
        let catalog = parse(SAMPLE).expect("parse");
        let written = serde_json::to_string(&catalog).expect("serialize");
        let back = parse(&written).expect("reparse");
        let model = back.model("anthropic", "claude-opus-5").expect("model survives");
        assert_eq!(model.limit.as_ref().and_then(|l| l.output), Some(128_000));
        assert_eq!(model.cost.as_ref().and_then(|c| c.cache_write), Some(6.25));
        assert_eq!(model.release_date.as_deref(), Some("2026-07-24"));
        assert_eq!(model.tool_call, Some(true));
        // Empty/None fields are omitted, keeping the persisted cache small.
        assert!(!written.contains("status"));
    }

    #[test]
    fn one_malformed_entry_does_not_sink_the_catalog() {
        // `values: [null]` is real upstream data (sarvam-*, 2026-07). The good model must survive.
        let catalog = parse(
            r#"{"sarvam":{"models":{
                "broken":{"reasoning_options":[{"type":"effort","values":[null]}]},
                "fine":{"name":"Fine","limit":{"context":32768}}}}}"#,
        )
        .expect("catalog still parses");
        assert!(catalog.model("sarvam", "broken").is_none(), "bad entry dropped");
        assert_eq!(catalog.model("sarvam", "fine").and_then(|m| m.name.as_deref()), Some("Fine"));
    }

    #[test]
    fn dated_and_latest_suffixes_fall_back_to_the_base_id() {
        let catalog = parse(SAMPLE).expect("parse");
        assert!(catalog.model("anthropic", "claude-opus-5-20260724").is_some());
        assert!(catalog.model("anthropic", "claude-opus-5-latest").is_some());
        assert!(catalog.model("anthropic", "claude-opus-6").is_none());
    }
}
