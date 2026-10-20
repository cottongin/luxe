//! Model catalog wiring — cache location, refresh scheduling, and the `/model` menu.
//!
//! Metadata itself is resolved by [`luxe_providers::catalog`]; this module owns the parts that
//! depend on where Luxe keeps files and when it's allowed to touch the network. Resolution is
//! always offline-safe — it reads the embedded snapshot plus a local cache and never blocks —
//! while [`spawn_refresh`] keeps that cache current in the background.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use luxe_core::provider::ModelInfo;
use luxe_providers::catalog::{self, Catalog, Provenance, Sources};
use luxe_providers::config::ModelsConfig;
use luxe_tui::model_picker::{ModelEntry, ModelMenu};

use crate::engine_setup::{
    env_nonempty, load_luxe_config, models_dev_cache_path, provider_key_env, AgentSettings,
};

/// How long a cached catalog is considered current.
///
/// Short, because revalidation is an `If-None-Match` that usually costs a `304` and zero bytes.
const DEFAULT_TTL: Duration = Duration::from_secs(6 * 3600);

/// Resolve a model's metadata through every layer: embedded snapshot, on-disk cache, user config.
///
/// The single entry point for "what do we know about `provider/id`" — startup, `--model`,
/// headless, the loop, the picker, and `luxe models` all come through here so they can't drift.
pub(crate) fn resolve_model(
    config: &ModelsConfig,
    provider: &str,
    id: &str,
) -> (ModelInfo, Provenance) {
    let cache = cached_catalog();
    catalog::resolve(
        provider,
        id,
        &Sources {
            snapshot: Some(catalog::snapshot()),
            cache: Some(&cache),
            model: config.models.get(&format!("{provider}/{id}")),
            provider: config.providers.get(provider),
        },
    )
}

