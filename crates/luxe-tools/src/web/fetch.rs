//! Fetch — a page as readable markdown, with a stealth retry and a clone for a GitHub repository.

use super::*;

/// Default polite user agent.
pub(super) const UA_DEFAULT: &str = concat!("Luxe/", env!("CARGO_PKG_VERSION"));
/// Browser-like user agent for the bot-protection-bypass fallback.
pub(super) const UA_STEALTH: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// Whether `web_fetch` may read from the machine it runs on.
///
/// It may. Luxe ships an unrestricted `bash`, so `curl http://127.0.0.1:4321` is already a tool call
/// away — refusing the same origin here protects nothing and costs the ordinary case of reading your
/// own dev server. The boundary that *is* real (private LAN, cloud metadata) is unaffected; see
/// [`crate::ssrf`].
pub(super) const LOCAL: crate::ssrf::LocalAccess = crate::ssrf::LocalAccess::Allowed;

/// Fetch a URL (SSRF-guarded) and reduce it to readable markdown.
pub(super) async fn fetch_url(url: &str) -> Result<String, WebError> {
    let fetched = crate::ssrf::safe_get(url, UA_DEFAULT, LOCAL).await?;
    Ok(fetched.reduced(|html| extract_readable(html, url)))
}

/// Same, with a browser-like user agent (for bot-protected pages).
pub(super) async fn fetch_url_stealth(url: &str) -> Result<String, WebError> {
    let fetched = crate::ssrf::safe_get(url, UA_STEALTH, LOCAL).await?;
    Ok(fetched.reduced(|html| extract_readable(html, url)))
}

/// Reduce an HTML page to readable markdown: extract the main article with a readability pass, then
/// convert that to Markdown. Falls back to whole-page conversion when readability finds nothing.
pub(super) fn extract_readable(html: &str, url: &str) -> String {
    if let Some((title, content_html)) = readability(html, url) {
        let body = htmd::convert(&content_html).unwrap_or(content_html);
        let body = body.trim();
        if body.is_empty() {
            fallback_markdown(html)
        } else if title.trim().is_empty() {
            body.to_string()
        } else {
            format!("# {}\n\n{body}", title.trim())
        }
    } else {
        fallback_markdown(html)
    }
}

/// Run the readability pass, returning `(title, main_content_html)` when it succeeds.
pub(super) fn readability(html: &str, url: &str) -> Option<(String, String)> {
    let mut readability = dom_smoothie::Readability::new(html, Some(url), None).ok()?;
    let article = readability.parse().ok()?;
    Some((article.title.to_string(), article.content.to_string()))
}

/// Whole-page HTML→Markdown, or the raw HTML if even that fails.
pub(super) fn fallback_markdown(html: &str) -> String {
    htmd::convert(html).unwrap_or_else(|_| html.to_string())
}

/// `(owner, repo)` for a GitHub repo-root URL (`github.com/owner/repo`), else `None`. Deeper paths
/// (`/blob/...`, `/tree/...`, `/issues`, ...) are not repo roots.
pub(super) fn github_repo(url: &Url) -> Option<(String, String)> {
    if url.host_str()? != "github.com" {
        return None;
    }
    let segs: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    (segs.len() == 2).then(|| (segs[0].to_string(), segs[1].trim_end_matches(".git").to_string()))
}

/// Map a GitHub blob URL (`github.com/owner/repo/blob/<ref>/<path>`) to its raw content URL.
pub(super) fn github_blob_raw(url_str: &str) -> Option<String> {
    let url = Url::parse(url_str).ok()?;
    if url.host_str()? != "github.com" {
        return None;
    }
    let segs: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    if segs.len() >= 5 && segs[2] == "blob" {
        let rest = segs[3..].join("/");
        Some(format!("https://raw.githubusercontent.com/{}/{}/{}", segs[0], segs[1], rest))
    } else {
        None
    }
}

/// web_fetch tool — fetch URL and extract readable content.
pub struct WebFetchTool {
    pub(super) config: Arc<RwLock<WebConfig>>,
    pub(super) clone_repositories: bool,
}

/// The tool description, which mentions cloning only where it is on.
pub(super) fn web_fetch_description(clone_repositories: bool) -> String {
    let github = if clone_repositories {
        "A GitHub repo URL is shallow-cloned into the project's `.luxe/gh-cache/` and its path \
         returned so you can read/grep the real files; a GitHub blob URL fetches the raw file."
    } else {
        "A GitHub blob URL fetches the raw file."
    };
    format!(
        "Fetch a URL and extract the main content as Markdown (readability). {github} Long pages \
         come back one window at a time — the reply names the exact `offset` to pass for the next \
         one. Reaches public sites and this machine (localhost/127.0.0.1/::1); refuses \
         private-network hosts and cloud-metadata endpoints. Retries bot-protected pages behind a \
         browser user-agent, then in a real browser where one is configured."
    )
}

