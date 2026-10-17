//! Exa MCP — web search that works with no account, no key, and no configuration.
//!
//! Exa hosts a public MCP endpoint that answers `tools/call` over JSON-RPC with no credential.
//! That is what makes search a *default capability* rather than a setup step: a user installs
//! Luxe and the model can research immediately. A configured `BRAVE_API_KEY`/`TAVILY_API_KEY`/
//! `EXA_API_KEY` still wins, since an explicit choice outranks the fallback, but having none does
//! not mean having no web search.
//!
//! The endpoint also carries the whole feature set worth having: several query variations in one
//! call, domain include/exclude, a published-after date, and extracted highlights. That last one is
//! the point. Returning titles and snippets forces the model into one `web_fetch` per result — a
//! research question becomes six round-trips. Returning the passages *with* the results answers it
//! in one.

use serde::Deserialize;

use crate::web::{SearchBackend, SearchHit, SearchOptions, WebError};

/// Exa's keyless MCP endpoint.
const MCP_URL: &str = "https://mcp.exa.ai/mcp";

/// The MCP tool that exposes filters, extra queries, and highlight extraction.
const ADVANCED_TOOL: &str = "web_search_advanced_exa";

/// Identifies Luxe to Exa, so the endpoint's operators can see who its traffic is.
const SOURCE_HEADER: &str = "luxe";

/// Longest highlight payload requested per result. Enough to answer from, short enough that ten
/// results stay inside a reasonable slice of the context window.
const HIGHLIGHT_CHARS: u32 = 1_200;

/// Fallback clip when a result has no extracted highlights and only raw page text.
const SNIPPET_CHARS: usize = 600;

/// Search backed by Exa's public MCP endpoint.
pub struct ExaMcpBackend {
    url: String,
}

impl Default for ExaMcpBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl ExaMcpBackend {
    pub fn new() -> Self {
        Self { url: MCP_URL.to_string() }
    }

    /// Point the backend at a stand-in endpoint (tests, a self-hosted proxy).
    pub fn with_url(url: impl Into<String>) -> Self {
        Self { url: url.into() }
    }

    /// Run one search, with options.
    pub async fn search_with(
        &self,
        query: &str,
        n: usize,
        options: &SearchOptions,
    ) -> Result<Vec<SearchHit>, WebError> {
        let arguments = build_arguments(query, n, options);
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": ADVANCED_TOOL, "arguments": arguments },
        });

        let response = reqwest::Client::new()
            .post(format!("{}?tools={ADVANCED_TOOL}", self.url))
            .header("Content-Type", "application/json")
            // The endpoint answers JSON-RPC as a single SSE `message` event, so both are accepted.
            .header("Accept", "application/json, text/event-stream")
            .header("x-exa-source", SOURCE_HEADER)
            .json(&body)
            .send()
            .await
            .map_err(|e| WebError::Network(e.to_string()))?;

        if !response.status().is_success() {
            return Err(WebError::Backend(format!("exa mcp returned {}", response.status())));
        }
        let text = response.text().await.map_err(|e| WebError::Network(e.to_string()))?;
        parse_response(&text, n)
    }
}

#[async_trait::async_trait]
impl SearchBackend for ExaMcpBackend {
    async fn search(
        &self,
        query: &str,
        n: usize,
        options: &SearchOptions,
    ) -> Result<Vec<SearchHit>, WebError> {
        self.search_with(query, n, options).await
    }

    /// Exa's advanced tool takes every phrasing as one call, which is the whole reason it is the
    /// default: one round-trip covers the angles a caller would otherwise pay for separately.
    fn batches_queries(&self) -> bool {
        true
    }
}

