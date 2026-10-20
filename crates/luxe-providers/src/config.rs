//! Configuration — `models.json` / `auth.json` parsing and value resolution.
//!
//! The shape of a config file and the `$ENV` / `${ENV}` / `!command` / literal forms a value may
//! take. *Which* credential a request uses is `crate::auth::source`'s job — it resolves per
//! request so a token can refresh mid-session. Keys live in [`ApiKey`], backed by
//! `secrecy::SecretString`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

use luxe_core::secret_command::SECRET_COMMAND_TIMEOUT;

use secrecy::{ExposeSecret, SecretString};
use std::sync::Arc;
use tracing::warn;

// Compat flags describe how a request must be shaped, which is per-*model* knowledge as often as
// per-provider — so the type lives beside the rest of the metadata that travels with a request. Re-
// exported here because a compat block is written in these files and every caller in the tree knows
// it by this path.
pub use luxe_core::provider::CompatFlags;

/// An API key held in memory as a redacted, zeroize-on-drop secret.
///
/// A stand-in for a plaintext `String` that never reveals the value through `Debug`
/// or `Serialize` (both emit `[REDACTED]`), so a config or provider struct that carries one
/// can't leak the key via a log line or an accidental round-trip serialize. `Deserialize` still
/// reads the real value from config; [`ApiKey::expose`] hands it to the one place that needs it
/// (the outbound auth header). Cheap to clone — the secret is shared behind an `Arc` and zeroized
/// when the last clone drops.
#[derive(Clone)]
pub struct ApiKey(Arc<SecretString>);

impl ApiKey {
    /// Wrap a resolved key value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(Arc::new(SecretString::from(value.into())))
    }

    /// Borrow the raw key for the auth header. Keep the exposed lifetime as short as possible.
    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }

    /// Whether the key is empty (unset / blank).
    pub fn is_empty(&self) -> bool {
        self.0.expose_secret().is_empty()
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey([REDACTED])")
    }
}

impl Default for ApiKey {
    fn default() -> Self {
        Self::new(String::new())
    }
}

impl From<String> for ApiKey {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for ApiKey {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl<'de> serde::Deserialize<'de> for ApiKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(ApiKey::new(String::deserialize(deserializer)?))
    }
}

impl serde::Serialize for ApiKey {
    /// Fail-safe: serialize to a redacted placeholder, never the real key. Nothing in-tree
    /// round-trips these configs, so this can't clobber a live key — it only guarantees a stray
    /// serialize can't leak one.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("[REDACTED]")
    }
}

/// Parsed luxe config — the shared shape of `auth.json` and `models.json`.
///
/// The documented `auth.json` nests keys under `providers.<name>` and can also set
/// `default_provider` / `default_model`; `models.json` adds per-model overrides. Both
/// parse into this one struct so a single loader can merge them.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelsConfig {
    /// Model overrides (provider/model -> config).
    #[serde(default)]
    pub models: HashMap<String, ModelConfig>,
    /// Provider-level defaults (api key, base url, compat).
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    /// Provider used when none is given on the CLI/env.
    #[serde(default)]
    pub default_provider: Option<String>,
    /// Model used when none is given on the CLI/env.
    #[serde(default)]
    pub default_model: Option<String>,
}

