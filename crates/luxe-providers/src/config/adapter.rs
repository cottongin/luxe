//! Building an adapter — which of the wire adapters a provider's API gets, and how Copilot's three
//! are put together.

use std::sync::Arc;

use luxe_core::auth::ApiKind;
use luxe_core::provider::{ModelInfo, Provider};

use super::{anthropic_provider, openai_compat_provider, openai_responses_provider};
use super::{ApiKey, CompatFlags, ProviderConfig};
use crate::auth::AuthSource;

/// Build one provider adapter.
///
/// The representative `model` only matters for Anthropic's build-time `max_tokens` fallback; every
/// adapter honors the per-request model, so an in-session `SetModel` swap changes the id without a
/// rebuild. Auth is resolved *per request* from `auth`, so a token can refresh and a fresh `/login`
/// takes effect without rebuilding anything either.
///
/// One owner for the routing, Copilot's included, so the TUI, `--rpc` and the loop cannot disagree
/// about which adapter a provider gets.
pub fn build_adapter(
    api: ApiKind,
    model: ModelInfo,
    auth: Arc<dyn AuthSource>,
    base_url: Option<String>,
    compat: CompatFlags,
) -> Arc<dyn Provider + Send + Sync> {
    match api {
        ApiKind::AnthropicMessages => {
            Arc::new(anthropic_provider(ApiKey::default(), base_url, model, compat).with_auth(auth))
        }
        ApiKind::OpenaiResponses | ApiKind::OpenaiCodex => {
            let config = ProviderConfig { base_url, compat, ..Default::default() };
            let adapter = openai_responses_provider(config);
            let adapter = match api {
                ApiKind::OpenaiCodex => adapter.codex(),
                _ => adapter,
            };
            Arc::new(adapter.with_auth(auth))
        }
        // Copilot speaks chat completions, Responses and Anthropic Messages on one host, and picks
        // per model
        ApiKind::OpenaiCompletions if compat.copilot_headers => {
            copilot_adapter(model, auth, base_url, compat)
        }
        ApiKind::OpenaiCompletions => {
            let config = ProviderConfig { base_url, compat: compat.clone(), ..Default::default() };
            Arc::new(openai_compat_provider(config, compat).with_auth(auth))
        }
    }
}

/// GitHub Copilot: one provider over its three wire APIs.
fn copilot_adapter(
    model: ModelInfo,
    auth: Arc<dyn AuthSource>,
    base_url: Option<String>,
    compat: CompatFlags,
) -> Arc<dyn Provider + Send + Sync> {
    use crate::copilot::{messages_auth, CopilotAdapters, CopilotProvider};
    let config = ProviderConfig { base_url, compat: compat.clone(), ..Default::default() };
    let chat = openai_compat_provider(config.clone(), compat.clone()).with_auth(auth.clone());
    let responses = openai_responses_provider(config.clone()).with_auth(auth.clone());
    let messages = anthropic_provider(ApiKey::default(), None, model, compat)
        .with_auth(messages_auth(auth.clone(), config.base_url.clone()));
    let adapters = CopilotAdapters {
        chat: Arc::new(chat),
        responses: Arc::new(responses),
        messages: Arc::new(messages),
    };
    Arc::new(CopilotProvider::new(adapters, auth, config.base_url))
}
