//! Layered model-metadata resolution — the one place a [`ModelInfo`] is built.
//!
//! Four layers, each overriding the previous field-by-field wherever it has a value:
//!
//! 1. **Defaults** — provider-aware floors, used when nothing else knows the model.
//! 2. **Snapshot** — the models.dev catalog embedded at build time; the offline/first-run floor.
//! 3. **Cache** — the refreshed on-disk models.dev catalog; stale is fine.
//! 4. **Config** — the user's `models.json`/`auth.json`, which always wins.
//!
//! Every entry point (startup, `--model`, headless, the loop, the `/model` picker, `luxe
//! models`) resolves through [`resolve`]. A layer only speaks where it has a value, so pinning
//! one field in config doesn't discard the rest. [`Provenance`] records which layer won each
//! field, which is what lets `luxe models show` explain a surprising context window.

use std::collections::HashMap;

use luxe_core::provider::{
    CompatFlags, Cost, Modality, ModelInfo, ModelRef, ThinkingControls, ThinkingLevel,
    PROVIDER_ONLY_COMPAT_KEYS,
};

use super::schema::{Catalog, Model};
use crate::config::{ModelConfig, ProviderConfig};

/// Context window assumed when no layer knows the model.
pub const DEFAULT_CONTEXT_WINDOW: u32 = 128_000;

/// Output cap for Anthropic when no layer knows the model.
///
/// Anthropic *requires* `max_tokens`, and a model no catalog knows is almost always a current one —
/// a fresh release, a Foundry deployment under its own name, a proxy — so this is the current
/// generation's ceiling rather than a safe floor. The old floor of 4,096 was safe and useless: with
/// thinking on, it truncated nearly every substantial reply. An older model that refuses this much
/// says what it does accept, and the adapter retries with that and remembers it. Everywhere else
/// `0` means "omit the field": OpenAI-compatible endpoints treat it as optional, an omitted cap is
/// the model's own maximum on every server luxe targets, and a forced large value trips strict ones.
const ANTHROPIC_FALLBACK_MAX_TOKENS: u32 = 128_000;

/// Which layer supplied a resolved field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layer {
    /// Built-in provider-aware default
    #[default]
    Default,
    /// The models.dev catalog embedded at build time
    Snapshot,
    /// The refreshed on-disk models.dev cache
    Cache,
    /// The user's `models.json` / `auth.json`
    Config,
}

impl Layer {
    /// Short label for `luxe models show`.
    pub fn label(self) -> &'static str {
        match self {
            Layer::Default => "default",
            Layer::Snapshot => "snapshot",
            Layer::Cache => "models.dev",
            Layer::Config => "config",
        }
    }
}

/// Which layer won each resolved field.
#[derive(Debug, Clone, Copy, Default)]
pub struct Provenance {
    /// Layer that supplied `display_name`
    pub display_name: Layer,
    /// Layer that supplied `context_window`
    pub context_window: Layer,
    /// Layer that supplied `max_tokens`
    pub max_tokens: Layer,
    /// Layer that supplied `reasoning`
    pub reasoning: Layer,
    /// Layer that supplied `thinking_level_map`
    pub thinking_level_map: Layer,
    /// Layer that supplied `default_thinking`
    pub default_thinking: Layer,
    /// Layer that supplied `compat`
    pub compat: Layer,
    /// Layer that supplied `sampling_params`
    pub sampling_params: Layer,
    /// Layer that supplied `thinking_controls`
    pub thinking_controls: Layer,
    /// Layer that supplied `input`
    pub input: Layer,
    /// Layer that supplied `cost`
    pub cost: Layer,
}

impl Provenance {
    /// Field name → winning layer, in display order.
    pub fn fields(&self) -> [(&'static str, Layer); 11] {
        [
            ("name", self.display_name),
            ("context window", self.context_window),
            ("max tokens", self.max_tokens),
            ("reasoning", self.reasoning),
            ("thinking levels", self.thinking_level_map),
            ("thinking default", self.default_thinking),
            ("thinking control", self.thinking_controls),
            ("compat", self.compat),
            ("sampling", self.sampling_params),
            ("modalities", self.input),
            ("cost", self.cost),
        ]
    }
}

/// The catalogs and config a resolution draws on. Any of them may be absent.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sources<'a> {
    /// models.dev embedded at build time
    pub snapshot: Option<&'a Catalog>,
    /// models.dev refreshed on disk
    pub cache: Option<&'a Catalog>,
    /// The user's entry for this exact `provider/id`
    pub model: Option<&'a ModelConfig>,
    /// The user's entry for this provider
    pub provider: Option<&'a ProviderConfig>,
}

