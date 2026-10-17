//! Firecrawl — a search index and, more usefully here, a renderer of pages that refuse to be read.
//!
//! It earns a place for the second of those. `web_fetch` already retries a refusal behind a
//! browser user-agent, which gets past a check on the header and nothing more; a page that builds
//! itself in JavaScript, or sits behind a real bot check, comes back as an empty shell or a
//! challenge. Firecrawl runs an actual browser and returns the rendered article as markdown, so it
//! is the last thing `web_fetch` tries before giving up — and only when a key says it may.
//!
//! Search is the same option set as everyone else's, with domain filters and a `tbs` recency
//! window that Firecrawl passes upstream.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::web::{SearchBackend, SearchHit, SearchOptions, WebError};

/// Firecrawl's API root, without the endpoint segment.
const API_ROOT: &str = "https://api.firecrawl.dev/v2";

/// How long Firecrawl may spend rendering one page, in milliseconds. Under `web_fetch`'s own
/// network bound, so the tool reports a Firecrawl timeout rather than being killed mid-retry.
const SCRAPE_TIMEOUT_MS: u32 = 45_000;

/// Firecrawl search request.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchRequest {
    query: String,
    limit: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    include_domains: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    exclude_domains: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tbs: Option<&'static str>,
}

/// Firecrawl wraps every payload in `{success, data}`.
#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    data: Option<SearchData>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchData {
    #[serde(default)]
    web: Vec<WebResult>,
}

#[derive(Debug, Deserialize)]
struct WebResult {
    title: Option<String>,
    url: Option<String>,
    description: Option<String>,
}

/// Firecrawl scrape request — markdown of the article, not the chrome around it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScrapeRequest<'a> {
    url: &'a str,
    formats: [&'static str; 1],
    only_main_content: bool,
    timeout: u32,
}

#[derive(Debug, Deserialize)]
struct ScrapeResponse {
    #[serde(default)]
    data: Option<ScrapeData>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ScrapeData {
    #[serde(default)]
    markdown: Option<String>,
    #[serde(default)]
    metadata: Option<ScrapeMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScrapeMetadata {
    #[serde(default)]
    title: Option<String>,
    /// What the *origin* answered. Firecrawl reports its own call as 200 and puts the upstream
    /// status here, so a 404 arrives looking like a success with a stub page attached.
    #[serde(default)]
    status_code: Option<u16>,
}

/// Firecrawl backend: web search, and a browser for pages that need one.
pub struct FirecrawlBackend {
    api_key: String,
    api_root: String,
}

impl FirecrawlBackend {
    /// A backend against Firecrawl's public API.
    pub fn new(api_key: String) -> Self {
        Self { api_key, api_root: API_ROOT.to_string() }
    }

    /// Point the backend at a stand-in endpoint (tests, a self-hosted Firecrawl).
    pub fn with_api_root(api_key: String, api_root: impl Into<String>) -> Self {
        Self { api_key, api_root: api_root.into() }
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}/{path}", self.api_root.trim_end_matches('/'))
    }

    /// The body for one search.
    fn search_body(&self, query: &str, n: usize, options: &SearchOptions) -> SearchRequest {
        SearchRequest {
            query: query.to_string(),
            limit: n.max(1),
            include_domains: options.include_domains.clone(),
            exclude_domains: options.exclude_domains.clone(),
            tbs: options.recency.map(|recency| recency.tbs()),
        }
    }

    /// Render `url` in a real browser and return the article as markdown.
    ///
    /// An upstream 404 or 403 is an error here even though Firecrawl answered 200: the caller is
    /// deciding whether the page was readable, and a stub saying "Not Found" would otherwise be
    /// reported as the page's content.
    pub async fn scrape(&self, url: &str) -> Result<String, WebError> {
        let body = ScrapeRequest {
            url,
            formats: ["markdown"],
            only_main_content: true,
            timeout: SCRAPE_TIMEOUT_MS,
        };
        let response = reqwest::Client::new()
            .post(self.endpoint("scrape"))
            .header("Content-Type", "application/json")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| WebError::Network(e.to_string()))?;

        let parsed: ScrapeResponse = decode(response, "scrape").await?;
        if let Some(error) = parsed.error {
            return Err(WebError::Backend(format!("Firecrawl: {error}")));
        }
        let data =
            parsed.data.ok_or_else(|| WebError::Parse("Firecrawl returned no page".into()))?;
        let upstream = data.metadata.as_ref().and_then(|meta| meta.status_code).unwrap_or(200);
        if !(200..300).contains(&upstream) {
            return Err(WebError::Http(format!("the page itself answered {upstream}")));
        }
        let markdown = data.markdown.unwrap_or_default();
        if markdown.trim().is_empty() {
            return Err(WebError::Parse("Firecrawl rendered the page but found no text".into()));
        }
        Ok(
            match data.metadata.and_then(|meta| meta.title).filter(|title| !title.trim().is_empty())
            {
                Some(title) => format!("# {}\n\n{markdown}", title.trim()),
                None => markdown,
            },
        )
    }
}

