//! Provider trait + request/response types.
//!
//! The [`Provider`] trait is transport-agnostic and lives in core; adapters live in
//! `luxe-providers` and depend on core (never the reverse).
//!
//! No HTTP, no UI, no process spawning — just traits and data shapes.

use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

use crate::message::Message;

mod api_kind;
mod error;
mod model;
mod plan_usage;
mod pricing;
mod request;

pub use api_kind::ApiKind;
pub use error::{
    classify_provider_message, describe_error_chain, ProviderError, ProviderErrorKind,
};
pub use model::*;
pub use plan_usage::*;
pub use pricing::*;
pub use request::*;

/// The Provider trait — transport-agnostic.
///
/// Adapters implement this to stream completions from an LLM. The agent loop maps
/// [`ProviderEvent`]s onto core [`Event`](crate::event::Event)s.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Provider name (e.g. "openai", "anthropic").
    fn name(&self) -> &str;

    /// Stream a completion. Returns a stream of [`ProviderEvent`]s.
    async fn stream(
        &self,
        req: CompletionRequest,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError>;

    /// How long the prompt cache entries `req` writes stay alive, when this provider can keep them
    /// warm with [`Provider::warm_cache`].
    ///
    /// `None` unless the adapter can replay `req` without changing what it caches: a provider that
    /// doesn't cache, caching switched off, a one-off prompt, or a request whose cache key depends
    /// on its output cap. That last one is Anthropic's budget-based thinking, where the budget is
    /// derived from `max_tokens` and the message cache is keyed on the budget.
    fn prompt_cache_ttl(&self, _req: &CompletionRequest) -> Option<std::time::Duration> {
        None
    }

    /// Re-sends `req` with a one-token output cap, so every cache entry it reads gets a fresh
    /// lifetime for the price of a cache read. Cancelled by dropping the future.
    ///
    /// Only called when [`Provider::prompt_cache_ttl`] said `req` can be kept warm.
    async fn warm_cache(&self, _req: CompletionRequest) -> Result<Usage, ProviderError> {
        Err(ProviderError::Other("this provider cannot keep a prompt cache warm".to_string()))
    }
}

#[cfg(test)]
mod tests;