/// Resolve a model's metadata from every available layer.
pub fn resolve(provider: &str, id: &str, sources: &Sources<'_>) -> (ModelInfo, Provenance) {
    // The floors below are properties of the *API*, not of a brand: Anthropic Messages requires
    // `max_tokens` and every model behind it reasons, whoever is hosting it. Keying on the name
    // would leave a Foundry or proxy deployment of the same API without an output cap.
    let is_anthropic = crate::config::resolve_api(provider, sources.provider)
        == luxe_core::auth::ApiKind::AnthropicMessages;
    let mut info = ModelInfo {
        r#ref: ModelRef::new(provider, id),
        display_name: None,
        context_window: DEFAULT_CONTEXT_WINDOW,
        max_tokens: if is_anthropic { ANTHROPIC_FALLBACK_MAX_TOKENS } else { 0 },
        reasoning: is_anthropic,
        thinking_level_map: HashMap::new(),
        default_thinking: None,
        compat: None,
        sampling_params: serde_json::Map::new(),
        thinking_controls: ThinkingControls::default(),
        input: vec![Modality::Text],
        cost: Cost::default(),
        // Whether the published rates are a charge or a plan equivalent is a property of the
        // provider, not of the model, so it comes from the provider catalog rather than models.dev.
        billing: match crate::auth::provider_descriptor(provider) {
            Some(descriptor) if descriptor.subscription_billed => {
                luxe_core::provider::Billing::Subscription
            }
            _ => luxe_core::provider::Billing::PerToken,
        },
    };
    let mut from = Provenance::default();

    let dev_provider = super::alias::models_dev_provider(provider, sources.provider);
    // A deployment is looked up under the model it runs; the wire still gets the deployment name
    let catalog_id = sources
        .provider
        .and_then(|provider| provider.deployments.get(id))
        .map_or(id, String::as_str);
    for (layer, catalog) in [(Layer::Snapshot, sources.snapshot), (Layer::Cache, sources.cache)] {
        if let Some(dev) = catalog.and_then(|c| c.model(dev_provider, catalog_id)) {
            apply_dev(&mut info, &mut from, layer, dev);
        }
    }
    // models.dev publishes no sampling recipe, so the only two layers are the user's own: a
    // provider-wide baseline for a local server, and the model's own keys on top of it.
    for params in [
        sources.provider.map(|provider| &provider.sampling_params),
        sources.model.map(|model| &model.sampling_params),
    ]
    .into_iter()
    .flatten()
    .filter(|params| !params.is_empty())
    {
        info.sampling_params.extend(params.iter().map(|(k, v)| (k.clone(), v.clone())));
        from.sampling_params = Layer::Config;
    }
    if let Some(config) = sources.model {
        apply_config(&mut info, &mut from, config);
    }

    // Compat is two layers like sampling is, but merged into a typed value rather than a free-form
    // map, and the provider's half has to be resolved the same way the adapter resolves it — through
    // the catalog when no block was written — or a model would be resolved against flags its adapter
    // is not running.
    let provider_compat = crate::config::resolve_provider_compat(provider, sources.provider);
    if let Some(written) =
        sources.model.map(|model| &model.compat).filter(|written| !written.is_empty())
    {
        if let Some(merged) = merge_compat(&provider_compat, written, provider, id) {
            info.compat = Some(merged);
            from.compat = Layer::Config;
        }
    }
    let compat = info.compat.as_ref().unwrap_or(&provider_compat);

    // Anthropic's adaptive-thinking compat exposes an `ultra` level the catalog can't know about.
    if is_anthropic
        && compat.force_adaptive_thinking
        && !info.thinking_level_map.contains_key("ultra")
        && !info.thinking_level_map.contains_key("max")
    {
        info.thinking_level_map.insert("ultra".into(), Some("max".into()));
    }
    // A thinking map implies reasoning, unless the user said outright otherwise.
    if from.reasoning != Layer::Config && !info.thinking_level_map.is_empty() {
        info.reasoning = true;
    }
    // Clamped once, here, where reasoning and the level map have both settled: a starting level the
    // model cannot reach would otherwise go out as an invented provider value, and a default nobody
    // can honor is config that silently does nothing.
    if let Some(level) = info.default_thinking {
        info.default_thinking = Some(info.effective_thinking_level(level));
    }

    (info, from)
}

