//! Tavily — a search API built for agents, so it takes luxe's whole option set as parameters.
//!
//! Domain include/exclude, a recency window and a result cap are all first-class here, and the
//! `content` field on each result is already the query-relevant extract rather than a meta
//! description. The key goes in the `Authorization` header: Tavily's original body-field auth
//! still answers, but it is the deprecated spelling and not what a new integration should send.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::web::{SearchBackend, SearchHit, SearchOptions, WebError};

/// Tavily Search API request.
#[derive(Debug, Serialize)]
struct TavilyRequest {
    query: String,
    max_results: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    include_domains: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    exclude_domains: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    time_range: Option<&'static str>,
    /// `advanced` costs a second credit and buys longer, better-targeted extracts, which is the
    /// trade this tool exists to make when the caller asked for content.
    search_depth: &'static str,
}

/// Tavily Search API response.
#[derive(Debug, Deserialize)]
struct TavilyResponse {
    results: Option<Vec<TavilyResult>>,
}

#[derive(Debug, Deserialize)]
struct TavilyResult {
    title: Option<String>,
    url: Option<String>,
    content: Option<String>,
}

/// Tavily search backend.
pub struct TavilyBackend {
    api_key: String,
    base_url: String,
}

impl TavilyBackend {
    /// A backend against Tavily's public endpoint.
    pub fn new(api_key: String) -> Self {
        Self { api_key, base_url: "https://api.tavily.com/search".to_string() }
    }

    /// Point the backend at a stand-in endpoint (tests, a proxy).
    pub fn with_base_url(api_key: String, base_url: impl Into<String>) -> Self {
        Self { api_key, base_url: base_url.into() }
    }

    /// The body for one search.
    fn request_body(&self, query: &str, n: usize, options: &SearchOptions) -> TavilyRequest {
        TavilyRequest {
            query: query.to_string(),
            max_results: n.max(1),
            include_domains: options.include_domains.clone(),
            exclude_domains: options.exclude_domains.clone(),
            time_range: options.recency.map(|recency| recency.tavily_range()),
            search_depth: match options.include_content {
                true => "advanced",
                false => "basic",
            },
        }
    }
}

#[async_trait]
impl SearchBackend for TavilyBackend {
    async fn search(
        &self,
        query: &str,
        n: usize,
        options: &SearchOptions,
    ) -> Result<Vec<SearchHit>, WebError> {
        let response = reqwest::Client::new()
            .post(&self.base_url)
            .header("Content-Type", "application/json")
            .bearer_auth(&self.api_key)
            .json(&self.request_body(query, n, options))
            .send()
            .await
            .map_err(|e| WebError::Network(e.to_string()))?;

        let status = response.status();
        let body = response.text().await.map_err(|e| WebError::Network(e.to_string()))?;
        if !status.is_success() {
            return Err(WebError::Http(format!("Tavily returned {status}: {}", body.trim())));
        }

        let parsed: TavilyResponse =
            serde_json::from_str(&body).map_err(|e| WebError::Parse(e.to_string()))?;
        Ok(parsed
            .results
            .unwrap_or_default()
            .into_iter()
            .map(|result| SearchHit {
                title: result.title.unwrap_or_default(),
                url: result.url.unwrap_or_default(),
                snippet: result.content.unwrap_or_default(),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(options: &SearchOptions) -> serde_json::Value {
        let backend = TavilyBackend::new("test-key".to_string());
        serde_json::to_value(backend.request_body("tokio", 3, options)).expect("serializes")
    }

    /// Every filter the tool schema advertises is a real Tavily parameter, so all of them have to
    /// reach the request. Dropping them returns plausible results for a question nobody asked.
    #[test]
    fn the_advertised_filters_all_reach_the_request() {
        let body = body(&SearchOptions {
            include_domains: vec!["docs.rs".to_string()],
            exclude_domains: vec!["reddit.com".to_string()],
            recency: Some(crate::web::Recency::Year),
            include_content: true,
        });
        assert_eq!(body["query"], "tokio");
        assert_eq!(body["max_results"], 3);
        assert_eq!(body["include_domains"][0], "docs.rs");
        assert_eq!(body["exclude_domains"][0], "reddit.com");
        assert_eq!(body["time_range"], "year");
        assert_eq!(body["search_depth"], "advanced");
    }

    /// An unasked filter is absent rather than empty: Tavily reads `include_domains: []` as a
    /// filter matching nothing on some plans, and an empty key is noise on every request.
    #[test]
    fn unset_filters_are_left_out_of_the_body_entirely() {
        let body = body(&SearchOptions::default());
        assert!(body.get("include_domains").is_none());
        assert!(body.get("exclude_domains").is_none());
        assert!(body.get("time_range").is_none());
        assert_eq!(body["search_depth"], "basic");
    }

    /// The key belongs in the header. Tavily's body-field auth is the deprecated spelling, and a
    /// key in a JSON body is also a key in every log that captures one.
    #[test]
    fn the_key_never_travels_in_the_body() {
        let body = body(&SearchOptions::default());
        assert!(body.get("api_key").is_none(), "{body}");
        assert!(!body.to_string().contains("test-key"), "{body}");
    }
}
