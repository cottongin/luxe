use super::fetch::{extract_readable, github_blob_raw, github_repo, page_of, FETCH_MAX_CHARS};
use super::render::{iso_date, plain_text, render_hits, SNIPPET_CHARS};
use super::search::{domain_filters, search_queries};
use super::*;
use luxe_core::ToolContext;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::{mpsc, watch};

struct MockSearchBackend {
    hits: Vec<SearchHit>,
}

impl MockSearchBackend {
    fn new(hits: Vec<SearchHit>) -> Self {
        Self { hits }
    }
}

#[async_trait]
impl SearchBackend for MockSearchBackend {
    async fn search(
        &self,
        _query: &str,
        n: usize,
        _options: &SearchOptions,
    ) -> Result<Vec<SearchHit>, WebError> {
        let n = n.min(self.hits.len());
        Ok(self.hits[..n].to_vec())
    }
}

fn test_context(tmp_dir: &std::path::Path) -> ToolContext {
    let (event_tx, _event_rx) = mpsc::channel(64);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    ToolContext {
        cwd: tmp_dir.to_path_buf(),
        session_id: "test-session".to_string(),
        cancelled: Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    }
}

/// A silent host is the one failure a browser user-agent cannot fix, so `web_fetch` must not
/// spend its budget twice on it. Two 30s attempts against a dead domain is the minute that looked
/// like a hung agent.
#[test]
fn only_a_failure_a_browser_could_fix_earns_the_stealth_retry() {
    assert!(!WebError::Timeout("no answer".into()).worth_a_second_try());
    for error in [
        WebError::Http("HTTP 403 Forbidden".into()),
        WebError::Network("connection reset".into()),
        WebError::Parse("not html".into()),
    ] {
        assert!(error.worth_a_second_try(), "{error} may well succeed as a browser");
    }
}

#[tokio::test]
async fn test_web_search_missing_query() {
    let tools = WebTools::new(WebConfig::default());
    let tool = tools.web_search();
    let tmp_dir = TempDir::new().unwrap();
    let ctx = test_context(tmp_dir.path());

    let result = tool.run(serde_json::json!({}), &ctx).await;
    assert!(result.is_error);
}

#[test]
fn extract_readable_pulls_main_content_as_markdown() {
    let html = r#"<html><head><title>Ignored</title></head><body>
            <nav>Home About Login Newsletter signup</nav>
            <article>
                <h1>Widget Guide</h1>
                <p>Install the widget with <code>cargo add widget</code> first.</p>
                <p>Then call <a href="https://example.com/api">the API</a> to render it.</p>
            </article>
            <footer>Copyright 2026</footer></body></html>"#;
    let md = extract_readable(html, "https://example.com/guide");
    assert!(md.contains("Widget Guide"), "heading kept: {md}");
    assert!(md.contains("cargo add widget"), "body kept: {md}");
    assert!(!md.contains("<article>") && !md.contains("<p>"), "tags stripped: {md}");
}

#[test]
fn github_repo_matches_only_repo_roots() {
    let root = Url::parse("https://github.com/rust-lang/cargo").unwrap();
    assert_eq!(github_repo(&root), Some(("rust-lang".into(), "cargo".into())));
    let dotgit = Url::parse("https://github.com/rust-lang/cargo.git").unwrap();
    assert_eq!(github_repo(&dotgit), Some(("rust-lang".into(), "cargo".into())));
    assert!(github_repo(&Url::parse("https://github.com/rust-lang").unwrap()).is_none());
    assert!(github_repo(&Url::parse("https://github.com/o/r/blob/main/x").unwrap()).is_none());
    assert!(github_repo(&Url::parse("https://example.com/o/r").unwrap()).is_none());
}

/// With cloning off the schema stops promising it, so the model reads a repository as a page
/// rather than waiting on a path that never comes.
#[test]
fn the_description_mentions_cloning_only_when_it_is_on() {
    let on = WebTools::new(WebConfig::default()).web_fetch().schema().description;
    assert!(on.contains(".luxe/gh-cache/"), "{on}");
    let off = WebConfig { clone_repositories: false, ..WebConfig::default() };
    let off = WebTools::new(off).web_fetch().schema().description;
    assert!(!off.contains("clone"), "{off}");
    assert!(off.contains("GitHub blob URL"), "{off}");
}

