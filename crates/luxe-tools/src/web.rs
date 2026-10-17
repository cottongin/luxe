// Web search and fetch tools.
//
// web_search — pluggable search backend (Brave/Tavily/Exa/Firecrawl) returning normalized hits.
// web_fetch — GET URL, extract readable content → markdown, with optional stealth fallback.

mod clone;
mod fetch;
mod render;
mod search;

pub use fetch::WebFetchTool;
pub use search::{create_backend, Recency, SearchBackend, SearchHit, SearchOptions, WebSearchTool};

use anyhow::Result;
use async_trait::async_trait;
use luxe_core::provider::ToolSchema;
use luxe_core::tool::{Tool, ToolContext, ToolResultData};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use url::Url;

/// Web tool error.
#[derive(Debug, thiserror::Error)]
pub enum WebError {
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("network error: {0}")]
    Network(String),
    /// The far end never finished answering: a name that would not resolve, or a socket that
    /// connected and then went quiet. Its own variant because it is the one failure a retry cannot
    /// help — see `WebError::worth_a_second_try`.
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("backend error: {0}")]
    Backend(String),
}

impl WebError {
    /// Whether refetching with a browser-like user agent could plausibly do better.
    ///
    /// The stealth retry exists to get past bot protection — a 403, a challenge page, a body that is
    /// all JavaScript — and it works by looking like a browser. A host that never answered does not
    /// care what we call ourselves, so retrying a timeout only spends the budget twice: two 30s
    /// attempts against a silent host is a minute of nothing, which is exactly how a dead domain came
    /// to look like a hung agent.
    fn worth_a_second_try(&self) -> bool {
        !matches!(self, Self::Timeout(_))
    }
}

/// Configuration for web tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    /// Search backend to use (brave, tavily, exa, firecrawl).
    pub search_backend: Option<String>,
    /// API key for the search backend.
    pub search_api_key: Option<String>,
    /// Enable stealth fetch fallback.
    pub enable_stealth: bool,
    /// Firecrawl key, held apart from the search key because it also backs `web_fetch`'s last
    /// resort — so it is worth having even when another vendor won the search slot.
    #[serde(default)]
    pub firecrawl_api_key: Option<String>,
    /// Whether a GitHub repository URL is cloned into `.luxe/gh-cache/` rather than read as a page.
    #[serde(default = "clone_by_default")]
    pub clone_repositories: bool,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            search_backend: None,
            search_api_key: None,
            enable_stealth: false,
            firecrawl_api_key: None,
            clone_repositories: clone_by_default(),
        }
    }
}

fn clone_by_default() -> bool {
    true
}

/// The keyed search vendors, in the order a configured key claims the slot.
const SEARCH_KEYS: &[(&str, &str)] = &[
    ("brave", "BRAVE_API_KEY"),
    ("tavily", "TAVILY_API_KEY"),
    ("exa", "EXA_API_KEY"),
    ("firecrawl", "FIRECRAWL_API_KEY"),
];

impl WebConfig {
    /// Build config from the environment: a configured key picks its backend, in `SEARCH_KEYS`
    /// order, and otherwise search falls back to Exa's keyless MCP endpoint.
    ///
    /// An explicit key outranks the fallback — someone who configured one chose it. But search is
    /// a *capability*, not a setup step: without this fallback, `web_search` was registered and
    /// then failed on first use for every user who had not signed up for a search vendor.
    pub fn from_env() -> Self {
        let (backend, key) = SEARCH_KEYS
            .iter()
            .find_map(|(name, var)| env_key(var).map(|key| ((*name).to_string(), key)))
            .map_or((None, None), |(name, key)| (Some(name), Some(key)));
        Self {
            search_backend: backend,
            search_api_key: key,
            enable_stealth: true,
            firecrawl_api_key: env_key("FIRECRAWL_API_KEY"),
            clone_repositories: clone_by_default(),
        }
    }

    /// Whether a search backend key is configured. Search works either way — this only says
    /// whether it will go through a keyed vendor or the keyless fallback.
    pub fn has_search_key(&self) -> bool {
        self.search_api_key.is_some()
    }
}

/// A non-blank environment value, since an exported-but-empty key is not a key.
fn env_key(var: &str) -> Option<String> {
    std::env::var(var).ok().map(|value| value.trim().to_string()).filter(|value| !value.is_empty())
}

/// Web tools (search + fetch) with shared configuration.
pub struct WebTools {
    config: Arc<RwLock<WebConfig>>,
    /// Read out at construction, because the schema promises it and a schema is built synchronously
    clone_repositories: bool,
}

impl WebTools {
    pub fn new(config: WebConfig) -> Self {
        let clone_repositories = config.clone_repositories;
        Self { config: Arc::new(RwLock::new(config)), clone_repositories }
    }

    /// Create a web_search tool.
    pub fn web_search(&self) -> WebSearchTool {
        WebSearchTool { config: self.config.clone() }
    }

    /// Create a web_fetch tool.
    pub fn web_fetch(&self) -> WebFetchTool {
        WebFetchTool { config: self.config.clone(), clone_repositories: self.clone_repositories }
    }
}

#[cfg(test)]
mod tests;
