//! Search — the backends a `web_search` can run on, and the tool itself.

use super::render::{iso_date, render_hits};
use super::*;

/// A search backend (Brave, Tavily, Exa, Firecrawl, etc.).
///
/// Every backend is handed the whole [`SearchOptions`] set, because the tool schema advertises
/// those filters to the model on every request: a backend that quietly drops them answers a
/// different question from the one that was asked, and returns plausible results while doing it.
/// Where a vendor spells a filter differently — Brave has no domain parameter and takes `site:`
/// operators instead — the backend translates. Where it genuinely cannot, [`Self::honors`] says so
/// and the caller tells the model rather than letting it believe the filter applied.
#[async_trait]
pub trait SearchBackend: Send + Sync {
    /// Run one search, shaped by everything the caller asked for.
    async fn search(
        &self,
        query: &str,
        n: usize,
        options: &SearchOptions,
    ) -> Result<Vec<SearchHit>, WebError>;

    /// What this backend does with the requested options, for the caller to report.
    fn honors(&self, _options: &SearchOptions) -> Vec<String> {
        Vec::new()
    }

    /// Whether a search's phrasings must run one after another. A rate-limited endpoint turns a
    /// burst away, so its phrasings wait their turn instead of failing together.
    fn one_phrasing_at_a_time(&self) -> bool {
        false
    }
}

/// How recent a result has to be — the four windows `web_search` offers, in each vendor's terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recency {
    Day,
    Week,
    Month,
    Year,
}

impl Recency {
    /// Parse the tool schema's spelling.
    pub fn parse(window: &str) -> Option<Self> {
        match window {
            "day" => Some(Self::Day),
            "week" => Some(Self::Week),
            "month" => Some(Self::Month),
            "year" => Some(Self::Year),
            _ => None,
        }
    }

    /// The window as a count of days.
    pub fn days(self) -> i64 {
        match self {
            Self::Day => 1,
            Self::Week => 7,
            Self::Month => 31,
            Self::Year => 365,
        }
    }

    /// The earliest publication date still inside the window, as `YYYY-MM-DD`.
    pub fn since(self) -> String {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default()
            .saturating_sub((self.days() * 86_400) as u64);
        iso_date(secs)
    }

    /// Brave's `freshness` code.
    pub fn brave_code(self) -> &'static str {
        match self {
            Self::Day => "pd",
            Self::Week => "pw",
            Self::Month => "pm",
            Self::Year => "py",
        }
    }

    /// Tavily's `time_range`.
    pub fn tavily_range(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
        }
    }

    /// The Google `tbs` window Firecrawl passes upstream.
    pub fn tbs(self) -> &'static str {
        match self {
            Self::Day => "qdr:d",
            Self::Week => "qdr:w",
            Self::Month => "qdr:m",
            Self::Year => "qdr:y",
        }
    }
}

/// What a caller can ask of a search, beyond the query itself.
///
/// Deliberately a small, provider-neutral set: the options that change *which* results come back
/// and whether their content rides along. Exa's own schema has thirty knobs; a tool schema that
/// exposed all of them would cost more context than the answers it fetches.
#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    /// Only these domains.
    pub include_domains: Vec<String>,
    /// Never these domains.
    pub exclude_domains: Vec<String>,
    /// Published no earlier than this window.
    pub recency: Option<Recency>,
    /// Carry extracted passages with each result instead of a bare snippet.
    pub include_content: bool,
}

impl SearchOptions {
    /// `site:`/`-site:` operators for a backend whose API has no domain filter of its own.
    ///
    /// Every general web index understands them, which is why a missing parameter is not a missing
    /// capability.
    pub fn site_operators(&self) -> String {
        let include = self.include_domains.iter().map(|domain| format!("site:{domain}"));
        let exclude = self.exclude_domains.iter().map(|domain| format!("-site:{domain}"));
        include.chain(exclude).collect::<Vec<_>>().join(" ")
    }

