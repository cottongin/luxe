//! OpenAI-compatible provider adapter.
//!
//! Implements the [`Provider`] trait for any API that speaks the OpenAI chat completions
//! format (OpenAI, Ollama, vLLM, LMStudio, etc.).

use crate::config::{CompatFlags, ProviderConfig};
use futures::stream::BoxStream as FBoxStream;
use luxe_core::provider::{CompletionRequest, Provider, ProviderError, ProviderEvent};
use tracing::debug;

mod request;
mod sse;
mod thinking;

pub(crate) use request::copilot_request_headers;
use request::request_headers;
pub(crate) use sse::CompatDecoder;

/// Default OpenAI API base URL.
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// OpenAI-compatible provider adapter.
#[derive(Debug, Clone)]
pub struct OpenaiCompatProvider {
    /// Provider configuration.
    config: ProviderConfig,
    /// Compatibility flags.
    compat: CompatFlags,
    /// Optional base URL override.
    base_url: Option<String>,
    /// Where this call's auth comes from. Resolved per request so an OAuth token can refresh and a
    /// mid-session `/login` takes effect without rebuilding the adapter.
    auth: std::sync::Arc<dyn crate::auth::AuthSource>,
}

impl OpenaiCompatProvider {
    /// Create a new adapter from configuration.
    pub fn new(config: ProviderConfig, compat: CompatFlags) -> Self {
        let auth = crate::auth::StaticAuth::bearer(config.api_key.clone()).into_arc();
        Self { config, compat, base_url: None, auth }
    }

    /// Set a custom base URL (overrides config).
    pub fn with_base_url(mut self, url: String) -> Self {
        self.base_url = Some(url);
        self
    }

    /// Resolve auth from a credential store instead of the fixed config key.
    pub fn with_auth(mut self, auth: std::sync::Arc<dyn crate::auth::AuthSource>) -> Self {
        self.auth = auth;
        self
    }

    /// Get the effective base URL.
    fn effective_base_url(&self) -> &str {
        self.base_url
            .as_deref()
            .unwrap_or(self.config.base_url.as_deref().unwrap_or(DEFAULT_BASE_URL))
    }

    /// The flags this request is shaped by: the model's own when it resolved any, else the
    /// provider's.
    ///
    /// Read per request rather than held, because one adapter serves every model on its endpoint and
    /// a local server hosting two builds is the ordinary case — the chat templates disagree about the
    /// `developer` role and about how thinking is asked for, and the adapter is built once.
    fn compat_for<'a>(&'a self, req: &'a CompletionRequest) -> &'a CompatFlags {
        req.model.compat.as_ref().unwrap_or(&self.compat)
    }
}

#[async_trait::async_trait]
impl Provider for OpenaiCompatProvider {
    fn name(&self) -> &str {
        "openai_compat"
    }

    async fn stream(
        &self,
        req: CompletionRequest,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<FBoxStream<'static, ProviderEvent>, ProviderError> {
        // Auth first: a missing credential is a question for the user, not a request to send.
        let auth = self.auth.request_auth().await?;
        // `base_url` already includes the API version segment (e.g. `.../v1`). A credential may pin
        // its own endpoint (GitHub Copilot's per-account proxy), which wins over the default.
        let base = auth.base_url.clone().unwrap_or_else(|| self.effective_base_url().to_string());
        let url = format!("{}/chat/completions", base.trim_end_matches('/'));
        let body = self.build_request_body(&req);

        debug!(url = %url, model = %req.model.r#ref.id, "sending completion request");

        let mut headers = vec![("Content-Type", "application/json")];
        headers.extend(request_headers(self.compat_for(&req), &req));
        let request =
            crate::http::json_request(crate::http::shared_client().post(&url), &headers, &body);
        crate::wire::record_request(self.name(), &url, &headers, &body);
        let response = crate::http::send(auth.apply(request)).await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(crate::http::status_error(status, body));
        }

        // Decode the SSE body as it arrives so text and reasoning reach the UI while the model is
        // still producing them, and a cancel can drop the connection mid-call.
        Ok(crate::sse::decode(response.bytes_stream(), cancel, CompatDecoder::default()))
    }
}

#[cfg(test)]
mod tests;
