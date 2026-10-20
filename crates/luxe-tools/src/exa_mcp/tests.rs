use super::*;

/// The exact shape the live endpoint returned during development.
const SAMPLE: &str = "event: message\n\
    data: {\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"Title: select in tokio - Rust\\nURL: https://docs.rs/tokio/latest/tokio/macro.select.html\\nPublished: N/A\\nAuthor: N/A\\nHighlights:\\n## Cancellation safety\\n...\\nCancellation safety describes what happens when a future is dropped.\\nTitle: Second Result\\nURL: https://example.com/two\\nHighlights:\\nSomething else entirely.\"}]}}\n";

/// A burst of searches crossed the keyless limit; this is the reply, copied from the endpoint.
const RATE_LIMITED: &str = "event: message\ndata: {\"result\":{\"_meta\":{\"ai.exa/rateLimited\":true},\"content\":[{\"type\":\"text\",\"text\":\"You've hit Exa's free MCP rate limit. To continue using without limits, create your own Exa API key.\\n\\nFix: Create API key at https://dashboard.exa.ai/api-keys , and then update Exa MCP URL to this https://mcp.exa.ai/mcp?exaApiKey=YOUR_EXA_API_KEY\"}]},\"jsonrpc\":\"2.0\",\"id\":1}\n";

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
    let bare = r#"{"result":{"content":[{"type":"text","text":"Title: A\nURL: https://a.test"}]}}"#;
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
    let body = r#"{"result":{"isError":true,"content":[{"type":"text","text":"rate limited"}]}}"#;
    let error = parse_response(body, 5).expect_err("isError must not be silently empty");
    assert!(error.to_string().contains("rate limited"), "{error}");
}

/// The break this catches: a rate-limited reply, which isn't flagged as an error, reading as a
/// search that found nothing, so the model gave up on a question the web could answer.
#[test]
fn a_rate_limited_reply_is_an_error_not_zero_results() {
    let error = parse_response(RATE_LIMITED, 5).expect_err("rate limited is not empty");
    assert!(error.to_string().contains("rate limit"), "{error}");
    assert!(error.to_string().contains("EXA_API_KEY"), "says how to lift it: {error}");
}

/// The break this catches: the keyless endpoint getting a search's phrasings all at once, which
/// crosses its limit and turns all but one of them away.
#[test]
fn the_keyless_endpoint_gets_one_phrasing_at_a_time() {
    assert!(ExaMcpBackend::new().one_phrasing_at_a_time());
}

#[test]
fn garbage_is_an_error_not_a_panic() {
    assert!(parse_response("not json at all", 5).is_err());
    assert!(parse_response("", 5).is_err());
}

#[test]
fn options_map_onto_exas_argument_names() {
    let options = SearchOptions {
        include_domains: vec!["arxiv.org".into()],
        exclude_domains: vec!["spam.example".into()],
        recency: Some(crate::web::Recency::Month),
        include_content: true,
    };
    let args = build_arguments("tokio cancellation", 7, &options);

    assert_eq!(args["query"], "tokio cancellation");
    assert_eq!(args["numResults"], 7);
    assert_eq!(args["highlightsQuery"], "tokio cancellation", "passages for this phrasing");
    assert_eq!(args["includeDomains"][0], "arxiv.org");
    assert_eq!(args["excludeDomains"][0], "spam.example");
    assert_eq!(args["startPublishedDate"].as_str().expect("a date").len(), 10);
    assert_eq!(args["enableHighlights"], true);
}

#[test]
fn defaults_stay_off_the_wire_and_the_count_is_clamped() {
    let args = build_arguments("q", 999, &SearchOptions::default());
    assert_eq!(args["numResults"], 25, "clamped to Exa's practical ceiling");
    for absent in ["includeDomains", "excludeDomains", "startPublishedDate", "enableHighlights"] {
        assert!(args.get(absent).is_none(), "{absent} must not be sent by default");
    }
}

/// The advanced tool answers with a JSON envelope, not the basic tool's prose. Parsing only the
/// prose form meant every advanced search reported "No results" while the endpoint was returning
/// perfectly good ones.
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
    let body = serde_json::json!({ "result": { "content": [{ "type": "text", "text": inner }] } })
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
    let inner =
        serde_json::json!({ "results": [{ "url": "https://a.test", "title": "A", "text": long }] })
            .to_string();
    let body = serde_json::json!({ "result": { "content": [{ "text": inner }] } }).to_string();
    let hits = parse_response(&body, 5).expect("parses");
    assert!(hits[0].snippet.chars().count() <= SNIPPET_CHARS + 1, "clipped");
    assert!(hits[0].snippet.ends_with('…'));
}