/// Map Luxe's options onto Exa's argument names.
fn build_arguments(query: &str, n: usize, options: &SearchOptions) -> serde_json::Value {
    let mut arguments = serde_json::json!({
        "query": query,
        "numResults": n.clamp(1, 25),
        "type": "auto",
    });
    let map = arguments.as_object_mut().expect("object literal");
    if !options.also.is_empty() {
        map.insert("additionalQueries".into(), serde_json::json!(options.also));
    }
    if !options.include_domains.is_empty() {
        map.insert("includeDomains".into(), serde_json::json!(options.include_domains));
    }
    if !options.exclude_domains.is_empty() {
        map.insert("excludeDomains".into(), serde_json::json!(options.exclude_domains));
    }
    if let Some(recency) = options.recency {
        map.insert("startPublishedDate".into(), serde_json::json!(recency.since()));
    }
    if options.include_content {
        map.insert("enableHighlights".into(), serde_json::json!(true));
        map.insert("highlightsMaxCharacters".into(), serde_json::json!(HIGHLIGHT_CHARS));
        map.insert("highlightsQuery".into(), serde_json::json!(query));
    }
    arguments
}

#[derive(Debug, Deserialize)]
struct RpcEnvelope {
    #[serde(default)]
    result: Option<RpcResult>,
    #[serde(default)]
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcResult {
    #[serde(default)]
    content: Vec<RpcContent>,
    #[serde(default, rename = "isError")]
    is_error: bool,
}

#[derive(Debug, Deserialize)]
struct RpcContent {
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    #[serde(default)]
    message: String,
}

/// Pull the JSON-RPC envelope out of a plain or SSE-framed body, then the hits out of that.
fn parse_response(body: &str, n: usize) -> Result<Vec<SearchHit>, WebError> {
    let envelope = json_payload(body)
        .ok_or_else(|| WebError::Backend("exa mcp returned no JSON payload".to_string()))?;
    let envelope: RpcEnvelope = serde_json::from_str(&envelope)
        .map_err(|e| WebError::Backend(format!("exa mcp response not understood: {e}")))?;
    if let Some(error) = envelope.error {
        return Err(WebError::Backend(format!("exa mcp: {}", error.message)));
    }
    let result = envelope
        .result
        .ok_or_else(|| WebError::Backend("exa mcp returned no result".to_string()))?;
    if result.is_error {
        let detail = result.content.first().map(|c| c.text.as_str()).unwrap_or("unknown error");
        return Err(WebError::Backend(format!("exa mcp: {detail}")));
    }
    let joined = result.content.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join("\n");
    // The advanced tool answers with a JSON envelope; the basic one answers with prose. Try the
    // structured shape first and fall back, so either tool parses.
    let hits = parse_json_hits(&joined).unwrap_or_else(|| parse_hits(&joined));
    Ok(hits.into_iter().take(n).collect())
}

#[derive(Debug, Deserialize)]
struct ExaResults {
    results: Vec<ExaResult>,
}

#[derive(Debug, Deserialize)]
struct ExaResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    highlights: Vec<String>,
}

/// Parse the advanced tool's JSON envelope, or `None` when the payload is not that shape.
///
/// Highlights are the passages that answer the query, so they lead. Falling back to the head of the
/// page text keeps a result useful when highlight extraction returns nothing.
fn parse_json_hits(text: &str) -> Option<Vec<SearchHit>> {
    let parsed: ExaResults = serde_json::from_str(text.trim()).ok()?;
    Some(
        parsed
            .results
            .into_iter()
            .filter(|result| !result.url.is_empty())
            .map(|result| {
                let snippet = if result.highlights.is_empty() {
                    collapse(&result.text, SNIPPET_CHARS)
                } else {
                    collapse(&result.highlights.join(" … "), HIGHLIGHT_CHARS as usize)
                };
                SearchHit { title: result.title, url: result.url, snippet }
            })
            .collect(),
    )
}

/// Squash whitespace and clip to `max` characters — page text arrives with heavy newline padding.
fn collapse(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &flat[..at]),
        None => flat,
    }
}

