//! Model lists — verifying a credential reaches a provider, and reading what it serves.
//!
//! The pure half of a login: nothing here writes anything. [`verify_credential`] is the one network
//! call, and the rest turn its answer into the confirmation a user reads.

use luxe_core::auth::{ApiKind, AuthError, AuthEvent, AuthInteraction};

use super::ProviderDescriptor;
use crate::auth::source::RequestAuth;

/// How many models to name when confirming a login.
const MODEL_PREVIEW: usize = 3;

/// Check that a credential reaches the provider, and learn what it can serve.
///
/// One cheap authenticated request — the model list — which doubles as the source of the "17 models
/// available" confirmation and of a sensible default model. Codex has no such endpoint, and its login
/// already proved the account, so it is exempt; so is a provider whose `lists_models` says the same
/// (Foundry's Anthropic passthrough), which verifies through the sibling sharing its resource
/// rather than reading a 404 as a broken key.
///
/// **What this does and doesn't prove.** A 401/403 is a real rejection and fails the login, which is
/// what catches a mistyped or revoked key on the providers that authenticate this endpoint (OpenAI,
/// Anthropic, most others). A few providers serve `/models` publicly — NVIDIA NIM does — so a wrong
/// key there passes this check and fails on the first turn instead, with the auth error the adapter
/// now raises. The alternative is spending tokens on a real completion at every login, which is a
/// worse trade for a check whose main job is catching typos and unreachable endpoints.
pub(super) async fn verify_credential(
    descriptor: &ProviderDescriptor,
    base_url: Option<&str>,
    auth: &RequestAuth,
) -> Result<Vec<String>, AuthError> {
    if descriptor.api == ApiKind::OpenaiCodex || !descriptor.lists_models {
        return Ok(Vec::new());
    }
    let Some(base) = base_url else { return Ok(Vec::new()) };
    let url = format!("{}/models", base.trim_end_matches('/'));

    let mut request = crate::http::shared_client().get(&url).header("Accept", "application/json");
    request = auth.apply(request);
    if descriptor.api == ApiKind::AnthropicMessages {
        request = request.header("anthropic-version", "2023-06-01");
    }
    let response = within_start_timeout(&url, "an answer", request.send()).await?;

    let status = response.status();
    let body = within_start_timeout(&url, "the model list", async {
        Ok(response.text().await.unwrap_or_default())
    })
    .await?;
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(AuthError::Rejected(format!(
            "{} rejected that credential ({status})",
            descriptor.name
        )));
    }
    if !status.is_success() {
        // A provider without a `/models` endpoint is not a broken credential. Say so and move on:
        // refusing the login here would block a working local server or a minimal proxy.
        tracing::debug!(provider = %descriptor.id, %status, "provider has no usable model list");
        return Ok(Vec::new());
    }
    Ok(parse_model_ids(&body))
}

/// Pull model ids out of an OpenAI- or Anthropic-shaped list response.
pub(super) fn parse_model_ids(body: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else { return Vec::new() };
    let rows = value.get("data").and_then(|data| data.as_array());
    let mut ids: Vec<String> = rows
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row.get("id").and_then(|id| id.as_str()))
                .filter(|id| !answers_nothing(id))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Whether an id names a model that cannot hold a conversation.
///
/// `/v1/models` is a flat list — LM Studio, Ollama and OpenAI all return an embedding model
/// beside the ones that answer — and nothing in the response says which is which. So the name
/// does, which is a heuristic and is kept to the two families whose ids are a convention rather
/// than a brand: offering one is a default that fails on the first turn, and the reader has no
/// way to know why. A model deliberately named this way is still reachable by declaring it in
/// `models.json`.
pub(super) fn answers_nothing(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    ["embed", "rerank"].iter().any(|kind| id.contains(kind))
}

/// "17 models available" — the line that tells a user the login actually worked.
pub(super) fn describe_models(models: &[String]) -> Option<String> {
    if models.is_empty() {
        return None;
    }
    let preview: Vec<&str> = models.iter().take(MODEL_PREVIEW).map(String::as_str).collect();
    let suffix = if models.len() > preview.len() { ", …" } else { "" };
    Some(format!(
        "{} model{} available ({}{suffix})",
        models.len(),
        if models.len() == 1 { "" } else { "s" },
        preview.join(", ")
    ))
}

/// Validate and tidy a user-typed base URL.
pub(super) fn normalize_base_url(raw: &str) -> Result<String, AuthError> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(AuthError::Input("no URL entered".to_string()));
    }
    let candidate =
        if trimmed.contains("://") { trimmed.to_string() } else { format!("https://{trimmed}") };
    let url = reqwest::Url::parse(&candidate)
        .map_err(|error| AuthError::Input(format!("'{trimmed}' is not a URL ({error})")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AuthError::Input("the endpoint must be an http(s) URL".to_string()));
    }
    if url.host_str().is_none() {
        return Err(AuthError::Input("the endpoint has no host".to_string()));
    }
    Ok(candidate)
}

/// Say so when a key is about to travel over plain http to somewhere that is not this machine.
///
/// A warning, not a refusal: a lab server on the LAN is a legitimate thing to point at, and the
/// user typed the URL. They should know the key crosses the network in the clear.
pub(super) fn warn_if_sent_in_the_clear(url: &str, interaction: &dyn AuthInteraction) {
    let Ok(url) = reqwest::Url::parse(url) else { return };
    if url.scheme() != "http" || is_loopback(&url) {
        return;
    }
    interaction.notify(AuthEvent::Info {
        message: format!(
            "{} is plain http, so your key will cross the network unencrypted.",
            url.host_str().unwrap_or("that endpoint")
        ),
        links: Vec::new(),
    });
}

fn is_loopback(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str() else { return false };
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// `work` against `url`, given as long as a provider gets to start answering.
///
/// Both halves of a verification need it: [`crate::http::send`] stops at the headers, so a server
/// that sends those and then goes quiet would hold the login there for good, and it speaks
/// [`luxe_core::provider::ProviderError`], which would stack a second "network error" on this
/// one's. `what` finishes "timed out after Ns waiting for …".
async fn within_start_timeout<T>(
    url: &str,
    what: &str,
    work: impl std::future::Future<Output = Result<T, reqwest::Error>>,
) -> Result<T, AuthError> {
    let unreachable = |why: String| AuthError::Network(format!("could not reach {url}: {why}"));
    let finished = match crate::http::start_timeout() {
        None => work.await,
        Some(limit) => tokio::time::timeout(limit, work).await.map_err(|_| {
            unreachable(format!("timed out after {}s waiting for {what}", limit.as_secs()))
        })?,
    };
    finished.map_err(|error| unreachable(luxe_core::provider::describe_error_chain(&error)))
}