    /// `query` with those operators appended, which is how such a backend is asked.
    pub fn applied_to(&self, query: &str) -> String {
        let operators = self.site_operators();
        match operators.is_empty() {
            true => query.to_string(),
            false => format!("{query} {operators}"),
        }
    }
}

/// Normalized search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Create a search backend based on configuration.
///
/// No key means Exa's keyless MCP endpoint rather than an error — see [`WebConfig::from_env`].
pub async fn create_backend(config: &WebConfig) -> Result<Box<dyn SearchBackend>, WebError> {
    let Some(api_key) = config.search_api_key.as_deref() else {
        return Ok(Box::new(crate::exa_mcp::ExaMcpBackend::new()));
    };
    match config.search_backend.as_deref().unwrap_or("exa_mcp") {
        "brave" => Ok(Box::new(crate::brave::BraveBackend::new(api_key.to_string()))),
        "tavily" => Ok(Box::new(crate::tavily::TavilyBackend::new(api_key.to_string()))),
        "exa" => Ok(Box::new(crate::exa::ExaBackend::new(api_key.to_string()))),
        "firecrawl" => Ok(Box::new(crate::firecrawl::FirecrawlBackend::new(api_key.to_string()))),
        "exa_mcp" => Ok(Box::new(crate::exa_mcp::ExaMcpBackend::new())),
        other => Err(WebError::Backend(format!("Unknown search backend: {other}"))),
    }
}

/// What a search found, and a note for each phrasing that failed.
#[derive(Debug)]
pub(super) struct Searched {
    pub(super) hits: Vec<SearchHit>,
    pub(super) skipped: Vec<String>,
}

/// Run every phrasing the caller asked for and merge the results.
///
/// Each phrasing is its own search, so each gets its own ranking and its own passages; one call
/// carrying the extras ranks everything against the first. A phrasing that errors is skipped and
/// named rather than sinking the whole search; the error only fails it when *nothing* answered.
pub(super) async fn run_search(
    backend: &dyn SearchBackend,
    queries: &[String],
    n: usize,
    options: &SearchOptions,
) -> Result<Searched, WebError> {
    let mut lists = Vec::new();
    let mut skipped = Vec::new();
    let mut last_error = None;
    for (query, answer) in queries.iter().zip(search_each(backend, queries, n, options).await) {
        match answer {
            Ok(hits) => lists.push(hits),
            Err(error) => {
                skipped.push(format!("\"{query}\" wasn't searched: {error}"));
                last_error = Some(error);
            }
        }
    }
    let hits = interleave(lists, n);
    match (hits.is_empty(), last_error) {
        (true, Some(error)) => Err(error),
        _ => Ok(Searched { hits, skipped }),
    }
}

/// Each phrasing's answer, in order: all at once, or in turn where the backend needs that.
async fn search_each(
    backend: &dyn SearchBackend,
    queries: &[String],
    n: usize,
    options: &SearchOptions,
) -> Vec<Result<Vec<SearchHit>, WebError>> {
    if !backend.one_phrasing_at_a_time() {
        let searches = queries.iter().map(|query| backend.search(query, n, options));
        return futures::future::join_all(searches).await;
    }
    let mut answers = Vec::with_capacity(queries.len());
    for query in queries {
        answers.push(backend.search(query, n, options).await);
    }
    answers
}

/// Up to `n` hits, rank by rank: the top of each list, then the next of each.
///
/// So every phrasing is represented. A page two phrasings found is one result, at its first
/// position, and its second slot goes to the next page.
fn interleave(lists: Vec<Vec<SearchHit>>, n: usize) -> Vec<SearchHit> {
    let mut lists: Vec<_> = lists.into_iter().map(Vec::into_iter).collect();
    let mut merged: Vec<SearchHit> = Vec::new();
    while merged.len() < n {
        let rank: Vec<SearchHit> = lists.iter_mut().filter_map(Iterator::next).collect();
        if rank.is_empty() {
            break;
        }
        for hit in rank {
            if merged.len() < n && !merged.iter().any(|seen| seen.url == hit.url) {
                merged.push(hit);
            }
        }
    }
    merged
}

