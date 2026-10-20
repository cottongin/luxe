//! Provider adapter construction — the active adapter plus a registry of every provider luxe can
//! route to, so an in-session model switch or a mid-session `/login` reaches the right backend.

use std::collections::HashMap;
use std::sync::Arc;

use luxe_core::auth::ApiKind;
use luxe_core::provider::{ModelInfo, Provider};
use luxe_providers::auth::{provider_descriptor, AuthSource, AuthStore, StoreAuth};
use luxe_providers::config::{
    resolve_api, resolve_key_style, resolve_value, CompatFlags, ProviderConfig,
};

use super::config::{load_luxe_config, provider_key_env};
use super::AgentSettings;

/// The compat flags a provider runs with.
///
/// One line, because the answer belongs where a model's own keys are merged over it — the adapter
/// and the model metadata have to agree about what the provider's flags are.
pub(crate) fn resolve_compat(provider_name: &str, config: Option<&ProviderConfig>) -> CompatFlags {
    luxe_providers::config::resolve_provider_compat(provider_name, config)
}

/// The base URL a provider runs against: the user's config, else the catalog's default, else the
/// adapter's own.
pub(crate) fn resolve_base_url(
    provider_name: &str,
    config: Option<&ProviderConfig>,
) -> Option<String> {
    config
        .and_then(|config| config.base_url.clone())
        .map(|url| resolve_value(&url))
        .filter(|url| !url.trim().is_empty())
        .or_else(|| {
            provider_descriptor(provider_name)
                .and_then(|descriptor| descriptor.base_url)
                .map(str::to_string)
        })
}

/// Build one provider adapter.
///
/// The representative `model` only matters for Anthropic's build-time `max_tokens` fallback; every
/// adapter honors the per-request model, so an in-session `SetModel` swap changes the id without a
/// rebuild. Auth is resolved *per request* from `auth`, so a token can refresh and a fresh `/login`
/// takes effect without rebuilding anything either.
/// `LUXE_WIRE_LOG` wraps every adapter here, so the recorder sees the default provider, the
/// per-provider registry, and any mid-session switch — one seam instead of one per call site.
pub(crate) fn build_provider_adapter(
    api: ApiKind,
    model: ModelInfo,
    auth: Arc<dyn AuthSource>,
    base_url: Option<String>,
    compat: CompatFlags,
) -> Arc<dyn Provider + Send + Sync> {
    luxe_providers::wire::RecordingProvider::wrap(luxe_providers::config::build_adapter(
        api, model, auth, base_url, compat,
    ))
}

/// The active provider's adapter, with the key named for this run, if any, over the store.
pub(crate) fn build_active_adapter(settings: &AgentSettings) -> Arc<dyn Provider + Send + Sync> {
    let config = load_luxe_config();
    let provider_config = config.providers.get(&settings.provider);
    let api = resolve_api(&settings.provider, provider_config);
    build_provider_adapter(
        api,
        settings.launch_model(),
        active_auth(settings, provider_config),
        settings.base_url.clone().or_else(|| resolve_base_url(&settings.provider, provider_config)),
        settings.compat.clone(),
    )
}

/// The active provider's auth source: the key named for this run, if any, over the same chain
/// every other provider uses.
pub(super) fn active_auth(
    settings: &AgentSettings,
    provider_config: Option<&ProviderConfig>,
) -> Arc<dyn AuthSource> {
    provider_auth(AuthStore::default_path(), &settings.provider, provider_config)
        .with_override(settings.api_key.clone())
        .into_arc()
}

/// How a provider finds its key when none was named for this run: what you signed in with, then
/// the provider's own variable, then the key its block names.
///
/// One chain for the provider a session starts on and every one it can switch to, so the key that
/// goes out never depends on how the session got there. A provider luxe does not know has no
/// variable of its own, only the `OPENAI_API_KEY` fallback, so its configured key comes first: an
/// OpenAI key must not go to someone else's endpoint in place of the key written for it. Keys stay
/// as written; the source expands them per request.
fn provider_auth(store: AuthStore, name: &str, config: Option<&ProviderConfig>) -> StoreAuth {
    let configured = config
        .and_then(|config| config.api_key.clone())
        .filter(|key| !key.expose().trim().is_empty());
    let auth = StoreAuth::new(store, name, resolve_api(name, config))
        .with_key_style(Some(resolve_key_style(name, config)));
    match (provider_descriptor(name), configured) {
        (None, None) => {
            auth.with_env(provider_key_env(name).iter().map(|v| v.to_string()).collect())
        }
        (_, configured) => auth.with_fallback(configured),
    }
}