/// Per-model configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelConfig {
    /// Human-friendly display name for the TUI (`/model` picker + statusline). Cosmetic only —
    /// the wire model id stays the `provider/id` config key, so the provider gets the exact
    /// identifier it expects.
    pub name: Option<String>,
    /// API key (resolved via auth resolution).
    pub api_key: Option<ApiKey>,
    /// Base URL override.
    pub base_url: Option<String>,
    /// Context window override.
    pub context_window: Option<u32>,
    /// Max tokens override.
    pub max_tokens: Option<u32>,
    /// Whether this model supports reasoning/thinking.
    pub reasoning: Option<bool>,
    /// Input kinds this endpoint accepts (`["text", "image"]`), overriding the catalog.
    ///
    /// The one piece of model metadata a config-resolved endpoint could not state. Every local or
    /// self-hosted model — LM Studio, Ollama, vLLM, a proxy — resolves purely from this file, so
    /// without it a vision model loaded with its projector reports `text`. Kept as strings rather
    /// than a typed enum so an unrecognized value costs that one capability instead of failing the
    /// whole file (see `catalog::resolve::parse_modalities`).
    #[serde(rename = "modalities", default)]
    pub input: Option<Vec<String>>,
    /// Price per million tokens, for endpoints models.dev doesn't cover (a proxy, a private
    /// deployment) or whose published rates don't match a negotiated one.
    pub cost: Option<CostConfig>,
    /// Thinking level map (level -> provider value; `null` = unsupported).
    #[serde(default)]
    pub thinking_level_map: HashMap<String, Option<String>>,
    /// Reasoning level a session starts at on this model, overriding whatever the last session
    /// chose.
    ///
    /// The level is otherwise remembered globally, which is wrong the moment two models disagree
    /// about what a level costs: a local 27B at `xhigh` spends minutes restating the question for
    /// no measurable gain, and it inherited that level from the frontier model you were on an hour
    /// ago. A bump made in-session still holds until the model changes — this is where you start,
    /// not a ceiling. Say which levels the endpoint *cannot* do with `thinking_level_map` instead.
    pub thinking: Option<luxe_core::provider::ThinkingLevel>,
    /// Sampling parameters merged verbatim into the request body, after the fields luxe sets
    /// itself — so these keys win. Lets a model carry the samplers luxe doesn't model (llama.cpp
    /// `min_p`/`repeat_penalty`, vLLM `top_k`), which is how a published sampling recipe is
    /// expressed at all. Merges per key over the provider's map. OpenAI-shaped APIs only.
    #[serde(default)]
    pub sampling_params: serde_json::Map<String, serde_json::Value>,
    /// Request-shaping quirks this model needs where they differ from its provider's, **exactly as
    /// written** — which is the point of the raw map.
    ///
    /// A typed [`CompatFlags`] here could not tell a flag the user set from one serde defaulted, so
    /// a model entry naming `thinking_format` would also assert `supports_usage_in_streaming: true`
    /// and `cache_control_format: null` — silently switching off streamed usage, or Anthropic prompt
    /// caching, for that model alone. Keys are merged over the provider's flags at resolution time
    /// (`catalog::resolve`), so only what is written here overrides anything.
    #[serde(default)]
    pub compat: serde_json::Map<String, serde_json::Value>,
}

/// Per-model price overrides, in USD per million tokens.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
pub struct CostConfig {
    /// Input cost per million tokens
    #[serde(default)]
    pub input: f64,
    /// Output cost per million tokens
    #[serde(default)]
    pub output: f64,
    /// Cache-hit read cost per million tokens
    #[serde(default)]
    pub cache_read: f64,
    /// Cache-write cost per million tokens
    #[serde(default)]
    pub cache_write: f64,
    /// Cost per million tokens of a cache write kept for an hour; unset prices it at twice input
    #[serde(default, skip_serializing_if = "is_unset_rate")]
    pub cache_write_1h: f64,
}

fn is_unset_rate(rate: &f64) -> bool {
    *rate == 0.0
}

/// Per-provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderConfig {
    /// Default API key.
    pub api_key: Option<ApiKey>,
    /// Default base URL.
    pub base_url: Option<String>,
    /// models.dev provider id to inherit model metadata from, when this provider's name isn't
    /// one (e.g. a `dashscope` entry serving Alibaba's catalog). Affects metadata lookup only.
    pub models_dev: Option<String>,
    /// Which wire API this provider speaks: `openai-completions`, `openai-responses`,
    /// `anthropic-messages`, or `openai-codex`.
    ///
    /// The built-in provider catalog normally answers this. Set it for a proxy or private deployment
    /// luxe doesn't recognize, or to pin a known provider to a different endpoint than its catalog
    /// default.
    pub api: Option<String>,
    /// How this provider's API key goes on the wire: `bearer`, `x-api-key`, `api-key`, or
    /// `foundry` (both key headers).
    ///
    /// Named for the header rather than the vendor, because that is what a 401 shows you. The
    /// catalog answers it for every provider luxe knows; set it for a gateway that wants a
    /// different one, or to send a Microsoft Entra ID token as `bearer` against a Foundry endpoint
    /// that would otherwise get a key header.
    pub auth: Option<String>,
    /// What this endpoint answered `/models` with, last time anything asked.
    ///
    /// The models.dev catalog is a poor description of a *local* server: it lists the models
    /// people commonly run under that name, not the ones this machine has, so a picker built from
    /// it offers ids the endpoint will refuse. Recorded here because the endpoint is the only
    /// authority, and because it has to survive a launch with the server switched off.
    ///
    /// An Azure resource is the same story told by a cloud: the catalog lists what Azure offers,
    /// not what anybody deployed, so luxe records the resource's deployments here too.
    pub served_models: Option<Vec<String>>,
    /// Deployment name → the model it runs, for a deployment named something other than its
    /// model (`"sonnet-prod": "claude-sonnet-5"`).
    ///
    /// The name is what goes on the wire; the model is what context window, output cap and price
    /// are looked up under. Written by deployment discovery, and yours to write when it is off.
    #[serde(default)]
    pub deployments: std::collections::BTreeMap<String, String>,
    /// Whether to ask an Azure resource which models it has deployed. On unless `false`.
    ///
    /// Off leaves `served_models` and `deployments` exactly as written, for a resource whose key
    /// cannot list deployments or a list you want to curate by hand.
    pub discover_deployments: Option<bool>,
    /// Sampling parameters applied to every model this provider serves, unless the model's own
    /// map overrides the key. A local server hosting several builds usually wants one recipe.
    #[serde(default)]
    pub sampling_params: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub compat: CompatFlags,
}

