//! Anthropic Messages API adapter.
//!
//! Implements the `Provider` trait for Anthropic's API, supporting:
//! - Streaming text, thinking blocks, and tool calls
//! - Extended thinking with signature round-trip
//! - Prompt caching with cache_control markers
//! - Fine-grained tool input streaming

use futures::stream::BoxStream;
use luxe_core::message::Message;
use luxe_core::provider::{
    CompletionRequest, ModelInfo, Provider, ProviderError, ProviderEvent, StopReason,
    ThinkingLevel, Usage,
};
use reqwest::Client;
use serde::Deserialize;

use crate::learned::Learned;

/// Anthropic API base URL.
const ANTHROPIC_API_BASE: &str = "https://api.anthropic.com/v1";

/// Anthropic provider adapter.
pub struct AnthropicProvider {
    client: Client,
    /// Where this call's auth comes from, resolved per request.
    auth: std::sync::Arc<dyn crate::auth::AuthSource>,
    model_info: ModelInfo,
    compat: AnthropicCompat,
    /// API base (defaults to [`ANTHROPIC_API_BASE`]; overridable for proxies).
    base_url: String,
    /// Thinking encodings learned from a rejection, keyed by model id.
    ///
    /// A 400 teaches us which mode a model really accepts; remembering it keeps the correction to
    /// one wasted round trip per model per session instead of one per turn.
    learned_thinking: Learned<ThinkingPlan>,
    /// Output caps learned from a rejection, keyed by model id.
    ///
    /// A model no catalog knows goes out with the current generation's ceiling; an older one that
    /// refuses it names its real limit, and remembering that keeps the retry to once per session.
    learned_max_tokens: Learned<u32>,
    /// Models whose endpoint refused in-place tool loading, so their deferred tools go back to the
    /// list for the rest of the session.
    listed_tools: Learned<()>,
}

impl AnthropicProvider {
    /// Build an adapter from an API key, optional base URL, model info, and compat flags.
    pub fn new(
        api_key: crate::config::ApiKey,
        base_url: Option<String>,
        model_info: ModelInfo,
        compat: AnthropicCompat,
    ) -> Self {
        Self {
            client: crate::http::shared_client(),
            auth: crate::auth::StaticAuth::anthropic(Some(api_key)).into_arc(),
            model_info,
            compat,
            base_url: base_url.unwrap_or_else(|| ANTHROPIC_API_BASE.to_string()),
            learned_thinking: Learned::default(),
            learned_max_tokens: Learned::default(),
            listed_tools: Learned::default(),
        }
    }

    /// Resolve auth from a credential store instead of the fixed key.
    pub fn with_auth(mut self, auth: std::sync::Arc<dyn crate::auth::AuthSource>) -> Self {
        self.auth = auth;
        self
    }

    /// POST one Messages request, encoding thinking per `plan` and deferred tools per `loading`.
    async fn post_messages(
        &self,
        req: &CompletionRequest,
        model_info: &ModelInfo,
        compat: &AnthropicCompat,
        plan: ThinkingPlan,
        loading: ToolLoading,
    ) -> Result<reqwest::Response, ProviderError> {
        // Streaming is requested via `stream: true` in the body — Anthropic has a single
        // `POST /v1/messages` endpoint, there is no `/messages/stream`.
        let mut body = build_anthropic_request(req, model_info, compat, plan, loading);
        body["stream"] = serde_json::Value::Bool(true);

        // Anthropic authenticates with the `x-api-key` header (not `Authorization: Bearer`); the
        // auth source supplies it. `anthropic-version` is protocol, not auth, so it stays here.
        let auth = self.auth.request_auth().await?;
        let base = auth.base_url.clone().unwrap_or_else(|| self.base_url.clone());
        let url = format!("{}/messages", base.trim_end_matches('/'));
        let mut headers: Vec<(&str, &str)> =
            vec![("anthropic-version", "2023-06-01"), ("Content-Type", "application/json")];
        if compat.copilot_headers {
            headers.extend(crate::copilot::turn_headers(req));
        }
        let request = crate::http::json_request(self.client.post(&url), &headers, &body);
        crate::wire::record_request(self.name(), &url, &headers, &body);
        crate::http::send(auth.apply(request)).await
    }

    /// The flags this model's requests are shaped by: its own when it resolved any, else the
    /// provider's.
    ///
    /// Owned rather than borrowed because the projection is two fields and the decoder wants a copy
    /// of its own — borrowing would put a lifetime on the adapter for the sake of a bool.
    fn compat_for(&self, model_info: &ModelInfo) -> AnthropicCompat {
        model_info.compat.as_ref().map_or_else(|| self.compat.clone(), AnthropicCompat::from)
    }