#[test]
fn github_blob_maps_to_raw() {
    assert_eq!(
        github_blob_raw("https://github.com/o/r/blob/main/src/lib.rs").as_deref(),
        Some("https://raw.githubusercontent.com/o/r/main/src/lib.rs")
    );
    assert!(github_blob_raw("https://github.com/o/r").is_none());
}

#[tokio::test]
async fn test_web_fetch_missing_url() {
    let tools = WebTools::new(WebConfig::default());
    let tool = tools.web_fetch();
    let tmp_dir = TempDir::new().unwrap();
    let ctx = test_context(tmp_dir.path());

    let result = tool.run(serde_json::json!({}), &ctx).await;
    assert!(result.is_error);
}

/// Search with no key is a working capability, not an error. Nothing here touches the network:
/// the contract under test is which backend gets chosen.
#[tokio::test]
async fn no_api_key_falls_back_to_the_keyless_backend() {
    let config = WebConfig::default();
    assert!(!config.has_search_key());
    assert!(create_backend(&config).await.is_ok(), "an unconfigured user still gets search");
}

#[tokio::test]
async fn a_configured_key_still_wins() {
    let config = WebConfig {
        search_backend: Some("brave".to_string()),
        search_api_key: Some("k".to_string()),
        ..Default::default()
    };
    assert!(create_backend(&config).await.is_ok());
    let unknown = WebConfig { search_backend: Some("nope".to_string()), ..config };
    assert!(create_backend(&unknown).await.is_err(), "a typo'd backend name is still an error");
}

#[tokio::test]
async fn a_search_with_no_query_at_all_is_rejected() {
    let tools = WebTools::new(WebConfig::default());
    let tool = tools.web_search();
    let tmp_dir = TempDir::new().unwrap();
    let ctx = test_context(tmp_dir.path());

    let result = tool.run(serde_json::json!({}), &ctx).await;
    assert!(result.is_error);
}

#[test]
fn queries_collapse_to_a_deduplicated_list() {
    let args = serde_json::json!({"queries": ["a", " a ", "b", ""], "query": "b"});
    assert_eq!(search_queries(&args), vec!["a".to_string(), "b".to_string()]);
    assert_eq!(search_queries(&serde_json::json!({"query": "solo"})), vec!["solo"]);
    assert!(search_queries(&serde_json::json!({})).is_empty());
}

#[test]
fn a_leading_dash_excludes_a_domain() {
    let filters =
        domain_filters(&serde_json::json!({"domains": ["docs.rs", "-pinterest.com", "-"]}));
    assert_eq!(filters.include_domains, vec!["docs.rs".to_string()]);
    assert_eq!(filters.exclude_domains, vec!["pinterest.com".to_string()]);
}

/// One window, four vendor spellings. They are not interchangeable: Brave takes a code, Tavily
/// a word, Firecrawl a Google `tbs` operator, and Exa an absolute date.
#[test]
fn a_recency_window_speaks_every_vendors_dialect() {
    let today = iso_date(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs(),
    );
    let week = Recency::parse("week").expect("a window the schema offers");
    assert_eq!(week.brave_code(), "pw");
    assert_eq!(week.tavily_range(), "week");
    assert_eq!(week.tbs(), "qdr:w");
    let since = week.since();
    assert_eq!(since.len(), 10, "YYYY-MM-DD: {since}");
    assert!(since < today, "{since} must precede {today}");
    assert!(Recency::parse("decade").is_none());
    assert!(Recency::Day.since() > Recency::Year.since(), "a shorter window starts later");
    // A known instant, so the civil-date arithmetic itself is pinned.
    assert_eq!(iso_date(1_767_225_600), "2026-01-01");
}