/// web_search tool — pluggable search backend.
pub struct WebSearchTool {
    pub(super) config: Arc<RwLock<WebConfig>>,
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    /// A search backend that has stopped answering is not worth a two-minute wait.
    fn timeout(&self) -> Option<std::time::Duration> {
        Some(luxe_core::tool::NETWORK_TOOL_TIMEOUT)
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "web_search".to_string(),
            description: "Search the web. Each result carries the passage the search engine \
                          matched, which often answers the question outright — `web_fetch` a URL \
                          only when you need more of the page than came back. Pass several \
                          `queries` to cover a topic from different angles in one call (varied \
                          phrasing beats repeating one query). Narrow with `domains` (prefix `-` \
                          to exclude) and `recency` when currency matters; a reply that leads with \
                          `Note:` is saying which of those the configured search vendor could not \
                          apply."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query"},
                    "queries": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Two to four varied phrasings of the same question, searched together. Prefer this over one query for research."
                    },
                    "n": {"type": "integer", "description": "Results to return (default 5, max 20)"},
                    "domains": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Limit to these domains, e.g. ['docs.rs']; prefix with '-' to exclude, e.g. '-pinterest.com'."
                    },
                    "recency": {
                        "type": "string",
                        "enum": ["day", "week", "month", "year"],
                        "description": "Only pages published within this window."
                    },
                    "content": {
                        "type": "boolean",
                        "description": "Include the relevant passage from each result (default true). Set false for a bare link list."
                    }
                },
                "required": []
            }),
        }
    }

    async fn run(&self, args: serde_json::Value, _cx: &ToolContext) -> ToolResultData {
        let queries = search_queries(&args);
        if queries.is_empty() {
            return ToolResultData::error("Error: 'query' (or 'queries') is required");
        }

        let n = (args.get("n").and_then(serde_json::Value::as_u64).unwrap_or(5) as usize)
            .clamp(1, MAX_RESULTS);
        let options = SearchOptions {
            include_content: args
                .get("content")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
            recency: args
                .get("recency")
                .and_then(serde_json::Value::as_str)
                .and_then(Recency::parse),
            ..domain_filters(&args)
        };

        let config = self.config.read().await;
        let backend = match create_backend(&config).await {
            Ok(backend) => backend,
            Err(e) => return ToolResultData::error(format!("Backend error: {e}")),
        };
        match run_search(backend.as_ref(), &queries, n, &options).await {
            Ok(searched) => {
                let mut caveats = backend.honors(&options);
                caveats.extend(searched.skipped);
                ToolResultData::text(render_hits(&queries, &searched.hits, &caveats))
            }
            Err(e) => ToolResultData::error(format!("Search failed: {e}")),
        }
    }
}

/// Most results any backend will return for one query. Brave refuses more than 20 outright, so the
/// ceiling is the lowest that every vendor accepts rather than the highest one of them allows.
pub(super) const MAX_RESULTS: usize = 20;

/// The queries to run: `queries` when given, else the single `query`, trimmed and de-duplicated.
///
/// Owned by `luxe_core` because the tool card lists the same set under the call's header, and a
/// second implementation would let the card and the search disagree about what ran.
pub(super) fn search_queries(args: &serde_json::Value) -> Vec<String> {
    luxe_core::web_search_queries(args)
}

/// Split `domains` into include/exclude sets; a leading `-` excludes.
pub(super) fn domain_filters(args: &serde_json::Value) -> SearchOptions {
    let mut options = SearchOptions::default();
    let Some(list) = args.get("domains").and_then(serde_json::Value::as_array) else {
        return options;
    };
    for entry in list.iter().filter_map(serde_json::Value::as_str) {
        let entry = entry.trim();
        // A bare `-` names no domain either way.
        if entry.is_empty() || entry == "-" {
            continue;
        }
        match entry.strip_prefix('-') {
            Some(excluded) => options.exclude_domains.push(excluded.to_string()),
            None => options.include_domains.push(entry.to_string()),
        }
    }
    options
}

#[cfg(test)]
mod tests;