/// Which wire API a provider speaks: explicit config, then the built-in catalog, then the legacy
/// `use_responses_api` compat flag, then chat/completions.
///
/// One owner, because three callers need the same answer for different reasons: the CLI builds an
/// adapter with it, model resolution needs it to know whether Anthropic's required `max_tokens`
/// applies, and auth needs it to pick a header. Deriving it twice is how a provider ends up
/// resolved against an API it is not speaking.
pub fn resolve_api(
    provider_name: &str,
    config: Option<&ProviderConfig>,
) -> luxe_core::auth::ApiKind {
    use luxe_core::auth::ApiKind;

    if let Some(api) = config.and_then(|config| config.api.as_deref()).and_then(ApiKind::parse) {
        return api;
    }
    if let Some(descriptor) = crate::auth::provider_descriptor(provider_name) {
        return descriptor.api;
    }
    match config.is_some_and(|config| config.compat.use_responses_api) {
        true => ApiKind::OpenaiResponses,
        false => ApiKind::OpenaiCompletions,
    }
}

/// How a provider's API key goes on the wire: its own block when it names one, else the catalog's,
/// else whatever its API implies.
pub fn resolve_key_style(
    provider_name: &str,
    config: Option<&ProviderConfig>,
) -> crate::auth::KeyStyle {
    if let Some(style) =
        config.and_then(|config| config.auth.as_deref()).and_then(crate::auth::KeyStyle::parse)
    {
        return style;
    }
    match crate::auth::provider_descriptor(provider_name) {
        Some(descriptor) => descriptor.key_style(),
        None => crate::auth::KeyStyle::for_api(resolve_api(provider_name, config)),
    }
}

/// Resolve a value that may contain `$ENV`, `${ENV}`, `$env:ENV` or `!command` references; an
/// unset `${ENV}` expands to nothing.
pub fn resolve_value(value: &str) -> String {
    // Check for `!command` on the original value (not `$!` which is an escape).
    if let Some(cmd) = value.strip_prefix('!') {
        return run_command(cmd.trim());
    }

    // Process escape sequences: `$$` → `$`, `$!` → `!`.
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '$' && chars.peek() == Some(&'$') {
            result.push('$');
            chars.next(); // consume the second `$`
        } else if ch == '$' && chars.peek() == Some(&'!') {
            result.push('!');
            chars.next(); // consume the `!`
        } else {
            result.push(ch);
        }
    }

    // The same expander `mcp.json` uses, so `${VAR}` means one thing everywhere; a bare `$NAME` is
    // this file's own, and stays as written when unset since a key may hold a `$` of its own
    luxe_core::mcp_client::expand_env_with_bare(&result)
}

/// Run a key command bounded by the default deadline, for the sync callers; a failure is logged
/// with stderr only and resolves to nothing.
fn run_command(cmd: &str) -> String {
    luxe_core::secret_command::run_blocking(cmd, KEY_COMMAND, SECRET_COMMAND_TIMEOUT)
        .unwrap_or_else(|error| {
            warn!(%error, "key command failed");
            String::new()
        })
}

/// What a command in a config value is, in its errors: usually a key, sometimes a base URL.
const KEY_COMMAND: &str = "a configured value";

/// [`resolve_value_async`] at the default deadline, for a login checking the key it was handed.
pub async fn resolve_key(value: &str) -> Result<String, String> {
    resolve_value_async(value, SECRET_COMMAND_TIMEOUT).await
}

/// [`resolve_value`] for a request: a `!command` runs bounded by `deadline` and off the runtime
/// thread, and a failure says why instead of resolving to nothing.
pub async fn resolve_value_async(value: &str, deadline: Duration) -> Result<String, String> {
    match value.strip_prefix('!') {
        Some(command) => {
            luxe_core::secret_command::run(command.trim(), KEY_COMMAND, deadline).await
        }
        None => Ok(resolve_value(value)),
    }
}