/// The model's own compat keys merged over `provider_flags`, or `None` when it says nothing.
///
/// Merged as JSON rather than field by field for the reason the raw map exists: only the keys the
/// user wrote may override, and a typed struct cannot tell those from serde's defaults. Two ways it
/// declines instead of guessing — a key that chooses the transport (an adapter is built once per
/// provider, so honoring it per model would be a lie), and a value of the wrong shape. Both leave
/// the model on its provider's flags and say so, because that is the outcome a reader can predict.
fn merge_compat(
    provider_flags: &CompatFlags,
    written: &serde_json::Map<String, serde_json::Value>,
    provider: &str,
    id: &str,
) -> Option<CompatFlags> {
    let serde_json::Value::Object(mut merged) = serde_json::to_value(provider_flags).ok()? else {
        return None;
    };
    let mut overrides = 0usize;
    for (key, value) in written {
        if PROVIDER_ONLY_COMPAT_KEYS.contains(&key.as_str()) {
            tracing::warn!(
                model = %format!("{provider}/{id}"),
                key = %key,
                "compat key chooses the transport and is provider-level only; ignoring it here"
            );
            continue;
        }
        merged.insert(key.clone(), value.clone());
        overrides += 1;
    }
    if overrides == 0 {
        return None;
    }
    match serde_json::from_value::<CompatFlags>(serde_json::Value::Object(merged)) {
        Ok(flags) => Some(flags),
        Err(error) => {
            tracing::warn!(
                model = %format!("{provider}/{id}"),
                %error,
                "model compat block is not a valid set of flags; using the provider's"
            );
            None
        }
    }
}

/// Apply one models.dev layer, touching only the fields it actually carries.
fn apply_dev(info: &mut ModelInfo, from: &mut Provenance, layer: Layer, dev: &Model) {
    if let Some(name) = &dev.name {
        info.display_name = Some(name.clone());
        from.display_name = layer;
    }
    if let Some(context) = dev.limit.as_ref().and_then(|l| l.context) {
        info.context_window = context;
        from.context_window = layer;
    }
    if let Some(output) = dev.limit.as_ref().and_then(|l| l.output) {
        info.max_tokens = output;
        from.max_tokens = layer;
    }
    if let Some(reasoning) = dev.reasoning {
        info.reasoning = reasoning;
        from.reasoning = layer;
    }
    let controls = thinking_controls(dev);
    if controls != ThinkingControls::default() {
        info.thinking_controls = controls;
        from.thinking_controls = layer;
    }
    let thinking = thinking_map(dev);
    if !thinking.is_empty() {
        info.thinking_level_map = thinking;
        from.thinking_level_map = layer;
    }
    if let Some(input) = modalities(dev) {
        info.input = input;
        from.input = layer;
    }
    if let Some(cost) = &dev.cost {
        info.cost = Cost {
            input: cost.input.unwrap_or_default(),
            output: cost.output.unwrap_or_default(),
            cache_read: cost.cache_read.unwrap_or_default(),
            cache_write: cost.cache_write.unwrap_or_default(),
            cache_write_1h: 0.0,
            tiers: tiers(cost),
        };
        from.cost = layer;
    }
}

/// A models.dev entry's long-prompt tiers, cheapest first.
///
/// A tier measured on anything but context size is skipped rather than guessed at, and so is one
/// without an input rate: billing a long prompt at zero would be worse than at the base rate.
fn tiers(cost: &super::schema::Cost) -> Vec<luxe_core::CostTier> {
    let tier = |above_tokens: u64, rates: &super::schema::TierRates| {
        Some(luxe_core::CostTier {
            above_tokens,
            input: rates.input?,
            output: rates.output.unwrap_or_default(),
            cache_read: rates.cache_read.unwrap_or_default(),
            cache_write: rates.cache_write.unwrap_or_default(),
        })
    };
    let mut tiers: Vec<luxe_core::CostTier> = cost
        .tiers
        .iter()
        .filter_map(|entry| {
            let bound = entry.tier.as_ref()?;
            if bound.kind.as_deref() != Some("context") {
                return None;
            }
            tier(bound.size?, &entry.rates)
        })
        .collect();
    if tiers.is_empty() {
        if let Some(rates) = &cost.context_over_200k {
            tiers.extend(tier(200_000, rates));
        }
    }
    tiers.sort_by_key(|tier| tier.above_tokens);
    tiers
}