/// Characters returned per fetched page.
///
/// Sized from a real measurement rather than a guess: the English Wikipedia article on Rust comes
/// back as ~187,000 characters, roughly 47,000 tokens — close to a quarter of a 200K window spent
/// on a single call.
pub(super) const FETCH_MAX_CHARS: usize = 50_000;

/// Return one window of a fetched page, and say exactly how to get the next one.
///
/// A cap alone would be a trap. Plenty of pages — an RFC, a changelog, a single-page API reference
/// — have no anchors and no sub-pages, so "fetch something more specific" is advice that cannot be
/// followed and the remainder is simply unreachable. Paging by character offset always exists, and
/// naming the *exact* next offset means the caller never has to work one out.
pub(super) fn page_of(content: &str, offset: usize) -> String {
    let total = content.chars().count();
    if offset >= total && offset > 0 {
        return format!(
            "Offset {offset} is past the end of this page ({total} characters). \
             Re-fetch with offset 0 to start over."
        );
    }
    let window: String = content.chars().skip(offset).take(FETCH_MAX_CHARS).collect();
    let end = offset + window.chars().count();
    if end >= total {
        return if offset > 0 {
            format!("[characters {offset}–{end} of {total} — end of page]\n{window}")
        } else {
            window
        };
    }
    format!(
        "[characters {offset}–{end} of {total}]\n{window}\n\n… {} characters remain. \
         Continue with the same url and offset: {end}",
        total - end
    )
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    /// A fetch is a network round-trip, so it takes the network bound rather than the generous
    /// default. This backs the per-request timeouts in `crate::ssrf` instead of duplicating them:
    /// those bound each hop, and a redirect chain plus the stealth retry still adds up to minutes —
    /// which is exactly what a host that accepts the connection and then says nothing costs.
    fn timeout(&self) -> Option<std::time::Duration> {
        Some(luxe_core::tool::NETWORK_TOOL_TIMEOUT)
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "web_fetch".to_string(),
            description: web_fetch_description(self.clone_repositories),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "URL to fetch"},
                    "offset": {
                        "type": "integer",
                        "description": "Character offset into the page, for reading a long page in \
                                        sequence. Use the offset the previous reply reported."
                    }
                },
                "required": ["url"]
            }),
        }
    }

    async fn run(&self, args: serde_json::Value, cx: &ToolContext) -> ToolResultData {
        let url = match args.get("url") {
            Some(v) => v.as_str().unwrap_or(""),
            None => {
                return ToolResultData::error("Error: 'url' argument is required");
            }
        };

        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

        // A GitHub repo URL is far more useful cloned (real files + a path to explore) than scraped.
        if self.clone_repositories {
            if let Some((owner, repo)) = Url::parse(url).ok().as_ref().and_then(github_repo) {
                return clone::fetch_repository(&owner, &repo, cx).await;
            }
        }
        // A GitHub blob URL → its raw file (skips the noisy rendered page).
        let effective = github_blob_raw(url).unwrap_or_else(|| url.to_string());
        let url = effective.as_str();

        let config = self.config.read().await;
        let page = fetch_with_fallbacks(url, &config).await;
        match page {
            Ok(content) => ToolResultData::text(page_of(&content, offset)),
            Err(attempts) => ToolResultData::error(format!("Fetch failed. {attempts}")),
        }
    }
}

/// Fetch `url`, escalating through the ways of reading a page that does not want to be read.
///
/// Plainly first, then behind a browser user-agent, then — only where a Firecrawl key says we may
/// — in an actual browser. Each step is strictly more capable and strictly more expensive, so none
/// of them runs until the cheaper one has failed. A silent host skips the retries entirely: a
/// browser cannot make an unanswered socket answer, and three thirty-second attempts against a
/// dead domain is the minute that looks like a hung agent.
///
/// The error names every attempt, because "fetch failed" for a page that is merely JavaScript is a
/// different problem from one that is offline, and the caller can only tell from the attempts.
pub(super) async fn fetch_with_fallbacks(url: &str, config: &WebConfig) -> Result<String, String> {
    let first = match fetch_url(url).await {
        Ok(content) => return Ok(content),
        Err(error) => error,
    };
    if !first.worth_a_second_try() {
        return Err(format!("{first}"));
    }

    let mut attempts = vec![format!("plain: {first}")];
    if config.enable_stealth {
        match fetch_url_stealth(url).await {
            Ok(content) => return Ok(content),
            Err(error) => attempts.push(format!("browser user-agent: {error}")),
        }
    }
    if let Some(key) = config.firecrawl_api_key.as_deref() {
        let firecrawl = crate::firecrawl::FirecrawlBackend::new(key.to_string());
        match firecrawl.scrape(url).await {
            Ok(content) => return Ok(content),
            Err(error) => attempts.push(format!("rendered browser: {error}")),
        }
    } else {
        attempts.push(
            "rendered browser: not tried — set FIRECRAWL_API_KEY to read pages that build \
             themselves in JavaScript"
                .to_string(),
        );
    }
    Err(attempts.join(" / "))
}