#[test]
fn results_render_as_compact_markdown_not_json() {
    let hits = vec![
        SearchHit {
            title: "Tokio select".into(),
            url: "https://docs.rs/tokio".into(),
            snippet: "Cancellation safety describes what happens…".into(),
        },
        SearchHit {
            title: "Bare".into(),
            url: "https://example.test".into(),
            snippet: String::new(),
        },
    ];
    let rendered = render_hits(&["a".into(), "b".into()], &hits, &[]);

    assert!(rendered.starts_with("Searched: a · b"), "{rendered}");
    assert!(rendered.contains("1. Tokio select"));
    assert!(rendered.contains("https://docs.rs/tokio"));
    assert!(rendered.contains("Cancellation safety"), "the passage rides along");
    assert!(!rendered.contains('{'), "no JSON punctuation tax: {rendered}");
    assert_eq!(render_hits(&["q".into()], &[], &[]), "No results for q.");
}

/// Brave marks the matched words with `<strong>` and Firecrawl hands back the page's own
/// markdown, newlines and fences included. Neither is information, and both are paid for by
/// the token.
#[test]
fn a_snippet_is_reduced_to_one_line_of_readable_text() {
    let brave = "Cancellation safety <strong>describes what happens</strong> &amp; why";
    assert_eq!(
        plain_text(brave, SNIPPET_CHARS),
        "Cancellation safety describes what happens & why"
    );

    let firecrawl = "# Struct CancellationToken\n\n## Examples\n```\nuse tokio::select;\n```";
    assert_eq!(
        plain_text(firecrawl, SNIPPET_CHARS),
        "# Struct CancellationToken ## Examples ``` use tokio::select; ```"
    );

    let long = "x".repeat(SNIPPET_CHARS + 500);
    let capped = plain_text(&long, SNIPPET_CHARS);
    assert_eq!(capped.chars().count(), SNIPPET_CHARS + 1, "capped, plus the ellipsis");
    assert!(capped.ends_with('…'));

    // A double-escaped entity decodes once, not twice.
    assert_eq!(plain_text("a &amp;lt; b", SNIPPET_CHARS), "a &lt; b");
    assert!(plain_text("   \n \t ", SNIPPET_CHARS).is_empty());
}

/// A docs.rs paragraph rendered to markdown spends more characters on link targets than on
/// prose, and the result's own URL is already on the line above.
#[test]
fn inline_link_targets_are_dropped_and_their_text_kept() {
    assert_eq!(
        plain_text("see [Receiver::recv](https://docs.rs/x#method.recv) for details", 500),
        "see Receiver::recv for details"
    );
    assert_eq!(plain_text("jump to [§](#examples) now", 500), "jump to § now");
    // Not links, and each has to survive being mistaken for one.
    for kept in [
        "an array [0](1) index",
        "a [bracket] alone",
        "nested [a [b](c)] text",
        "[unclosed",
        "[text](not a url)",
    ] {
        assert_eq!(plain_text(kept, 500), kept, "mangled a non-link");
    }
}

/// A note about what the backend could not do has to arrive before the results it qualifies,
/// and has to arrive even when there are none.
#[test]
fn a_backends_caveat_leads_the_reply() {
    let caveats = vec!["Brave returns a short search snippet".to_string()];
    let rendered = render_hits(&["q".into()], &[], &caveats);
    assert!(rendered.starts_with("Note: Brave returns"), "{rendered}");
    assert!(rendered.contains("No results for q."), "{rendered}");
}

#[tokio::test]
async fn test_web_search_with_api_key() {
    // Test with mock backend since we don't want to make real API calls
    let config = WebConfig {
        search_backend: Some("mock".to_string()),
        search_api_key: Some("test-key".to_string()),
        ..Default::default()
    };
    let tools = WebTools::new(config);
    let tool = tools.web_search();
    let tmp_dir = TempDir::new().unwrap();
    let ctx = test_context(tmp_dir.path());

    let result = tool.run(serde_json::json!({"query": "test", "n": 3}), &ctx).await;
    // Should fail because mock backend is not implemented in create_backend
    assert!(result.is_error);
}

#[tokio::test]
async fn test_web_fetch_with_url() {
    let tools = WebTools::new(WebConfig::default());
    let tool = tools.web_fetch();
    let tmp_dir = TempDir::new().unwrap();
    let ctx = test_context(tmp_dir.path());

    let result = tool.run(serde_json::json!({"url": "https://example.com"}), &ctx).await;
    assert!(!result.is_error);
    assert!(!result.content.is_empty());
}