/// Load and parse `models.json` from a path.
pub fn load_models_config(path: &Path) -> Result<ModelsConfig, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    Ok(models_config_from(serde_json::from_str(&content)?)?)
}

/// Parse an already-read `models.json`, for a caller that wants the raw document too.
pub(crate) fn models_config_from(
    mut raw: serde_json::Value,
) -> Result<ModelsConfig, serde_json::Error> {
    layer_catalog_compat(&mut raw);
    serde_json::from_value(raw)
}

/// Fill each provider block's `compat` with the defaults for every flag the user did not write, so
/// only what they actually wrote overrides.
///
/// A provider block is normally about a base URL or a key, and the flags come along by accident:
/// `{"providers":{"anthropic":{"base_url":"…"}}}` must not deserialize `compat` from serde defaults
/// and replace the catalog's wholesale, which would silently switch off Anthropic prompt caching
/// (and every other catalog quirk) for anyone who configured a provider by hand.
///
/// The catalog answers for a provider it knows. For one it does not, the declared wire API answers
/// instead: a private Anthropic Messages deployment caches by `cache_control` however it is named,
/// and a proxy left with caching off would re-read its whole transcript at full price every turn.
fn layer_catalog_compat(raw: &mut serde_json::Value) {
    let Some(providers) = raw.get_mut("providers").and_then(|value| value.as_object_mut()) else {
        return;
    };
    for (name, block) in providers.iter_mut() {
        let Some(block) = block.as_object_mut() else { continue };
        let declared_api = block.get("api").and_then(|value| value.as_str()).unwrap_or_default();
        let Some(defaults) = crate::auth::provider_descriptor(name)
            .map(|descriptor| descriptor.compat())
            .or_else(|| crate::auth::wire_api_compat_floor(declared_api))
        else {
            continue;
        };
        let Ok(serde_json::Value::Object(defaults)) = serde_json::to_value(defaults) else {
            continue;
        };
        let written =
            block.get("compat").and_then(|value| value.as_object()).cloned().unwrap_or_default();
        let mut merged = defaults;
        merged.extend(written);
        block.insert("compat".to_string(), serde_json::Value::Object(merged));
    }
}

/// The compat flags a provider runs with: its own block when the user wrote one (already layered
/// over the catalog's defaults at load), else the catalog's, else conservative ones.
///
/// One owner, because two callers need the same answer for different reasons: the adapter is built
/// with these, and a model's own keys are merged over them into its resolved [`CompatFlags`].
/// Computing it twice is how a model ends up resolved against flags its adapter is not running.
pub fn resolve_provider_compat(
    provider_name: &str,
    config: Option<&ProviderConfig>,
) -> CompatFlags {
    match config {
        Some(config) => config.compat.clone(),
        None => crate::auth::provider_descriptor(provider_name)
            .map(|descriptor| descriptor.compat())
            .unwrap_or_default(),
    }
}

/// Create an OpenAI-compatible provider from configuration.
pub fn openai_compat_provider(
    config: ProviderConfig,
    compat: CompatFlags,
) -> crate::openai_compat::OpenaiCompatProvider {
    crate::openai_compat::OpenaiCompatProvider::new(config, compat)
}

/// Create an OpenAI Responses API provider from configuration.
pub fn openai_responses_provider(
    config: ProviderConfig,
) -> crate::openai_responses::OpenaiResponsesProvider {
    crate::openai_responses::OpenaiResponsesProvider::new(config)
}

/// Map the generic [`CompatFlags`] onto Anthropic's adapter flags.
///
/// The conversion itself lives on `AnthropicCompat`, which the adapter runs per request since a
/// model may carry its own flags.
pub fn anthropic_compat(flags: &CompatFlags) -> crate::anthropic::AnthropicCompat {
    crate::anthropic::AnthropicCompat::from(flags)
}

/// Create an Anthropic provider from configuration + resolved model info.
pub fn anthropic_provider(
    api_key: ApiKey,
    base_url: Option<String>,
    model_info: luxe_core::provider::ModelInfo,
    compat: CompatFlags,
) -> crate::anthropic::AnthropicProvider {
    crate::anthropic::AnthropicProvider::new(
        api_key,
        base_url,
        model_info,
        anthropic_compat(&compat),
    )
}

mod adapter;

pub use adapter::build_adapter;

#[cfg(test)]
mod tests;