    /// The thinking encoding to open with: whatever a rejection already taught us about this
    /// model, else what its declared controls imply.
    fn thinking_plan(&self, model_info: &ModelInfo, compat: &AnthropicCompat) -> ThinkingPlan {
        self.learned_thinking
            .get(&model_info.r#ref.id)
            .unwrap_or_else(|| ThinkingPlan::for_model(model_info, compat))
    }

    fn remember_thinking_plan(&self, model_info: &ModelInfo, plan: ThinkingPlan) {
        self.learned_thinking.remember(&model_info.r#ref.id, plan);
    }

    /// The output cap a rejection already taught us for this model, if any.
    fn learned_max_tokens(&self, model_info: &ModelInfo) -> Option<u32> {
        self.learned_max_tokens.get(&model_info.r#ref.id)
    }

    fn remember_max_tokens(&self, model_info: &ModelInfo, limit: u32) {
        self.learned_max_tokens.remember(&model_info.r#ref.id, limit);
    }

    /// How this model's deferred tools load: in place unless the flags or a refusal said not to.
    fn tool_loading(&self, model_info: &ModelInfo, compat: &AnthropicCompat) -> ToolLoading {
        let refused = self.listed_tools.knows(&model_info.r#ref.id);
        if compat.defer_tool_loading && !refused {
            ToolLoading::InPlace
        } else {
            ToolLoading::Listed
        }
    }

    fn remember_listed_tools(&self, model_info: &ModelInfo) {
        self.listed_tools.remember(&model_info.r#ref.id, ());
    }
}

/// The output cap Anthropic names when it refuses `max_tokens`, or `None` when the 400 is about
/// something else.
///
/// The wording has held since 2024: `max_tokens: 128000 > 64000, which is the maximum allowed
/// number of output tokens for claude-opus-4-5-20251101`.
fn max_tokens_limit(body: &str) -> Option<u32> {
    if !body.contains("maximum allowed number of output tokens") {
        return None;
    }
    let (_, rest) = body.split_once("max_tokens: ")?;
    let (_, rest) = rest.split_once(" > ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|limit| *limit > 0)
}

/// Anthropic-specific compatibility flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnthropicCompat {
    /// Force adaptive thinking mode (default false).
    pub force_adaptive_thinking: bool,
    /// Use cache_control markers (default true).
    pub cache_control: bool,
    /// Keep cache entries for an hour rather than the default five minutes.
    pub long_cache_retention: bool,
    /// Ask for a tool call's arguments as the model writes them rather than in one lump at the end.
    pub stream_tool_arguments: bool,
    /// Send GitHub Copilot's per-request headers, for Claude reached through Copilot.
    pub copilot_headers: bool,
    /// Load deferred tools with `defer_loading` and `tool_reference` (default true).
    pub defer_tool_loading: bool,
}

impl Default for AnthropicCompat {
    fn default() -> Self {
        Self {
            force_adaptive_thinking: false,
            cache_control: true,
            long_cache_retention: false,
            stream_tool_arguments: true,
            copilot_headers: false,
            defer_tool_loading: true,
        }
    }
}

impl From<&luxe_core::provider::CompatFlags> for AnthropicCompat {
    /// The flags this API has an opinion about, projected out of the shared set.
    ///
    /// `cache_control_format` doubles as the on switch: naming a format is how caching is asked
    /// for, and `null` is how it is refused.
    fn from(flags: &luxe_core::provider::CompatFlags) -> Self {
        Self {
            force_adaptive_thinking: flags.force_adaptive_thinking,
            cache_control: flags.cache_control_format.is_some(),
            long_cache_retention: flags.supports_long_cache_retention,
            stream_tool_arguments: flags.stream_tool_arguments,
            copilot_headers: flags.copilot_headers,
            defer_tool_loading: flags.defer_tool_loading,
        }
    }
}

#[async_trait::async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    async fn stream(
        &self,
        mut req: CompletionRequest,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        // Honor the *per-request* model so a runtime model switch (`Action::SetModel`) actually
        // changes the id/thinking sent to Anthropic. Fall back to the adapter's build-time model
        // only when the request carries the placeholder default (`unknown`).
        let model_info =
            if req.model.r#ref.id == "unknown" { &self.model_info } else { &req.model };
        // Per request for the same reason: one adapter serves every Claude generation, and how
        // thinking is asked for differs between them.
        let compat = self.compat_for(model_info);
        let mut plan = self.thinking_plan(model_info, &compat);
        let mut loading = self.tool_loading(model_info, &compat);
        if let Some(limit) = self.learned_max_tokens(model_info) {
            req.max_tokens = req.max_tokens.map(|max| max.min(limit));
        }
        let mut response = self.post_messages(&req, model_info, &compat, plan, loading).await?;

        // Three 400s are recoverable rather than fatal, each at most once: an output cap the model
        // can't honor, which Anthropic answers with the real limit; a thinking encoding from the
        // wrong generation, which a model our catalog hasn't caught up with resolves to; and an
        // endpoint that doesn't know in-place tool loading. Fix the request, retry, and remember
        // what worked.
        let (mut cap_revised, mut plan_revised) = (false, false);
        while response.status() == reqwest::StatusCode::BAD_REQUEST {
            let rejection = response.text().await.unwrap_or_default();
            if let Some(limit) = max_tokens_limit(&rejection).filter(|_| !cap_revised) {
                tracing::warn!(
                    model = %model_info.r#ref.id,
                    requested = ?req.max_tokens,
                    limit,
                    "anthropic refused the output cap; retrying at the model's limit"
                );
                req.max_tokens = req.max_tokens.map(|max| max.min(limit));
                self.remember_max_tokens(model_info, limit);
                cap_revised = true;
            } else if let Some(revised) = plan.revised(&rejection).filter(|_| !plan_revised) {
                tracing::warn!(
                    model = %model_info.r#ref.id,
                    ?plan,
                    ?revised,
                    "anthropic rejected the thinking configuration; retrying with the other encoding"
                );
                plan = revised;
                plan_revised = true;
            } else if loading == ToolLoading::InPlace && refuses_in_place_loading(&rejection) {
                tracing::warn!(
                    model = %model_info.r#ref.id,
                    %rejection,
                    "the endpoint refused in-place tool loading; listing deferred tools instead"
                );
                loading = ToolLoading::Listed;
                self.remember_listed_tools(model_info);
            } else {
                return Err(crate::http::status_error(reqwest::StatusCode::BAD_REQUEST, rejection));
            }
            response = self.post_messages(&req, model_info, &compat, plan, loading).await?;
            if response.status().is_success() && plan_revised {
                self.remember_thinking_plan(model_info, plan);
            }
        }

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(crate::http::status_error(status, text));
        }

        // Decode the SSE body as it arrives: reasoning and text have to reach the UI while the
        // model is still producing them, and a cancel has to be able to drop the connection.
        Ok(crate::sse::decode(response.bytes_stream(), cancel, AnthropicDecoder::default()))
    }

    /// Five minutes, or an hour on the long tier — unless the request would cache differently when
    /// replayed with a one-token cap.
    ///
    /// That is budget-based thinking: the budget is derived from `max_tokens`, Anthropic keys the
    /// message cache on it, and a replayed request would also be allowed to think for thousands of
    /// tokens. Adaptive thinking, and thinking switched off, replay byte-for-byte apart from the cap.
    fn prompt_cache_ttl(&self, req: &CompletionRequest) -> Option<std::time::Duration> {
        let model_info =
            if req.model.r#ref.id == "unknown" { &self.model_info } else { &req.model };
        let compat = self.compat_for(model_info);
        if !compat.cache_control || !req.reuse.worth_caching() {
            return None;
        }
        let budgeted = self.thinking_plan(model_info, &compat).mode == ThinkingMode::Extended
            && thinking_is_on(model_info, req.thinking, &compat);
        if budgeted {
            return None;
        }
        let minutes = if compat.long_cache_retention { 60 } else { 5 };
        Some(std::time::Duration::from_secs(minutes * 60))
    }

    /// Replays `req` through [`Provider::stream`] at one output token and reads the usage off the
    /// end of it, so the body is the one the turn sent — same breakpoints, same learned thinking
    /// encoding — and only `max_tokens` differs, which is not part of the cache key.
    ///
    /// One token rather than the documented `max_tokens: 0`, which Anthropic refuses on a streamed
    /// request and on any request with budget-based thinking.
    async fn warm_cache(&self, mut req: CompletionRequest) -> Result<Usage, ProviderError> {
        use futures::StreamExt;

        req.max_tokens = Some(1);
        // Nobody cancels a refresh by flag; the warmer drops this future instead
        let (_never, cancel) = tokio::sync::watch::channel(false);
        let mut events = self.stream(req, cancel).await?;
        let mut usage = None;
        while let Some(event) = events.next().await {
            match event {
                ProviderEvent::Usage(measured) => usage = Some(measured),
                ProviderEvent::Error { message, .. } => return Err(ProviderError::Other(message)),
                ProviderEvent::Done { .. } => break,
                _ => {}
            }
        }
        usage.ok_or_else(|| ProviderError::Other("the refresh reported no usage".to_string()))
    }
}

mod request;
mod sse;

use request::*;
use sse::*;

#[cfg(test)]
mod tests;
