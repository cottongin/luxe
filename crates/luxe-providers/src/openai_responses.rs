//! OpenAI Responses API adapter.
//!
//! Implements the [`Provider`] trait for the `/v1/responses` endpoint, which newer OpenAI
//! models (e.g. `gpt-5`, `gpt-5-codex`) require. The classic `/v1/chat/completions` endpoint
//! is rejected by those models with "use the v1/responses endpoint instead".
//!
//! # Wire format
//! - Request body: `model`, `stream: true`, `instructions` (system prompt), `max_output_tokens`,
//!   `reasoning`, `tools` (flat — not nested under "function"), and `input` (array of items).
//! - SSE events identified by `event: <type>` / `data: <json>` pairs.

use crate::config::ProviderConfig;
use futures::stream::BoxStream as FBoxStream;
use luxe_core::provider::{CompletionRequest, Provider, ProviderError, ProviderEvent};
use tracing::{debug, warn};

use crate::learned::Learned;

mod plan;
mod request;
mod sse;

pub use plan::plan_usage_from_headers;
use request::refuses_in_place_loading;
#[cfg(test)]
pub(crate) use sse::parse_responses_sse;
use sse::ResponsesDecoder;

/// Default OpenAI API base URL.
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// Base URL of the ChatGPT-subscription Codex backend.
const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";

/// Which flavour of the Responses API an adapter speaks.
///
/// Same wire format and the same SSE decoder either way; the ChatGPT backend differs in its path,
/// in refusing to store a response, and in not accepting an output cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponsesFlavor {
    /// `POST {base}/responses` on an API-key endpoint.
    Standard,
    /// `POST {base}/codex/responses` against a ChatGPT plan.
    Codex,
}

/// OpenAI Responses API provider adapter.
#[derive(Debug, Clone)]
pub struct OpenaiResponsesProvider {
    /// Provider configuration (API key, base URL, compat flags).
    config: ProviderConfig,
    /// Standard endpoint or the ChatGPT-subscription Codex backend.
    flavor: ResponsesFlavor,
    /// Where this call's auth comes from, resolved per request.
    auth: std::sync::Arc<dyn crate::auth::AuthSource>,
    /// Models whose endpoint refused `additional_tools`, so their deferred tools go back to the
    /// list for the rest of the session.
    listed_tools: std::sync::Arc<Learned<()>>,
}

impl OpenaiResponsesProvider {
    /// Create a new adapter from configuration.
    pub fn new(config: ProviderConfig) -> Self {
        let auth = crate::auth::StaticAuth::bearer(config.api_key.clone()).into_arc();
        Self { config, flavor: ResponsesFlavor::Standard, auth, listed_tools: Default::default() }
    }

    /// Speak to the ChatGPT-subscription Codex backend instead of an API-key endpoint.
    pub fn codex(mut self) -> Self {
        self.flavor = ResponsesFlavor::Codex;
        self
    }

    /// Resolve auth from a credential store instead of the fixed config key.
    pub fn with_auth(mut self, auth: std::sync::Arc<dyn crate::auth::AuthSource>) -> Self {
        self.auth = auth;
        self
    }

    /// Get the effective base URL.
    fn effective_base_url(&self) -> &str {
        self.config.base_url.as_deref().unwrap_or(match self.flavor {
            ResponsesFlavor::Standard => DEFAULT_BASE_URL,
            ResponsesFlavor::Codex => CODEX_BASE_URL,
        })
    }

    /// The endpoint one request goes to.
    fn request_url(&self, base: &str) -> String {
        let base = base.trim_end_matches('/');
        match self.flavor {
            ResponsesFlavor::Standard => format!("{base}/responses"),
            // Tolerate a base URL that already names the path, so a user pointing at a proxy can
            // configure either form.
            ResponsesFlavor::Codex if base.ends_with("/codex/responses") => base.to_string(),
            ResponsesFlavor::Codex if base.ends_with("/codex") => format!("{base}/responses"),
            ResponsesFlavor::Codex => format!("{base}/codex/responses"),
        }
    }

    /// Whether this request's deferred tools load in place: unless the flags or a refusal said not
    /// to.
    fn loads_in_place(&self, req: &CompletionRequest) -> bool {
        let compat = req.model.compat.as_ref().unwrap_or(&self.config.compat);
        compat.defer_tool_loading && !self.listed_tools.knows(&req.model.r#ref.id)
    }

    fn remember_listed_tools(&self, req: &CompletionRequest) {
        self.listed_tools.remember(&req.model.r#ref.id, ());
    }
}

#[async_trait::async_trait]
impl Provider for OpenaiResponsesProvider {
    fn name(&self) -> &str {
        "openai_responses"
    }

    async fn stream(
        &self,
        req: CompletionRequest,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<FBoxStream<'static, ProviderEvent>, ProviderError> {
        // Auth first: a missing credential is a question for the user, not a request to send.
        let auth = self.auth.request_auth().await?;
        let base = auth.base_url.clone().unwrap_or_else(|| self.effective_base_url().to_string());
        let url = self.request_url(&base);
        let mut in_place = self.loads_in_place(&req);

        debug!(url = %url, model = %req.model.r#ref.id, "sending responses request");

        let mut headers =
            vec![("Content-Type", "application/json"), ("Accept", "text/event-stream")];
        let compat = req.model.compat.as_ref().unwrap_or(&self.config.compat);
        headers.extend(crate::openai_compat::copilot_request_headers(compat, &req));
        let response = loop {
            let body = self.body_for(&req, in_place);
            let request =
                crate::http::json_request(crate::http::shared_client().post(&url), &headers, &body);
            crate::wire::record_request(self.name(), &url, &headers, &body);
            let response = crate::http::send(auth.apply(request)).await?;
            if !(in_place && response.status() == reqwest::StatusCode::BAD_REQUEST) {
                break response;
            }
            // One recoverable 400: an endpoint that doesn't know `additional_tools`. List the
            // deferred tools instead, retry once, and remember it for this model
            let body_text = response.text().await.unwrap_or_default();
            if !refuses_in_place_loading(&body_text) {
                return Err(crate::http::status_error(reqwest::StatusCode::BAD_REQUEST, body_text));
            }
            warn!(
                model = %req.model.r#ref.id,
                rejection = %body_text,
                "the endpoint refused in-place tool loading; listing deferred tools instead"
            );
            self.remember_listed_tools(&req);
            in_place = false;
        };

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            return Err(crate::http::status_error(status, body_text));
        }

        // A subscription backend reports how much of the plan is left in the response headers. Read
        // it before consuming the body, and lead the stream with it so the UI can show headroom for
        // the turn that is about to run.
        let plan = plan_usage_from_headers(response.headers());
        let events = crate::sse::decode(response.bytes_stream(), cancel, ResponsesDecoder);
        Ok(match plan {
            Some(plan) => {
                use futures::StreamExt;
                let lead = futures::stream::once(async move { ProviderEvent::PlanUsage(plan) });
                Box::pin(lead.chain(events))
            }
            None => events,
        })
    }
}

#[cfg(test)]
mod tests;
