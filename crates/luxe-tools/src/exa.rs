//! Exa — the keyed REST endpoint, for an account that has one.
//!
//! [`crate::exa_mcp`] is the same vendor's keyless MCP surface and the default when nothing is
//! configured; this is what a paid key buys, and the two return the same normalized hits.
//!
//! Two things about Exa's schema are easy to get wrong and silent when you do: the field is
//! `numResults`, not `num_results`, and a result carries no page text unless `contents` asks for
//! it. Getting either wrong still answers 200 — with Exa's own default result count, and with
//! nothing in the snippet.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::web::{SearchBackend, SearchHit, SearchOptions, WebError};

/// Longest highlight payload requested per result, matching the keyless backend's budget.
const HIGHLIGHT_CHARS: u32 = 1_200;

/// Fallback clip when a result carries raw text but no extracted highlight.
const SNIPPET_CHARS: usize = 600;

/// Exa Search API request.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExaRequest {
    query: String,
    num_results: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    include_domains: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    exclude_domains: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_published_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    contents: Option<ExaContents>,
}

/// What Exa should extract from each result page.
#[derive(Debug, Serialize)]
struct ExaContents {
    text: ExaText,
    highlights: ExaHighlights,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExaText {
    max_characters: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExaHighlights {
    num_sentences: u32,
    highlights_per_url: u32,
}

/// Exa Search API response.
#[derive(Debug, Deserialize)]
struct ExaResponse {
    results: Option<Vec<ExaResult>>,
}

#[derive(Debug, Deserialize)]
struct ExaResult {
    title: Option<String>,
    url: Option<String>,
    text: Option<String>,
    #[serde(default)]
    highlights: Vec<String>,
}

impl ExaResult {
    /// The passage to show: the matched highlights when Exa extracted any, else a clip of the page.
    fn snippet(&self) -> String {
        if !self.highlights.is_empty() {
            return self.highlights.join(" … ");
        }
        let text = self.text.clone().unwrap_or_default();
        match text.chars().count() > SNIPPET_CHARS {
            true => text.chars().take(SNIPPET_CHARS).collect::<String>() + "…",
            false => text,
        }
    }
}

/// Exa search backend.
pub struct ExaBackend {
    api_key: String,
    base_url: String,
}

impl ExaBackend {
    /// A backend against Exa's public endpoint.
    pub fn new(api_key: String) -> Self {
        Self { api_key, base_url: "https://api.exa.ai/search".to_string() }
    }

    /// Point the backend at a stand-in endpoint (tests, a proxy).
    pub fn with_base_url(api_key: String, base_url: impl Into<String>) -> Self {
        Self { api_key, base_url: base_url.into() }
    }

    /// The body for one search.
    fn request_body(&self, query: &str, n: usize, options: &SearchOptions) -> ExaRequest {
        ExaRequest {
            query: query.to_string(),
            num_results: n.max(1),
            include_domains: options.include_domains.clone(),
            exclude_domains: options.exclude_domains.clone(),
            start_published_date: options.recency.map(|recency| recency.since()),
            contents: options.include_content.then_some(ExaContents {
                text: ExaText { max_characters: HIGHLIGHT_CHARS },
                highlights: ExaHighlights { num_sentences: 3, highlights_per_url: 2 },
            }),
        }
    }
}

#[async_trait]
impl SearchBackend for ExaBackend {
    async fn search(
        &self,
        query: &str,
        n: usize,
        options: &SearchOptions,
    ) -> Result<Vec<SearchHit>, WebError> {
        let response = reqwest::Client::new()
            .post(&self.base_url)
            .header("Content-Type", "application/json")
            .header("x-api-key", &self.api_key)
            .json(&self.request_body(query, n, options))
            .send()
            .await
            .map_err(|e| WebError::Network(e.to_string()))?;

        let status = response.status();
        let body = response.text().await.map_err(|e| WebError::Network(e.to_string()))?;
        if !status.is_success() {
            return Err(WebError::Http(format!("Exa returned {status}: {}", body.trim())));
        }

        let parsed: ExaResponse =
            serde_json::from_str(&body).map_err(|e| WebError::Parse(e.to_string()))?;
        Ok(parsed
            .results
            .unwrap_or_default()
            .into_iter()
            .map(|result| SearchHit {
                title: result.title.clone().unwrap_or_default(),
                url: result.url.clone().unwrap_or_default(),
                snippet: result.snippet(),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(options: &SearchOptions) -> serde_json::Value {
        let backend = ExaBackend::new("test-key".to_string());
        serde_json::to_value(backend.request_body("tokio", 3, options)).expect("serializes")
    }

    /// Exa's schema is camelCase and it ignores what it does not recognize, so `num_results` was
    /// accepted, dropped, and answered with Exa's own default page size instead of the one asked
    /// for. Nothing about that shows up in the response.
    #[test]
    fn the_result_count_uses_the_field_name_exa_actually_reads() {
        let body = body(&SearchOptions::default());
        assert_eq!(body["numResults"], 3);
        assert!(body.get("num_results").is_none(), "{body}");
    }

    /// A result carries no page text unless `contents` asks, so a backend that never asks returns
    /// bare links — while the tool that called it advertises passages.
    #[test]
    fn content_is_requested_only_when_the_caller_wants_it() {
        let with = body(&SearchOptions { include_content: true, ..Default::default() });
        assert_eq!(with["contents"]["text"]["maxCharacters"], HIGHLIGHT_CHARS);
        assert_eq!(with["contents"]["highlights"]["numSentences"], 3);
        assert!(body(&SearchOptions::default()).get("contents").is_none());
    }

    #[test]
    fn domain_and_recency_filters_use_exas_spellings() {
        let body = body(&SearchOptions {
            include_domains: vec!["docs.rs".to_string()],
            exclude_domains: vec!["reddit.com".to_string()],
            recency: Some(crate::web::Recency::Month),
            ..Default::default()
        });
        assert_eq!(body["includeDomains"][0], "docs.rs");
        assert_eq!(body["excludeDomains"][0], "reddit.com");
        let since = body["startPublishedDate"].as_str().expect("a date");
        assert_eq!(since.len(), 10, "an ISO date, which is what Exa parses: {since}");
    }

    /// Highlights are the matched passage and beat a clip of the page's opening, which is usually
    /// navigation. A page with neither returns nothing rather than a panic.
    #[test]
    fn the_matched_passage_outranks_a_clip_of_the_page() {
        let highlighted = ExaResult {
            title: None,
            url: None,
            text: Some("boilerplate".repeat(200)),
            highlights: vec!["the matched sentence".to_string()],
        };
        assert_eq!(highlighted.snippet(), "the matched sentence");

        let clipped = ExaResult {
            title: None,
            url: None,
            text: Some("x".repeat(SNIPPET_CHARS + 50)),
            highlights: Vec::new(),
        };
        assert_eq!(
            clipped.snippet().chars().count(),
            SNIPPET_CHARS + 1,
            "clipped plus the ellipsis"
        );

        let empty = ExaResult { title: None, url: None, text: None, highlights: Vec::new() };
        assert!(empty.snippet().is_empty());
    }
}
