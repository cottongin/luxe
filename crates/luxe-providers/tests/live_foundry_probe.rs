//! Live smoke test: do both halves of a Microsoft Foundry resource still answer?
//!
//! Ignored, so it never runs in CI or on a machine without a Foundry resource, and reports as
//! ignored rather than passed. Run on purpose, it needs a stored credential for at least one half.
//! It exists because the two things Luxe had to decide from documentation — that the Anthropic
//! passthrough authenticates on `x-api-key`/`api-key` rather than a bearer token, and that the
//! OpenAI half takes a bearer — are only really settled by a 200. A failure here means Foundry moved, not that the login is broken.
//!
//! Run it with `--no-capture` to see each response's status and first bytes, which is what you want
//! when it fails:
//!
//! ```text
//! cargo nextest run -p luxe-providers \
//!   -E 'binary(live_foundry_probe)' --run-ignored only --no-capture
//! ```

#![allow(clippy::unwrap_used)]

use luxe_providers::auth::{provider_descriptor, AuthStore, Credential};
use luxe_providers::config::{load_models_config, ApiKey};

/// The smallest turn that still proves streaming, tool-free, one word each way.
const PROMPT: &str = "Reply with the single word: pong";

#[tokio::test]
#[ignore = "needs a configured Foundry resource and a network"]
async fn both_foundry_surfaces_accept_the_stored_key() {
    let mut checked = 0usize;
    for (id, model_env, default_model) in [
        ("foundry-anthropic", "LUXE_TEST_FOUNDRY_CLAUDE", "claude-sonnet-5"),
        ("foundry-openai", "LUXE_TEST_FOUNDRY_GPT", "gpt-5.6-sol"),
    ] {
        let Some(key) = stored_key(id) else {
            eprintln!("skipping {id}: nothing stored — run `luxe auth login foundry`");
            continue;
        };
        let Some(base_url) = configured_base_url(id) else {
            eprintln!("skipping {id}: no base_url in models.json — run `luxe auth login foundry`");
            continue;
        };
        let model = std::env::var(model_env).unwrap_or_else(|_| default_model.to_string());
        let descriptor = provider_descriptor(id).expect("in catalog");

        let (url, body) = match id {
            "foundry-anthropic" => (
                format!("{base_url}/messages"),
                serde_json::json!({
                    "model": model,
                    "max_tokens": 64,
                    "stream": true,
                    "messages": [{ "role": "user", "content": PROMPT }],
                }),
            ),
            _ => (
                format!("{base_url}/responses"),
                serde_json::json!({
                    "model": model,
                    "stream": true,
                    "store": false,
                    "max_output_tokens": 1024,
                    "input": [{
                        "type": "message",
                        "role": "user",
                        "content": [{ "type": "input_text", "text": PROMPT }],
                    }],
                }),
            ),
        };

        let mut request = reqwest::Client::new()
            .post(&url)
            .header("Accept", "text/event-stream")
            .header("Content-Type", "application/json");
        if descriptor.api == luxe_core::auth::ApiKind::AnthropicMessages {
            request = request.header("anthropic-version", "2023-06-01");
        }
        request = descriptor.key_style().headers(&key).apply(request);

        let response = request.json(&body).send().await.expect("Foundry is reachable");
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        println!("{id} -> {url}\n  status: {status}\n  body: {}", &text[..text.len().min(400)]);

        assert!(
            status.is_success(),
            "{id} refused the request ({status}) at {url}. A 401 means the key headers moved \
             (Luxe sends {:?}); a 404 means the path did; a 400 naming the model means \
             '{model}' is not a deployment on this resource.\n{text}",
            descriptor.key_style(),
        );
        assert!(
            text.contains("data:"),
            "{id} answered {status} but not as a stream — the SSE decoder would see nothing:\n{text}"
        );
        checked += 1;
    }

    assert!(checked > 0, "nothing was configured to probe — run `luxe auth login foundry` first");
}

/// The stored credential for `id`, resolved the way a request resolves it.
fn stored_key(id: &str) -> Option<ApiKey> {
    match AuthStore::default_path().credential(id)? {
        Credential::ApiKey(api) => {
            let resolved = luxe_providers::config::resolve_value(api.key.expose());
            (!resolved.trim().is_empty()).then(|| ApiKey::new(resolved))
        }
        Credential::OAuth(_) => None,
    }
}

/// The base URL the login recorded for `id`.
fn configured_base_url(id: &str) -> Option<String> {
    let path = luxe_providers::auth::ConfigFile::default_path().path().to_path_buf();
    let base = load_models_config(&path).ok()?.providers.get(id)?.base_url.clone()?;
    Some(base.trim_end_matches('/').to_string())
}
