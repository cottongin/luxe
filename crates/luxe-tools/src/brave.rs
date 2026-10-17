//! Brave Search — a keyed web index, reached over its REST API.
//!
//! Brave has no domain-filter parameter, so `domains` becomes `site:`/`-site:` operators in the
//! query itself, which its index understands. Recency is Brave's own coarse `freshness` code
//! rather than a date. What it cannot do is carry a page's text: `description` is a search snippet
//! of a couple of hundred characters, so [`SearchBackend::honors`] says so and the caller passes
//! that on rather than letting the model treat a snippet as the passage.

use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;

use crate::web::{SearchBackend, SearchHit, SearchOptions, WebError};

/// Most results Brave's API accepts. It answers 422 above this, not a clamped list.
const MAX_COUNT: usize = 20;

/// Brave Search API response.
#[derive(Debug, Deserialize)]
struct BraveResponse {
    web: Option<BraveWeb>,
    /// Present instead of `web` when Brave rejects the request.
    error: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct BraveWeb {
    results: Option<Vec<BraveResult>>,
}

#[derive(Debug, Deserialize)]
struct BraveResult {
    title: Option<String>,
    url: Option<String>,
    description: Option<String>,
}

/// Brave search backend.
pub struct BraveBackend {
    api_key: String,
    base_url: String,
}

impl BraveBackend {
    /// A backend against Brave's public endpoint.
    pub fn new(api_key: String) -> Self {
        Self { api_key, base_url: "https://api.search.brave.com/res/v1/web/search".to_string() }
    }

    /// Point the backend at a stand-in endpoint (tests, a proxy).
    pub fn with_base_url(api_key: String, base_url: impl Into<String>) -> Self {
        Self { api_key, base_url: base_url.into() }
    }

    /// The URL for one search, with every value properly encoded.
    ///
    /// Built through `query_pairs_mut` rather than `format!`, which is not a style preference: an
    /// interpolated query ends at the first `&` and at the first `#`, so `tokio select! &
    /// cancellation` reached Brave as `tokio select!` and `rust #[tokio::main]` as `rust` — a
    /// wrong answer returned with no error at all.
    fn request_url(&self, query: &str, n: usize, options: &SearchOptions) -> String {
        let mut url = match url::Url::parse(&self.base_url) {
            Ok(url) => url,
            Err(_) => return self.base_url.clone(),
        };
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("q", &options.applied_to(query));
            pairs.append_pair("count", &n.clamp(1, MAX_COUNT).to_string());
            if let Some(recency) = options.recency {
                pairs.append_pair("freshness", recency.brave_code());
            }
        }
        url.to_string()
    }
}

#[async_trait]
impl SearchBackend for BraveBackend {
    async fn search(
        &self,
        query: &str,
        n: usize,
        options: &SearchOptions,
    ) -> Result<Vec<SearchHit>, WebError> {
        let response = reqwest::Client::new()
            .get(self.request_url(query, n, options))
            .header("Accept", "application/json")
            .header("X-Subscription-Token", &self.api_key)
            .send()
            .await
            .map_err(|e| WebError::Network(e.to_string()))?;

        let status = response.status();
        let body = response.text().await.map_err(|e| WebError::Network(e.to_string()))?;
        if !status.is_success() {
            return Err(WebError::Http(format!("Brave returned {status}: {}", body.trim())));
        }

        let parsed: BraveResponse =
            serde_json::from_str(&body).map_err(|e| WebError::Parse(e.to_string()))?;
        if let Some(error) = parsed.error {
            return Err(WebError::Backend(format!("Brave: {error}")));
        }

        Ok(parsed
            .web
            .and_then(|web| web.results)
            .unwrap_or_default()
            .into_iter()
            .map(|result| SearchHit {
                title: result.title.unwrap_or_default(),
                url: result.url.unwrap_or_default(),
                snippet: result.description.unwrap_or_default(),
            })
            .collect())
    }

    fn honors(&self, options: &SearchOptions) -> Vec<String> {
        match options.include_content {
            true => {
                vec!["Brave returns a short search snippet per result, not the page's own text — \
                 web_fetch a result for the full page."
                    .to_string()]
            }
            false => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend() -> BraveBackend {
        BraveBackend::new("test-key".to_string())
    }

    /// The reported failure, as the three characters that caused it. Brave answered 200 with
    /// results for a *different* query, so nothing downstream could tell.
    #[test]
    fn a_query_reaches_brave_whole_however_it_is_punctuated() {
        for (query, encoded) in [
            ("tokio select! & cancellation", "%26"),
            ("rust #[tokio::main] macro", "%23"),
            ("a + b in rust", "%2B"),
        ] {
            let url = backend().request_url(query, 5, &SearchOptions::default());
            let parsed = url::Url::parse(&url).expect("a valid URL");
            let got = parsed
                .query_pairs()
                .find(|(key, _)| key == "q")
                .map(|(_, value)| value.into_owned())
                .expect("q is present");
            assert_eq!(got, query, "{url}");
            assert!(url.contains(encoded), "the character must go over the wire escaped: {url}");
        }
    }

    /// Brave answers 422 above 20 rather than returning what it can, so the ceiling is ours to
    /// hold — and a caller asking for one result must not be handed Brave's default page.
    #[test]
    fn the_result_count_is_clamped_to_what_brave_accepts() {
        let count = |n: usize| {
            let url = backend().request_url("rust", n, &SearchOptions::default());
            url::Url::parse(&url)
                .expect("a valid URL")
                .query_pairs()
                .find(|(key, _)| key == "count")
                .map(|(_, value)| value.into_owned())
                .expect("count is present")
        };
        assert_eq!(count(3), "3");
        assert_eq!(count(20), "20");
        assert_eq!(count(50), "20");
        assert_eq!(count(0), "1");
    }

    /// Brave has no domain parameter, so a filter the schema advertises has to become something
    /// its index reads — otherwise it is a request that silently does nothing.
    #[test]
    fn domain_filters_become_site_operators_and_recency_becomes_a_freshness_code() {
        let options = SearchOptions {
            include_domains: vec!["docs.rs".to_string()],
            exclude_domains: vec!["reddit.com".to_string()],
            recency: Some(crate::web::Recency::Week),
            ..Default::default()
        };
        let url = url::Url::parse(&backend().request_url("tokio", 5, &options)).expect("valid");
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(pairs["q"], "tokio site:docs.rs -site:reddit.com");
        assert_eq!(pairs["freshness"], "pw");
    }

    /// A snippet is not a passage, and a model told otherwise stops fetching pages it needs.
    #[test]
    fn brave_admits_it_returns_snippets_rather_than_page_text() {
        let asked = SearchOptions { include_content: true, ..Default::default() };
        assert!(backend().honors(&asked)[0].contains("web_fetch"));
        assert!(backend().honors(&SearchOptions::default()).is_empty());
    }
}