#[tokio::test]
async fn test_mock_search_backend() {
    let hits = vec![
        SearchHit {
            title: "Test Result 1".to_string(),
            url: "https://example.com/1".to_string(),
            snippet: "Snippet 1".to_string(),
        },
        SearchHit {
            title: "Test Result 2".to_string(),
            url: "https://example.com/2".to_string(),
            snippet: "Snippet 2".to_string(),
        },
    ];

    let backend = MockSearchBackend::new(hits);
    let results = backend.search("test", 2, &SearchOptions::default()).await.unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].title, "Test Result 1");
    assert_eq!(results[1].title, "Test Result 2");
}

#[tokio::test]
async fn test_mock_search_backend_limit() {
    let hits = vec![
        SearchHit {
            title: "Result 1".to_string(),
            url: "https://example.com/1".to_string(),
            snippet: "Snippet 1".to_string(),
        },
        SearchHit {
            title: "Result 2".to_string(),
            url: "https://example.com/2".to_string(),
            snippet: "Snippet 2".to_string(),
        },
        SearchHit {
            title: "Result 3".to_string(),
            url: "https://example.com/3".to_string(),
            snippet: "Snippet 3".to_string(),
        },
    ];

    let backend = MockSearchBackend::new(hits);
    let results = backend.search("test", 2, &SearchOptions::default()).await.unwrap();
    assert_eq!(results.len(), 2);
}

/// A bound that cannot be walked past is a trap: pages with no anchors and no sub-pages would
/// leave the remainder permanently unreachable. Every window must name the next offset.
#[test]
fn a_long_page_is_walkable_end_to_end() {
    let page: String = "abcdefghij".repeat(FETCH_MAX_CHARS / 10 * 3 + 137);
    let total = page.chars().count();

    let mut offset = 0usize;
    let mut seen = 0usize;
    let mut windows = 0;
    loop {
        let out = page_of(&page, offset);
        windows += 1;
        assert!(windows < 20, "should terminate");
        let next = out
            .rsplit("offset: ")
            .next()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|n| *n > offset);
        match next {
            Some(n) => {
                seen += n - offset;
                offset = n;
            }
            None => {
                assert!(out.contains("end of page"), "the last window says so: {}", &out[..80]);
                seen += total - offset;
                break;
            }
        }
    }
    assert_eq!(seen, total, "every character is reachable across {windows} windows");
}

#[test]
fn a_short_page_is_returned_untouched() {
    assert_eq!(page_of("just a little page", 0), "just a little page");
}

#[test]
fn the_first_window_of_a_long_page_reports_the_next_offset() {
    let page = "z".repeat(FETCH_MAX_CHARS * 2);
    let out = page_of(&page, 0);
    assert!(
        out.starts_with(&format!("[characters 0\u{2013}{FETCH_MAX_CHARS} of ")),
        "{}",
        &out[..60]
    );
    assert!(out.contains(&format!("offset: {FETCH_MAX_CHARS}")), "names the exact next offset");
    assert!(out.contains("characters remain"));
}

/// Even the mistake is recoverable rather than a dead end.
#[test]
fn an_offset_past_the_end_says_how_to_recover() {
    let out = page_of("short", 9_000);
    assert!(out.contains("past the end"), "{out}");
    assert!(out.contains("offset 0"), "and how to start over: {out}");
}

/// A page cut at the size cap says so after extraction, where readability cannot drop the note.
#[test]
fn a_cut_page_says_so_after_extraction() {
    let html =
        "<html><body><article><h1>Title</h1><p>Some long article text.</p></article></body></html>";
    let read = |truncated| {
        let fetched = crate::ssrf::Fetched { body: html.into(), truncated };
        fetched.reduced(|html| extract_readable(html, "https://x.test/"))
    };
    let cut = read(true);
    assert!(cut.trim_end().ends_with("and the rest was not read]"), "{cut}");
    assert!(!read(false).contains("truncated"));
}
