//! Live smoke tests for every `web_search` backend, run against the real vendors.
//!
//! Skipped unless `LUXE_TEST_LIVE_WEB=1`, so the suite stays offline; each keyed backend is
//! skipped again unless its key is in the environment. Unit tests pin the *request* each backend
//! builds, which is where the silent bugs live — a filter serialized under a name the vendor does
//! not read is accepted, ignored, and answered 200. These pin the other half: that the vendor
//! agrees, and that what comes back is shaped like an answer.
//!
//! Every backend here shipped once without this. Brave interpolated the query straight into a URL,
//! so `tokio select! & cancellation` reached it as `tokio select!`; Exa asked for `num_results`,
//! which it does not read, and never requested page text at all. Both returned confident results
//! for a question nobody asked.
//!
//! ```bash
//! LUXE_TEST_LIVE_WEB=1 cargo test -p luxe-tools --test live_web_search -- --nocapture
//! ```

use luxe_tools::brave::BraveBackend;
use luxe_tools::exa::ExaBackend;
use luxe_tools::firecrawl::FirecrawlBackend;
use luxe_tools::tavily::TavilyBackend;
use luxe_tools::web::{Recency, SearchBackend, SearchHit, SearchOptions};

fn enabled() -> bool {
    std::env::var("LUXE_TEST_LIVE_WEB").is_ok_and(|value| value == "1")
}

/// The key for `var`, or `None` with a note saying which test was skipped and why.
fn key(var: &str) -> Option<String> {
    if !enabled() {
        eprintln!("skipped: set LUXE_TEST_LIVE_WEB=1 to run");
        return None;
    }
    match std::env::var(var).ok().filter(|value| !value.trim().is_empty()) {
        Some(key) => Some(key),
        None => {
            eprintln!("skipped: {var} is not set");
            None
        }
    }
}

/// Every backend owes the caller the same three things, whatever it costs to get them.
fn assert_usable(hits: &[SearchHit], what: &str) {
    assert!(!hits.is_empty(), "{what}: a mainstream query must return something");
    for hit in hits {
        assert!(hit.url.starts_with("http"), "{what}: every hit carries a usable url: {hit:?}");
        assert!(!hit.title.trim().is_empty(), "{what}: and a title: {hit:?}");
    }
    assert!(
        hits.iter().any(|hit| hit.snippet.chars().count() > 80),
        "{what}: a result with no text to read is a link the model has to spend a fetch on: \
         {hits:#?}"
    );
    eprintln!("{what}: {} hits · first: {} ({})", hits.len(), hits[0].title, hits[0].url);
}

#[tokio::test]
async fn brave_searches_the_query_it_was_given() {
    let Some(api_key) = key("BRAVE_API_KEY") else { return };
    let backend = BraveBackend::new(api_key);

    let hits = backend
        .search("tokio cancellation safety", 3, &SearchOptions::default())
        .await
        .expect("Brave answers");
    assert_usable(&hits, "brave");

    // The punctuation that used to truncate the query. Brave answers 200 either way, so the only
    // observable difference is whether the results are about the right thing.
    let punctuated = backend
        .search("rust #[tokio::main] macro & async fn", 3, &SearchOptions::default())
        .await
        .expect("Brave accepts a punctuated query");
    assert!(!punctuated.is_empty(), "a punctuated query must still search");

    let filtered = backend
        .search(
            "cancellation",
            5,
            &SearchOptions {
                include_domains: vec!["docs.rs".to_string()],
                recency: Some(Recency::Year),
                ..Default::default()
            },
        )
        .await
        .expect("Brave accepts site: operators and a freshness code");
    assert!(
        filtered.iter().all(|hit| hit.url.contains("docs.rs")),
        "site: operators are the domain filter Brave actually reads: {filtered:#?}"
    );

    // Above 20 Brave answers 422 rather than clamping, so the clamp has to be ours.
    assert!(backend.search("rust", 50, &SearchOptions::default()).await.is_ok());
}

#[tokio::test]
async fn tavily_applies_every_filter_the_schema_advertises() {
    let Some(api_key) = key("TAVILY_API_KEY") else { return };
    let backend = TavilyBackend::new(api_key);

    let hits = backend
        .search(
            "tokio cancellation safety",
            3,
            &SearchOptions { include_content: true, ..Default::default() },
        )
        .await
        .expect("Tavily answers a bearer-authenticated request");
    assert_usable(&hits, "tavily");

    let filtered = backend
        .search(
            "tokio",
            5,
            &SearchOptions {
                include_domains: vec!["docs.rs".to_string()],
                recency: Some(Recency::Year),
                ..Default::default()
            },
        )
        .await
        .expect("Tavily accepts the filters");
    assert!(
        filtered.iter().all(|hit| hit.url.contains("docs.rs")),
        "include_domains is honored server-side: {filtered:#?}"
    );
}

#[tokio::test]
async fn exa_returns_the_count_asked_for_and_the_page_text() {
    let Some(api_key) = key("EXA_API_KEY") else { return };
    let backend = ExaBackend::new(api_key);

    let hits = backend
        .search(
            "tokio cancellation safety",
            3,
            &SearchOptions { include_content: true, ..Default::default() },
        )
        .await
        .expect("Exa answers");
    assert_usable(&hits, "exa");
    // `num_results` was accepted and ignored, so this came back as Exa's default page size.
    assert!(hits.len() <= 3, "numResults is the field Exa reads: {} hits", hits.len());
    assert!(
        hits.iter().any(|hit| hit.snippet.chars().count() > 200),
        "contents.text is what makes a hit answerable without a fetch: {hits:#?}"
    );
}

#[tokio::test]
async fn firecrawl_searches_and_renders_a_page_a_plain_get_cannot_read() {
    let Some(api_key) = key("FIRECRAWL_API_KEY") else { return };
    let backend = FirecrawlBackend::new(api_key);

    let hits = backend
        .search(
            "tokio cancellation safety",
            3,
            &SearchOptions { include_content: true, ..Default::default() },
        )
        .await
        .expect("Firecrawl answers");
    assert_usable(&hits, "firecrawl");

    let filtered = backend
        .search(
            "cancellation",
            5,
            &SearchOptions {
                include_domains: vec!["docs.rs".to_string()],
                recency: Some(Recency::Year),
                ..Default::default()
            },
        )
        .await
        .expect("Firecrawl accepts the filters");
    assert!(
        filtered.iter().all(|hit| hit.url.contains("docs.rs")),
        "includeDomains is honored server-side: {filtered:#?}"
    );

    // The reason this backend earns its place in `web_fetch`: a rendered page, as markdown.
    let page = backend
        .scrape("https://openrouter.ai/docs/features/prompt-caching")
        .await
        .expect("Firecrawl renders a JavaScript-built docs page");
    assert!(page.len() > 2_000, "a rendered page is a page, not a shell: {} chars", page.len());
    assert!(page.contains("cach"), "and it is the page that was asked for");
    eprintln!("firecrawl scrape: {} chars", page.len());

    // Firecrawl answers 200 with a stub for an upstream 404; reporting that as content would hand
    // the model a "Not Found" page as though it were the article. (A *soft* 404 — a 200 whose body
    // says the page is missing, which is what docs.rs serves — is indistinguishable from content
    // and is not what this checks.)
    let missing = backend.scrape("https://www.rust-lang.org/no-such-page-luxe-xyz").await;
    assert!(missing.is_err(), "an upstream 404 is a failure, not a page: {missing:?}");
}