/// Build the model menu (browsable catalog + active model) for the TUI.
pub(crate) fn build_menu(settings: &AgentSettings) -> ModelMenu {
    let config = load_luxe_config();
    let cache = cached_catalog();
    let snapshot = catalog::snapshot();
    let active = settings.model.clone();

    let mut entries: Vec<ModelEntry> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let push = |entries: &mut Vec<ModelEntry>, info: ModelInfo, configured: bool| {
        entries.push(ModelEntry {
            label: info.display().to_string(),
            info,
            configured,
            unverified: false,
        });
    };

    // Configured models are the source of truth; the catalog only fills the gaps they leave.
    for key in config.models.keys() {
        let Some((provider, id)) = key.split_once('/') else { continue };
        seen.insert(key.clone());
        push(&mut entries, resolve_model(&config, provider, id).0, true);
    }

    // Then everything the catalog knows for providers Luxe can actually route to: the ones with a
    // config block, anything the user has signed in to (a Copilot login is worthless if its models
    // never appear), and the launch's own provider when the launch itself carries the credentials
    // to reach it.
    //
    // Not the launch's provider unconditionally, which is where this went wrong: with nothing
    // configured at all it resolves to the fallback name, and the fallback aliases to OpenAI — so
    // a machine that had never been set up opened the picker on thirty-eight models it could not
    // authenticate, and choosing one looked like progress right up to the first turn.
    let signed_in = signed_in_providers();
    // `base_url` is no evidence: every catalog provider resolves to its published endpoint whether
    // or not this machine can authenticate against it.
    let launch_is_reachable = settings.api_key.is_some()
        || provider_key_env(&settings.provider).iter().any(|name| env_nonempty(name).is_some())
        || is_keyless(&settings.provider);
    let routable: HashSet<String> = config
        .providers
        .keys()
        .cloned()
        .chain(launch_is_reachable.then(|| settings.provider.clone()))
        .chain(configured_providers())
        .chain(signed_in.keys().cloned())
        .collect();
    for provider in routable {
        let block = config.providers.get(&provider);
        // An endpoint that told us what it serves has the only answer worth having. models.dev
        // lists the models people commonly run under a name like `lmstudio`, which is a different
        // machine's library: built from it, the picker offered three ids this server had never
        // heard of and the login's own choice was not among them.
        if let Some(served) = block.and_then(|block| block.served_models.as_ref()) {
            for id in served {
                if !seen.insert(format!("{provider}/{id}")) {
                    continue;
                }
                push(&mut entries, resolve_model(&config, &provider, id).0, true);
            }
            continue;
        }
        let dev_provider = catalog::alias::models_dev_provider(&provider, block);
        let allowed = signed_in.get(&provider).cloned().unwrap_or_default();
        // An Azure resource could have said what it serves and has not yet: what follows is Azure's
        // offer, and the rows say so rather than passing for the resource's deployments.
        let unverified = luxe_providers::auth::provider_descriptor(&provider)
            .is_some_and(|descriptor| descriptor.lists_deployments);
        let known = snapshot
            .provider_models(dev_provider)
            .chain(cache.provider_models(dev_provider))
            .filter(|(_, model)| model.tool_call != Some(false));
        for (id, _) in known {
            if !offers_model(&provider, id, &allowed) {
                continue;
            }
            if !seen.insert(format!("{provider}/{id}")) {
                continue;
            }
            push(&mut entries, resolve_model(&config, &provider, id).0, false);
            if let Some(entry) = entries.last_mut() {
                entry.unverified = unverified;
            }
        }
    }

    // Always include the active model so `/model` can show and return to it. Nothing to add when
    // no model is chosen — the menu is then purely what this machine can reach, which is the list
    // somebody arriving at it for the first time is trying to read.
    if let Some(active) = active.as_ref() {
        if seen.insert(format!("{}/{}", active.r#ref.provider, active.r#ref.id)) {
            push(&mut entries, active.clone(), true);
        }
    }

    // Stable, scannable order: configured first, then by provider, then by label.
    entries.sort_by(|a, b| {
        b.configured
            .cmp(&a.configured)
            .then_with(|| a.info.r#ref.provider.cmp(&b.info.r#ref.provider))
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
    });

    ModelMenu { catalog: entries, active }
}

/// The models a client of the protocol can switch to: the `/model` menu in its order, leaving out
/// rows only guessed for an endpoint that hasn't said what it serves.
pub(crate) fn choices(settings: &AgentSettings) -> Vec<luxe_core::event::ModelChoice> {
    offered(&build_menu(settings).catalog)
}

fn offered(menu: &[ModelEntry]) -> Vec<luxe_core::event::ModelChoice> {
    menu.iter().filter(|entry| !entry.unverified).map(|entry| choice(&entry.info)).collect()
}

/// `info` as a client offers it, with the thinking levels it honors as they are, not clamped.
fn choice(info: &ModelInfo) -> luxe_core::event::ModelChoice {
    let levels = luxe_core::provider::ThinkingLevel::ALL.iter().copied();
    let honored = levels.filter(|level| info.effective_thinking_level(*level) == *level);
    luxe_core::event::ModelChoice {
        provider: info.r#ref.provider.clone(),
        model: info.r#ref.id.clone(),
        name: info.display().to_string(),
        thinking_levels: honored.collect(),
    }
}

/// Whether a provider needs no credential at all — a server on this machine.
///
/// Naming one on the command line is the whole statement of intent: there is no key to check, so
/// `luxe --provider lmstudio` has said everything it can say about being able to reach it.
fn is_keyless(provider: &str) -> bool {
    luxe_providers::auth::provider_descriptor(provider).is_some_and(|descriptor| descriptor.keyless)
}

/// Every provider that could authenticate a turn right now.
///
/// Asked of the auth service rather than re-derived, because it is the same question the first-run
/// path and the dashboard ask, and it has more answers than a credential file: an ambient
/// environment variable, a key written into a provider block, a local server that was connected.
/// A second copy of that rule is how the picker starts disagreeing with the screen that sent you
/// to it.
fn configured_providers() -> Vec<String> {
    use luxe_core::auth::AuthService;
    luxe_providers::auth::ProviderAuth::new()
        .providers()
        .into_iter()
        .filter(|option| option.status.is_configured())
        .map(|option| option.id)
        .collect()
}

/// Providers with a stored credential, mapped to the model ids that credential is good for.
///
/// An empty list means "no restriction the credential knows about" — most providers don't enumerate
/// what a key can reach, and GitHub Copilot does.
fn signed_in_providers() -> std::collections::HashMap<String, Vec<String>> {
    let store = luxe_providers::auth::AuthStore::default_path();
    store
        .credentials()
        .into_iter()
        .map(|(provider, credential)| {
            let available = credential
                .as_oauth()
                .map(|oauth| oauth.available_models.clone())
                .unwrap_or_default();
            (provider, available)
        })
        .collect()
}

/// Whether a provider should offer `model_id` in the picker.
///
/// Two things models.dev can't express. GitHub Copilot's *plan* decides which models an account may
/// use, and the login recorded that list. `openai-codex` borrows OpenAI's catalog for metadata but
/// is a different product from the API: the ChatGPT-account backend refuses `gpt-5.3-codex`
/// ("not supported when using Codex with a ChatGPT account") while serving `gpt-5.6-sol` and
/// friends, so the catalog names exactly what it serves rather than guessing from the id.
fn offers_model(provider: &str, model_id: &str, credential_models: &[String]) -> bool {
    // What the account itself reported always wins — it is the most specific truth available.
    if !credential_models.is_empty() {
        return credential_models.iter().any(|allowed| allowed == model_id);
    }
    luxe_providers::auth::provider_descriptor(provider)
        .is_none_or(|descriptor| descriptor.served_models.serves(model_id))
}

/// Best-effort background refresh of the models.dev cache.
///
/// Skipped when the cache is current or `LUXE_MODELS_DEV_REFRESH=0`; forced by `=1`. Failures
/// are logged and ignored — resolution always works from the snapshot regardless.
pub(crate) fn spawn_refresh() {
    let setting = std::env::var("LUXE_MODELS_DEV_REFRESH").ok();
    let normalized = setting.as_deref().map(|v| v.trim().to_ascii_lowercase());
    if matches!(normalized.as_deref(), Some("0" | "false" | "no" | "off")) {
        return;
    }
    let forced = matches!(normalized.as_deref(), Some("1" | "true" | "yes" | "on"));
    let path = models_dev_cache_path();
    if !forced && !cache_is_stale(&path) {
        return;
    }
    tokio::spawn(async move {
        if let Err(error) = refresh(&path).await {
            // `warn`, not `info`: a silent refresh failure is how a stale catalog goes unnoticed.
            tracing::warn!(%error, "models.dev refresh failed; using the cached/embedded catalog");
        }
    });
}

/// Revalidate the cache against models.dev, returning whether the catalog changed.
pub(crate) async fn refresh(path: &std::path::Path) -> Result<bool, String> {
    let meta = catalog::store::load_meta(path);
    let url =
        std::env::var("LUXE_MODELS_DEV_URL").unwrap_or_else(|_| catalog::MODELS_DEV_API_URL.into());
    match catalog::fetch(&url, meta.etag.as_deref()).await.map_err(|error| error.to_string())? {
        catalog::Fetched::Unchanged => {
            catalog::store::touch(path, None).map_err(|error| error.to_string())?;
            tracing::debug!("models.dev catalog unchanged");
            Ok(false)
        }
        catalog::Fetched::Changed { body, etag } => {
            // Parse before persisting so a bad response can't replace a good cache.
            let parsed = catalog::parse(&body).map_err(|error| error.to_string())?;
            catalog::store::store(path, &body, etag).map_err(|error| error.to_string())?;
            invalidate_cached_catalog();
            tracing::info!(models = parsed.len(), "refreshed models.dev catalog");
            Ok(true)
        }
    }
}

/// Path to the on-disk models.dev catalog cache, honoring `LUXE_MODELS_DEV_PATH`.
pub(crate) fn cache_path() -> PathBuf {
    models_dev_cache_path()
}

/// Whether the cache is older than the TTL (or absent) and should be revalidated.
pub(crate) fn cache_is_stale(path: &std::path::Path) -> bool {
    catalog::store::load_meta(path).is_stale(ttl())
}

/// How long a cached catalog stays current (`LUXE_MODELS_DEV_TTL`, in seconds).
fn ttl() -> Duration {
    std::env::var("LUXE_MODELS_DEV_TTL")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_TTL)
}

/// Parsed copy of the on-disk catalog, shared process-wide.
static CACHED: Mutex<Option<Arc<Catalog>>> = Mutex::new(None);

/// The on-disk catalog, parsed once per process (empty when there's no usable cache).
pub(crate) fn cached_catalog() -> Arc<Catalog> {
    let mut slot = CACHED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    slot.get_or_insert_with(|| Arc::new(catalog::store::load(&cache_path()).unwrap_or_default()))
        .clone()
}

/// Drop the parsed copy so the next resolution picks up a refreshed file.
pub(crate) fn invalidate_cached_catalog() {
    *CACHED.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

#[cfg(test)]
mod tests;