/// Read a Firecrawl response, turning a non-2xx into an error that names what it was doing.
async fn decode<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    what: &str,
) -> Result<T, WebError> {
    let status = response.status();
    let body = response.text().await.map_err(|e| WebError::Network(e.to_string()))?;
    if !status.is_success() {
        return Err(WebError::Http(format!("Firecrawl {what} returned {status}: {}", body.trim())));
    }
    serde_json::from_str(&body).map_err(|e| WebError::Parse(e.to_string()))
}

#[async_trait]
impl SearchBackend for FirecrawlBackend {
    async fn search(
        &self,
        query: &str,
        n: usize,
        options: &SearchOptions,
    ) -> Result<Vec<SearchHit>, WebError> {
        let response = reqwest::Client::new()
            .post(self.endpoint("search"))
            .header("Content-Type", "application/json")
            .bearer_auth(&self.api_key)
            .json(&self.search_body(query, n, options))
            .send()
            .await
            .map_err(|e| WebError::Network(e.to_string()))?;

        let parsed: SearchResponse = decode(response, "search").await?;
        if let Some(error) = parsed.error {
            return Err(WebError::Backend(format!("Firecrawl: {error}")));
        }
        Ok(parsed
            .data
            .map(|data| data.web)
            .unwrap_or_default()
            .into_iter()
            .map(|result| SearchHit {
                title: result.title.unwrap_or_default(),
                url: result.url.unwrap_or_default(),
                snippet: result.description.unwrap_or_default(),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend() -> FirecrawlBackend {
        FirecrawlBackend::new("fc-test".to_string())
    }

    fn body(options: &SearchOptions) -> serde_json::Value {
        serde_json::to_value(backend().search_body("tokio", 3, options)).expect("serializes")
    }

    #[test]
    fn the_advertised_filters_reach_the_request_in_firecrawls_spellings() {
        let body = body(&SearchOptions {
            include_domains: vec!["docs.rs".to_string()],
            exclude_domains: vec!["reddit.com".to_string()],
            recency: Some(crate::web::Recency::Week),
            ..Default::default()
        });
        assert_eq!(body["query"], "tokio");
        assert_eq!(body["limit"], 3);
        assert_eq!(body["includeDomains"][0], "docs.rs");
        assert_eq!(body["excludeDomains"][0], "reddit.com");
        assert_eq!(body["tbs"], "qdr:w");
    }

    #[test]
    fn unset_filters_are_left_out_of_the_body() {
        let body = body(&SearchOptions::default());
        assert!(body.get("includeDomains").is_none());
        assert!(body.get("excludeDomains").is_none());
        assert!(body.get("tbs").is_none());
    }

    /// Firecrawl's payload is camelCase throughout, and a field name that misses reads as absent
    /// rather than as an error — which turned an upstream 404 back into a page.
    #[test]
    fn the_upstream_status_is_read_off_the_field_firecrawl_actually_sends() {
        let metadata: ScrapeMetadata =
            serde_json::from_str(r#"{"title":"Not Found","statusCode":404}"#).expect("parses");
        assert_eq!(metadata.status_code, Some(404));
        assert_eq!(metadata.title.as_deref(), Some("Not Found"));
    }

    /// A self-hosted Firecrawl is reached by root, and the two endpoints hang off it.
    #[test]
    fn endpoints_hang_off_a_configurable_root() {
        let hosted = FirecrawlBackend::with_api_root("k".into(), "http://localhost:3002/v2/");
        assert_eq!(hosted.endpoint("search"), "http://localhost:3002/v2/search");
        assert_eq!(backend().endpoint("scrape"), "https://api.firecrawl.dev/v2/scrape");
    }
}