/// Apply the user's config — the top layer, so anything it sets is final.
fn apply_config(info: &mut ModelInfo, from: &mut Provenance, config: &ModelConfig) {
    if let Some(name) = &config.name {
        info.display_name = Some(name.clone());
        from.display_name = Layer::Config;
    }
    if let Some(context) = config.context_window {
        info.context_window = context;
        from.context_window = Layer::Config;
    }
    if let Some(max_tokens) = config.max_tokens {
        info.max_tokens = max_tokens;
        from.max_tokens = Layer::Config;
    }
    if let Some(reasoning) = config.reasoning {
        info.reasoning = reasoning;
        from.reasoning = Layer::Config;
    }
    if !config.thinking_level_map.is_empty() {
        // A hand-written map describes what the endpoint supports, so it replaces rather than
        // merges — otherwise a stale catalog level would survive being deliberately removed.
        info.thinking_level_map = config.thinking_level_map.clone();
        from.thinking_level_map = Layer::Config;
    }
    if let Some(values) = &config.input {
        // Replaces for the same reason the thinking map does: the list is a closed claim about
        // what this endpoint accepts, so dropping a modality here must not be undone by a catalog
        // entry that still advertises it. A list luxe recognizes nothing in falls back to text
        // rather than to nothing — no model accepts zero kinds of input, and an empty list would
        // render as a blank row.
        info.input = parse_modalities(values.iter().map(String::as_str))
            .unwrap_or_else(|| vec![Modality::Text]);
        from.input = Layer::Config;
    }
    if let Some(level) = config.thinking {
        info.default_thinking = Some(level);
        from.default_thinking = Layer::Config;
    }
    if let Some(cost) = &config.cost {
        info.cost = Cost {
            input: cost.input,
            output: cost.output,
            cache_read: cost.cache_read,
            cache_write: cost.cache_write,
            cache_write_1h: cost.cache_write_1h,
            tiers: Vec::new(),
        };
        from.cost = Layer::Config;
    }
}

/// Derive luxe thinking levels from models.dev `reasoning_options` effort values.
///
/// A declared effort list is a *closed* set when effort is the model's only dial: the levels it
/// omits are marked unsupported so the picker grays them out and the wire request clamps to a value
/// the endpoint actually accepts, instead of inventing one and earning a 400. Where a token budget
/// is also on offer every level is expressible, so the list only adds what it names.
///
/// `off` is never mapped — thinking is switched off by its own parameter, and an entry here would
/// clamp "off" upward into thinking.
fn thinking_map(dev: &Model) -> HashMap<String, Option<String>> {
    let mut map = HashMap::new();
    for value in dev.reasoning_options.iter().flat_map(|option| &option.values) {
        let entry = match value.as_str() {
            level @ ("minimal" | "low" | "medium" | "high" | "xhigh") => (level, level),
            // models.dev's `max` is luxe's `ultra`.
            "max" | "ultra" => ("ultra", "max"),
            _ => continue,
        };
        map.insert(entry.0.to_string(), Some(entry.1.to_string()));
    }
    if !map.is_empty() && thinking_controls(dev).effort_only() {
        for level in ThinkingLevel::ALL.iter().filter(|level| **level != ThinkingLevel::Off) {
            map.entry(level.as_str().to_string()).or_insert(None);
        }
    }
    map
}

/// Which reasoning dials the entry declares. models.dev types them: `effort` for discrete levels,
/// `budget_tokens` for a token budget; `toggle` says thinking can be switched on and off.
///
/// An off switch is claimed only where the entry says so — a `toggle` option, or `none` among the
/// effort values. Silence has to read as "no", because the endpoints that reject a disable are the
/// ones that answer 400 for it, and an entry with no `reasoning_options` at all is as likely to be
/// one of those as not.
fn thinking_controls(dev: &Model) -> ThinkingControls {
    let mut controls = ThinkingControls::default();
    for option in &dev.reasoning_options {
        match option.kind.as_deref() {
            Some("effort") => controls.effort = true,
            Some("budget_tokens") => controls.budget_tokens = true,
            Some("toggle") => controls.can_disable = true,
            _ => {}
        }
        controls.can_disable |= option.values.iter().any(|value| value == "none");
    }
    controls
}

/// Input modalities luxe understands, or `None` when the entry declares none it recognizes.
fn modalities(dev: &Model) -> Option<Vec<Modality>> {
    let declared = dev.modalities.as_ref().map(|m| m.input.as_slice()).unwrap_or_default();
    parse_modalities(declared.iter().map(String::as_str))
}

/// The modalities in `values` that luxe models, in declared order and deduplicated.
///
/// Anything else is dropped rather than rejected. A catalog that grows a `pdf` or `audio` input,
/// or a hand-written entry with a typo in it, must not take the whole file down with it — the
/// worst outcome for an unknown value is that a capability goes unclaimed.
fn parse_modalities<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<Vec<Modality>> {
    let mut modalities: Vec<Modality> = Vec::new();
    for value in values {
        let modality = match value.to_ascii_lowercase().as_str() {
            "text" => Modality::Text,
            "image" => Modality::Image,
            _ => continue,
        };
        if !modalities.contains(&modality) {
            modalities.push(modality);
        }
    }
    (!modalities.is_empty()).then_some(modalities)
}

#[cfg(test)]
mod tests;