/// Assemble the provider registry the engine routes through on an in-session model switch.
///
/// Every provider luxe knows about is registered: the built-in catalog plus anything declared in
/// `~/.config/luxe/{auth,models}.json`. That is what makes `/login` take effect mid-session — the
/// adapter is already there, and its store-backed auth source finds the new credential on the next
/// turn. A provider with no credential registers too, and answers with an actionable auth error
/// rather than a 401.
pub(crate) fn build_provider_registry(
    settings: &AgentSettings,
    active: &Arc<dyn Provider + Send + Sync>,
) -> HashMap<String, Arc<dyn Provider + Send + Sync>> {
    let mut registry: HashMap<String, Arc<dyn Provider + Send + Sync>> = HashMap::new();
    registry.insert(settings.provider.clone(), active.clone());

    let config = load_luxe_config();
    let store = AuthStore::default_path();
    let catalog_ids = luxe_providers::auth::provider_descriptors()
        .iter()
        .filter(|descriptor| descriptor.id != "custom")
        .map(|descriptor| descriptor.id.to_string());
    let configured_ids = config.providers.keys().cloned();

    for name in catalog_ids.chain(configured_ids) {
        if registry.contains_key(&name) {
            continue;
        }
        let provider_config = config.providers.get(&name);
        let api = resolve_api(&name, provider_config);
        // No override here: the CLI's `--api-key` was meant for the active provider, not for every
        // provider the registry can reach.
        let auth = provider_auth(store.clone(), &name, provider_config);
        // A placeholder model: every adapter honors the per-request model, so this only supplies
        // Anthropic's build-time `max_tokens` fallback.
        let (model, _) = crate::model_catalog::resolve_model(&config, &name, "");
        let adapter = build_provider_adapter(
            api,
            model,
            auth.into_arc(),
            resolve_base_url(&name, provider_config),
            resolve_compat(&name, provider_config),
        );
        registry.insert(name, adapter);
    }
    registry
}

/// A `--model` that names its provider, `anthropic/claude-opus-5-5`, split into the two.
///
/// Only when the prefix is a provider luxe knows, built in or configured, and no other provider
/// was named: under `--provider openrouter`, `anthropic/claude-x` is OpenRouter's own id, slash and
/// all. Without the split the whole string went to the default provider as an id, which is the
/// form the "no model selected" error tells you to type.
pub(super) fn split_model_ref(
    model: &str,
    named_provider: Option<&str>,
    config: &luxe_providers::config::ModelsConfig,
) -> (Option<String>, String) {
    let Some((prefix, id)) = model.split_once('/') else { return (None, model.to_string()) };
    let known = luxe_providers::auth::provider_descriptor(prefix).is_some()
        || config.providers.contains_key(prefix);
    let canonical = luxe_providers::auth::canonical_id;
    let agrees = named_provider.is_none_or(|named| canonical(named) == canonical(prefix));
    if known && agrees && !id.is_empty() {
        let provider = named_provider.map_or_else(|| prefix.to_string(), str::to_string);
        (Some(provider), id.to_string())
    } else {
        (None, model.to_string())
    }
}

/// Whether `provider` can serve `model`, as far as anything knows before a request is made.
///
/// Deliberately permissive: it only rejects a pairing the provider catalog *names* as unserved (a
/// subscription backend with a fixed model list), or a model configured for a different provider
/// entirely. An unknown provider or an unlisted model is allowed through — the endpoint is the real
/// authority, and refusing a valid model would be worse than passing an invalid one to it.
pub(super) fn provider_serves(provider: &str, model: &str, configured_for: Option<&str>) -> bool {
    if let Some(configured_for) = configured_for {
        if !luxe_providers::auth::canonical_id(configured_for)
            .eq(&luxe_providers::auth::canonical_id(provider))
        {
            return false;
        }
    }
    luxe_providers::auth::provider_descriptor(provider)
        .is_none_or(|descriptor| descriptor.served_models.serves(model))
}

#[cfg(test)]
mod tests;
