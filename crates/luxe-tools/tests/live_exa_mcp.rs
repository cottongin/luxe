//! Live smoke test for the keyless search endpoint.
//!
//! Skipped unless `LUXE_TEST_LIVE_WEB=1`, so the suite stays offline. Run it when the endpoint's
//! response shape is in question — this is the test that would have caught the advanced tool
//! answering with a JSON envelope while the parser expected prose, which silently turned working
//! searches into "No results".
//!
//! ```bash
//! LUXE_TEST_LIVE_WEB=1 cargo test -p luxe-tools --test live_exa_mcp -- --nocapture
//! ```

use luxe_tools::exa_mcp::ExaMcpBackend;
use luxe_tools::web::SearchOptions;

fn enabled() -> bool {
    std::env::var("LUXE_TEST_LIVE_WEB").is_ok_and(|value| value == "1")
}

#[tokio::test]
async fn the_keyless_endpoint_returns_usable_results_with_content() {
    if !enabled() {
        eprintln!("skipped: set LUXE_TEST_LIVE_WEB=1 to run");
        return;
    }

    let hits = ExaMcpBackend::new()
        .search_with(
            "tokio select cancellation safety",
            3,
            &SearchOptions { include_content: true, ..Default::default() },
        )
        .await
        .expect("the keyless endpoint answers with no credential of any kind");

    assert!(!hits.is_empty(), "a mainstream query must return something");
    for hit in &hits {
        assert!(hit.url.starts_with("http"), "every hit carries a usable url: {hit:?}");
        assert!(!hit.title.trim().is_empty(), "and a title: {hit:?}");
    }
    // The whole point of the tool: enough content to answer from, without a follow-up fetch.
    assert!(
        hits.iter().any(|hit| hit.snippet.chars().count() > 200),
        "at least one result must carry a real passage, not a bare snippet: {hits:#?}"
    );
    eprintln!("{} hits · first: {} ({})", hits.len(), hits[0].title, hits[0].url);
}

#[tokio::test]
async fn the_domain_filter_reaches_the_endpoint() {
    if !enabled() {
        eprintln!("skipped: set LUXE_TEST_LIVE_WEB=1 to run");
        return;
    }

    let hits = ExaMcpBackend::new()
        .search_with(
            "async runtime",
            5,
            &SearchOptions {
                include_domains: vec!["docs.rs".to_string()],
                include_content: true,
                ..Default::default()
            },
        )
        .await
        .expect("filters are accepted");

    assert!(!hits.is_empty(), "the domain filter must not empty the result set");
    assert!(
        hits.iter().all(|hit| hit.url.contains("docs.rs")),
        "includeDomains is honored server-side: {hits:#?}"
    );
}
