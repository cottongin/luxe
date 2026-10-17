//! Support — the provider and request every test here starts from.

use super::*;

pub(super) fn make_provider() -> OpenaiResponsesProvider {
    OpenaiResponsesProvider::new(ProviderConfig {
        api_key: Some("sk-test".into()),
        base_url: Some("https://api.openai.com/v1".into()),
        ..Default::default()
    })
}

pub(super) fn make_req(model_id: &str) -> CompletionRequest {
    CompletionRequest {
        model: ModelInfo { r#ref: ModelRef::new("openai", model_id), ..Default::default() },
        messages: vec![Message::user("Hello")],
        system: None,
        tools: vec![],
        thinking: ThinkingLevel::Off,
        max_tokens: None,
        ..Default::default()
    }
}

// -----------------------------------------------------------------------
// Codex flavour
// -----------------------------------------------------------------------

/// Header names and shapes taken verbatim from a live ChatGPT Plus response (2026-08-05).
pub(super) fn codex_headers(pairs: &[(&str, &str)]) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    for (name, value) in pairs {
        headers.insert(
            reqwest::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            reqwest::header::HeaderValue::from_str(value).unwrap(),
        );
    }
    headers
}