/// The JSON object in an SSE-framed (`data: {...}`) or bare body.
fn json_payload(body: &str) -> Option<String> {
    for line in body.lines() {
        let candidate = line.strip_prefix("data:").unwrap_or(line).trim();
        if candidate.starts_with('{') {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Split Exa's `Title:` / `URL:` / `Highlights:` text blocks into normalized hits.
///
/// The endpoint returns model-readable prose rather than structured results, so this is a parse of
/// a documented shape: a `Title:` line opens each record and `URL:` identifies it. Anything else
/// under a record becomes its snippet, which is where the highlights land.
fn parse_hits(text: &str) -> Vec<SearchHit> {
    let mut hits: Vec<SearchHit> = Vec::new();
    let mut snippet = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(title) = trimmed.strip_prefix("Title:") {
            flush(&mut hits, &mut snippet);
            hits.push(SearchHit {
                title: title.trim().to_string(),
                url: String::new(),
                snippet: String::new(),
            });
        } else if let Some(url) = trimmed.strip_prefix("URL:") {
            if let Some(last) = hits.last_mut() {
                last.url = url.trim().to_string();
            }
        } else if !trimmed.is_empty()
            && !trimmed.starts_with("Published:")
            && !trimmed.starts_with("Author:")
            && trimmed != "Highlights:"
            && trimmed != "..."
        {
            if !snippet.is_empty() {
                snippet.push(' ');
            }
            snippet.push_str(trimmed);
        }
    }
    flush(&mut hits, &mut snippet);
    hits.retain(|hit| !hit.url.is_empty());
    hits
}

fn flush(hits: &mut [SearchHit], snippet: &mut String) {
    if let Some(last) = hits.last_mut() {
        last.snippet = std::mem::take(snippet);
    } else {
        snippet.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact shape the live endpoint returned during development.
    const SAMPLE: &str = "event: message\n\
        data: {\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"Title: select in tokio - Rust\\nURL: https://docs.rs/tokio/latest/tokio/macro.select.html\\nPublished: N/A\\nAuthor: N/A\\nHighlights:\\n## Cancellation safety\\n...\\nCancellation safety describes what happens when a future is dropped.\\nTitle: Second Result\\nURL: https://example.com/two\\nHighlights:\\nSomething else entirely.\"}]}}\n";

    #[test]
    fn an_sse_framed_response_parses_into_hits_with_content() {
        let hits = parse_response(SAMPLE, 10).expect("parses");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "select in tokio - Rust");
        assert_eq!(hits[0].url, "https://docs.rs/tokio/latest/tokio/macro.select.html");
        assert!(
            hits[0].snippet.contains("Cancellation safety describes"),
            "the extracted passage rides along, so no follow-up fetch is needed: {:?}",
            hits[0].snippet
        );
        assert!(!hits[0].snippet.contains("N/A"), "metadata noise is dropped");
        assert_eq!(hits[1].url, "https://example.com/two");
    }

    #[test]
    fn the_result_count_is_honored() {
        assert_eq!(parse_response(SAMPLE, 1).expect("parses").len(), 1);
    }

    #[test]
    fn a_bare_json_body_parses_too() {
        let bare =
            r#"{"result":{"content":[{"type":"text","text":"Title: A\nURL: https://a.test"}]}}"#;
        assert_eq!(parse_response(bare, 5).expect("parses")[0].url, "https://a.test");
    }

    #[test]
    fn an_rpc_error_surfaces_instead_of_an_empty_result() {
        let body = r#"{"error":{"code":-32602,"message":"bad arguments"}}"#;
        let error = parse_response(body, 5).expect_err("must not look like zero results");
        assert!(error.to_string().contains("bad arguments"), "{error}");
    }

    #[test]
    fn a_tool_level_error_surfaces_too() {
        let body =
            r#"{"result":{"isError":true,"content":[{"type":"text","text":"rate limited"}]}}"#;
        let error = parse_response(body, 5).expect_err("isError must not be silently empty");
        assert!(error.to_string().contains("rate limited"), "{error}");
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(parse_response("not json at all", 5).is_err());
        assert!(parse_response("", 5).is_err());
    }

    #[test]
    fn options_map_onto_exas_argument_names() {
        let options = SearchOptions {
            also: vec!["second phrasing".into()],
            include_domains: vec!["arxiv.org".into()],
            exclude_domains: vec!["spam.example".into()],
            recency: Some(crate::web::Recency::Month),
            include_content: true,
        };
        let args = build_arguments("tokio cancellation", 7, &options);

        assert_eq!(args["query"], "tokio cancellation");
        assert_eq!(args["numResults"], 7);
        assert_eq!(args["additionalQueries"][0], "second phrasing");
        assert_eq!(args["includeDomains"][0], "arxiv.org");
        assert_eq!(args["excludeDomains"][0], "spam.example");
        assert_eq!(args["startPublishedDate"].as_str().expect("a date").len(), 10);
        assert_eq!(args["enableHighlights"], true);
    }

    #[test]
    fn defaults_stay_off_the_wire_and_the_count_is_clamped() {
        let args = build_arguments("q", 999, &SearchOptions::default());
        assert_eq!(args["numResults"], 25, "clamped to Exa's practical ceiling");
        for absent in [
            "additionalQueries",
            "includeDomains",
            "excludeDomains",
            "startPublishedDate",
            "enableHighlights",
        ] {
            assert!(args.get(absent).is_none(), "{absent} must not be sent by default");
        }
    }

    /// The advanced tool answers with a JSON envelope, not the basic tool's prose. Parsing only the
    /// prose form meant every advanced search reported "No results" while the endpoint was
    /// returning perfectly good ones.
    #[test]
    fn the_advanced_tools_json_envelope_parses() {
        let inner = serde_json::json!({
            "requestId": "abc",
            "results": [
                {
                    "id": "https://tokio.rs/x",
                    "url": "https://tokio.rs/x",
                    "title": "Select | Tokio",
                    "text": "Select | Tokio\n\n# Select\n\nSo far...",
                    "highlights": ["Futures can implement Drop to clean up.", "oneshot is cancel safe."],
                    "publishedDate": "2026-03-18T00:00:00.000Z"
                },
                { "url": "", "title": "dropped: no url" }
            ]
        })
        .to_string();
        let body = serde_json::json!({
            "result": { "content": [{ "type": "text", "text": inner }] }
        })
        .to_string();

        let hits = parse_response(&body, 10).expect("parses");
        assert_eq!(hits.len(), 1, "a result with no url is not a result");
        assert_eq!(hits[0].title, "Select | Tokio");
        assert_eq!(hits[0].url, "https://tokio.rs/x");
        assert!(hits[0].snippet.contains("cancel safe"), "highlights lead: {:?}", hits[0].snippet);
        assert!(!hits[0].snippet.contains('\n'), "whitespace is collapsed");
    }

    #[test]
    fn a_result_without_highlights_falls_back_to_its_text() {
        let inner = serde_json::json!({
            "results": [{ "url": "https://a.test", "title": "A", "text": "  body   text here  " }]
        })
        .to_string();
        let body = serde_json::json!({ "result": { "content": [{ "text": inner }] } }).to_string();
        let hits = parse_response(&body, 5).expect("parses");
        assert_eq!(hits[0].snippet, "body text here");
    }

    #[test]
    fn a_long_snippet_is_clipped() {
        let long = "word ".repeat(400);
        let inner = serde_json::json!({
            "results": [{ "url": "https://a.test", "title": "A", "text": long }]
        })
        .to_string();
        let body = serde_json::json!({ "result": { "content": [{ "text": inner }] } }).to_string();
        let hits = parse_response(&body, 5).expect("parses");
        assert!(hits[0].snippet.chars().count() <= SNIPPET_CHARS + 1, "clipped");
        assert!(hits[0].snippet.ends_with('…'));
    }
}
