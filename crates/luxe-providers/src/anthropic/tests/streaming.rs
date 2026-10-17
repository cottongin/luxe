//! Streaming — the adapter against a canned server: headers, retries, refreshes.

use super::*;

/// Regression: the adapter must POST to `{base}/messages` (NOT `/messages/stream`, which
/// 404s on real Anthropic), authenticate with `x-api-key` (NOT `Authorization: Bearer`),
/// and request SSE via `stream: true` in the body. Verified against the real
/// `AnthropicProvider::stream` using a std TcpListener mock (no extra deps).
#[tokio::test]
async fn stream_posts_to_messages_with_x_api_key_and_stream_flag() {
    let server = serve_once("200 OK", "text/event-stream", &sse_turn("pong"));
    let port = server.port();

    let model =
        ModelInfo { r#ref: ModelRef::new("anthropic", "claude-sonnet-4-6"), ..Default::default() };
    let provider = AnthropicProvider::new(
        "test-key".into(),
        Some(format!("http://127.0.0.1:{port}/v1")),
        model.clone(),
        AnthropicCompat::default(),
    );
    let (_keep, cancel) = tokio::sync::watch::channel(false);
    let stream = provider
        .stream(turn_request(&model, ThinkingLevel::Off), cancel)
        .await
        .expect("stream opens");
    let events: Vec<ProviderEvent> = stream.collect().await;

    let raw = server.requests().remove(0);
    let request_line = raw.lines().next().unwrap_or_default();
    assert!(
        request_line.starts_with("POST /v1/messages "),
        "must POST to /v1/messages, got: {request_line:?}"
    );
    assert!(!request_line.contains("/messages/stream"), "the /messages/stream path 404s");
    let lower = raw.to_ascii_lowercase();
    assert!(lower.contains("x-api-key: test-key"), "must authenticate with x-api-key");
    assert!(!lower.contains("authorization:"), "Anthropic rejects Authorization: Bearer");
    assert!(lower.contains("anthropic-version:"), "must send anthropic-version");
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or_default();
    assert!(body.contains("\"stream\":true"), "body must request SSE: {body}");

    let text: String = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert!(text.contains("pong"), "provider should parse the SSE text delta, got: {events:?}");
}

/// Runtime model switching: the streamed body must carry the *request's* model id, not the
/// adapter's build-time model. Without this, `Action::SetModel` couldn't change the Anthropic
/// model within one adapter.
#[tokio::test]
async fn stream_uses_the_per_request_model_id() {
    let server = serve_once("200 OK", "text/event-stream", &sse_turn(""));
    let port = server.port();

    // Adapter built for one model; the request asks for a *different* one.
    let provider = AnthropicProvider::new(
        "test-key".into(),
        Some(format!("http://127.0.0.1:{port}/v1")),
        ModelInfo { r#ref: ModelRef::new("anthropic", "claude-sonnet-4-6"), ..Default::default() },
        AnthropicCompat::default(),
    );
    let requested =
        ModelInfo { r#ref: ModelRef::new("anthropic", "claude-opus-4-8"), ..Default::default() };
    let (_keep, cancel) = tokio::sync::watch::channel(false);
    let stream = provider
        .stream(turn_request(&requested, ThinkingLevel::Off), cancel)
        .await
        .expect("stream opens");
    let _events: Vec<ProviderEvent> = stream.collect().await;

    let raw = server.requests().remove(0);
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or_default();
    assert!(
        body.contains("\"model\":\"claude-opus-4-8\""),
        "body must carry the request's model, got: {body}"
    );
    assert!(
        !body.contains("claude-sonnet-4-6"),
        "body must not fall back to the adapter's build-time model: {body}"
    );
}

/// A 400 that names the thinking mode is a mismatch we can fix, not a turn to fail: the adapter
/// flips the encoding, retries once, and remembers the answer so the next turn goes out right
/// the first time. This is the path a stale catalog lands on when a new model drops extended
/// thinking.
#[tokio::test]
async fn a_rejected_thinking_mode_is_retried_and_remembered() {
    let rejection = r#"{"type":"error","error":{"type":"invalid_request_error","message":"thinking.type.enabled is not supported for this model. Use thinking.type.adaptive and output_config.effort to control thinking behavior"}}"#;
    let server = serve_replies(vec![
        http_reply("400 Bad Request", "application/json", rejection),
        http_reply("200 OK", "text/event-stream", &sse_turn("pong")),
        http_reply("200 OK", "text/event-stream", &sse_turn("pong")),
    ]);
    let port = server.port();

    // No catalog metadata at all, so the adapter opens on extended thinking — exactly the
    // situation the retry exists for.
    let model = ModelInfo {
        r#ref: ModelRef::new("anthropic", "claude-sonnet-9"),
        reasoning: true,
        ..Default::default()
    };
    let provider = AnthropicProvider::new(
        "test-key".into(),
        Some(format!("http://127.0.0.1:{port}/v1")),
        model.clone(),
        AnthropicCompat::default(),
    );
    for _ in 0..2 {
        let (_keep, cancel) = tokio::sync::watch::channel(false);
        let stream = provider
            .stream(turn_request(&model, ThinkingLevel::High), cancel)
            .await
            .expect("the retry recovers the turn");
        let events: Vec<ProviderEvent> = stream.collect().await;
        assert!(
            events.iter().any(|event| matches!(event, ProviderEvent::TextDelta { .. })),
            "the retried turn streams: {events:?}"
        );
    }

    let bodies = server
        .requests()
        .iter()
        .map(|raw| raw.split("\r\n\r\n").nth(1).unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert_eq!(bodies.len(), 3, "one rejected attempt, its retry, then the next turn");
    assert!(bodies[0].contains("\"budget_tokens\""), "first attempt: {}", bodies[0]);
    assert!(bodies[1].contains("\"adaptive\""), "retry flips to adaptive: {}", bodies[1]);
    assert!(bodies[1].contains("\"effort\":\"high\""), "retry carries effort: {}", bodies[1]);
    assert!(
        bodies[2].contains("\"adaptive\"") && !bodies[2].contains("\"budget_tokens\""),
        "the learned encoding is reused without a second rejection: {}",
        bodies[2]
    );
}

/// A model the catalog doesn't know goes out at the current generation's ceiling, and one that
/// can't honor it is not a failed turn: Anthropic names the real limit, the adapter retries at
/// it, and every later turn opens there without a second rejection.
#[tokio::test]
async fn a_refused_output_cap_is_retried_at_the_named_limit_and_remembered() {
    // Verbatim shape from a real rejection.
    let rejection = r#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens: 128000 > 32000, which is the maximum allowed number of output tokens for claude-opus-4-1-20250805"},"request_id":"req_011CXxg3eqF8ZMvqtkKHNjKQ"}"#;
    let server = serve_replies(vec![
        http_reply("400 Bad Request", "application/json", rejection),
        http_reply("200 OK", "text/event-stream", &sse_turn("pong")),
        http_reply("200 OK", "text/event-stream", &sse_turn("pong")),
    ]);
    let port = server.port();
    let model = ModelInfo {
        r#ref: ModelRef::new("foundry-anthropic", "opus-legacy-prod"),
        ..Default::default()
    };
    let provider = AnthropicProvider::new(
        "test-key".into(),
        Some(format!("http://127.0.0.1:{port}/v1")),
        model.clone(),
        AnthropicCompat::default(),
    );
    for _ in 0..2 {
        let (_keep, cancel) = tokio::sync::watch::channel(false);
        let request = CompletionRequest {
            max_tokens: Some(128_000),
            ..turn_request(&model, ThinkingLevel::Off)
        };
        let stream = provider.stream(request, cancel).await.expect("the retry recovers");
        let events: Vec<ProviderEvent> = stream.collect().await;
        assert!(events.iter().any(|event| matches!(event, ProviderEvent::TextDelta { .. })));
    }

    let bodies = server
        .requests()
        .iter()
        .map(|raw| raw.split("\r\n\r\n").nth(1).unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert_eq!(bodies.len(), 3, "one refusal, its retry, then the next turn");
    assert!(bodies[0].contains("\"max_tokens\":128000"), "{}", bodies[0]);
    assert!(bodies[1].contains("\"max_tokens\":32000"), "retried at the limit: {}", bodies[1]);
    assert!(bodies[2].contains("\"max_tokens\":32000"), "and remembered: {}", bodies[2]);
}

/// An endpoint that doesn't know in-place tool loading is told once: the adapter lists the
/// deferred tools instead, retries, and keeps listing them for that model, so a proxy that
/// lags behind Anthropic costs one wasted round trip rather than a session.
#[tokio::test]
async fn a_refusal_of_in_place_loading_falls_back_to_listing_and_is_remembered() {
    use luxe_core::provider::ToolSchema;
    let rejection = r#"{"type":"error","error":{"type":"invalid_request_error","message":"tools.3.defer_loading: Extra inputs are not permitted"}}"#;
    let server = serve_replies(vec![
        http_reply("400 Bad Request", "application/json", rejection),
        http_reply("200 OK", "text/event-stream", &sse_turn("pong")),
        http_reply("200 OK", "text/event-stream", &sse_turn("pong")),
    ]);
    let port = server.port();
    let model =
        ModelInfo { r#ref: ModelRef::new("proxy", "claude-sonnet-5"), ..Default::default() };
    let provider = AnthropicProvider::new(
        "test-key".into(),
        Some(format!("http://127.0.0.1:{port}/v1")),
        model.clone(),
        AnthropicCompat::default(),
    );
    let schema = |name: &str| ToolSchema {
        name: name.to_string(),
        description: String::new(),
        parameters: serde_json::json!({"type": "object"}),
    };
    for _ in 0..2 {
        let (_keep, cancel) = tokio::sync::watch::channel(false);
        let request = CompletionRequest {
            tools: vec![schema("activate_tools"), schema("read")],
            deferred_tools: vec![schema("debug")],
            ..turn_request(&model, ThinkingLevel::Off)
        };
        let stream = provider.stream(request, cancel).await.expect("the retry recovers");
        let events: Vec<ProviderEvent> = stream.collect().await;
        assert!(events.iter().any(|event| matches!(event, ProviderEvent::TextDelta { .. })));
    }

    let bodies = server
        .requests()
        .iter()
        .map(|raw| raw.split("\r\n\r\n").nth(1).unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert_eq!(bodies.len(), 3, "one refusal, its retry, then the next turn");
    assert!(bodies[0].contains("\"defer_loading\":true"), "{}", bodies[0]);
    assert!(!bodies[1].contains("defer_loading"), "retried listed: {}", bodies[1]);
    assert!(!bodies[1].contains("\"debug\""), "and the inactive tool stays out: {}", bodies[1]);
    assert!(!bodies[2].contains("defer_loading"), "and remembered: {}", bodies[2]);
}

/// A refresh is the turn's own request with a one-token cap — same breakpoints, same thinking
/// encoding — because anything else would write a different cache entry instead of reading
/// the one it exists to keep. Its usage comes back so the cost is recorded like any call's.
#[tokio::test]
async fn a_cache_refresh_replays_the_request_at_one_token_and_reports_its_cost() {
    let refresh = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":3,",
        "\"cache_read_input_tokens\":180000,\"cache_creation_input_tokens\":0,",
        "\"output_tokens\":1}}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"},",
        "\"usage\":{\"output_tokens\":1}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n",
    );
    let server = serve_replies(vec![
        http_reply("200 OK", "text/event-stream", &sse_turn("pong")),
        http_reply("200 OK", "text/event-stream", refresh),
    ]);
    let port = server.port();
    let model = ModelInfo {
        r#ref: ModelRef::new("anthropic", "claude-opus-5"),
        reasoning: true,
        thinking_controls: ThinkingControls { effort: true, ..Default::default() },
        thinking_level_map: HashMap::from([("high".to_string(), Some("high".to_string()))]),
        ..Default::default()
    };
    let provider = AnthropicProvider::new(
        "test-key".into(),
        Some(format!("http://127.0.0.1:{port}/v1")),
        model.clone(),
        AnthropicCompat::default(),
    );
    let request = CompletionRequest {
        system: Some("You are terse.".into()),
        ..turn_request(&model, ThinkingLevel::High)
    };
    assert_eq!(
        provider.prompt_cache_ttl(&request),
        Some(std::time::Duration::from_secs(300)),
        "adaptive thinking replays cleanly on the default five-minute tier"
    );

    let (_keep, cancel) = tokio::sync::watch::channel(false);
    let _turn: Vec<ProviderEvent> =
        provider.stream(request.clone(), cancel).await.expect("turn").collect().await;
    let usage = provider.warm_cache(request).await.expect("the refresh reports usage");
    assert_eq!(usage.cache_read_tokens, 180_000);
    assert_eq!(usage.cache_write_tokens, 0);
    assert_eq!(usage.completion_tokens, 1);

    let bodies: Vec<serde_json::Value> = server
        .requests()
        .iter()
        .map(|raw| {
            serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap_or_default())
                .expect("a JSON body")
        })
        .collect();
    let (mut turn, mut warm) = (bodies[0].clone(), bodies[1].clone());
    assert_eq!(warm["max_tokens"], 1);
    assert_ne!(turn["max_tokens"], 1);
    turn.as_object_mut().unwrap().remove("max_tokens");
    warm.as_object_mut().unwrap().remove("max_tokens");
    assert_eq!(turn, warm, "nothing but the cap may differ, or the refresh writes a new entry");
}

/// A refresh that fails says why, so the warmer's log names the provider's reason rather than
/// a generic "no usage".
#[tokio::test]
async fn a_refresh_the_provider_refuses_reports_its_reason() {
    let overloaded = concat!(
        "event: error\n",
        "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",",
        "\"message\":\"Overloaded\"}}\n\n",
    );
    let server = serve_once("200 OK", "text/event-stream", overloaded);
    let port = server.port();
    let model = ModelInfo::default();
    let provider = AnthropicProvider::new(
        "test-key".into(),
        Some(format!("http://127.0.0.1:{port}/v1")),
        model.clone(),
        AnthropicCompat::default(),
    );
    let error = provider
        .warm_cache(turn_request(&model, ThinkingLevel::Off))
        .await
        .expect_err("an error event fails the refresh");
    assert!(error.to_string().contains("Overloaded"), "{error}");
    server.requests();
}

/// A refresh returns at the end of the reply, not at the end of the connection. A server or
/// proxy that holds the socket open after `message_stop` would otherwise keep the refresh
/// waiting until the warmer gave up on it.
#[tokio::test]
async fn a_refresh_returns_when_the_reply_ends_even_if_the_connection_does_not() {
    // No length and no close: the body ends only when the socket does
    let (server, release) = serve_held_open(concat!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n",
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":2,",
        "\"cache_read_input_tokens\":9000,\"output_tokens\":1}}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"},",
        "\"usage\":{\"output_tokens\":1}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    ));
    let port = server.port();
    let model = ModelInfo::default();
    let provider = AnthropicProvider::new(
        "test-key".into(),
        Some(format!("http://127.0.0.1:{port}/v1")),
        model.clone(),
        AnthropicCompat::default(),
    );
    let usage = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        provider.warm_cache(turn_request(&model, ThinkingLevel::Off)),
    )
    .await
    .expect("the refresh returns without waiting for the socket")
    .expect("usage");
    assert_eq!(usage.cache_read_tokens, 9_000);
    let _ = release.send(());
    server.requests();
}

/// Which requests can be kept warm at all.
#[test]
fn only_a_request_that_replays_byte_for_byte_is_offered_for_warming() {
    use luxe_core::provider::{ModelRef, PromptReuse};
    let adapter = |compat| AnthropicProvider::new("k".into(), None, ModelInfo::default(), compat);
    let budgeted = ModelInfo {
        r#ref: ModelRef::new("anthropic", "claude-sonnet-4-5"),
        reasoning: true,
        thinking_controls: ThinkingControls { budget_tokens: true, ..Default::default() },
        ..Default::default()
    };
    let five = Some(std::time::Duration::from_secs(300));
    let default = adapter(AnthropicCompat::default());

    assert_eq!(
        default.prompt_cache_ttl(&turn_request(&budgeted, ThinkingLevel::High)),
        None,
        "the budget follows max_tokens and keys the message cache, so a replay can't match"
    );
    assert_eq!(
        default.prompt_cache_ttl(&turn_request(&budgeted, ThinkingLevel::Off)),
        five,
        "with thinking off there is no budget to move"
    );
    let one_off = CompletionRequest {
        reuse: PromptReuse::OneOff,
        ..turn_request(&budgeted, ThinkingLevel::Off)
    };
    assert_eq!(default.prompt_cache_ttl(&one_off), None, "nothing cached, nothing to keep");
    assert_eq!(
        adapter(AnthropicCompat { cache_control: false, ..AnthropicCompat::default() })
            .prompt_cache_ttl(&turn_request(&budgeted, ThinkingLevel::Off)),
        None
    );
    assert_eq!(
        adapter(AnthropicCompat { long_cache_retention: true, ..AnthropicCompat::default() })
            .prompt_cache_ttl(&turn_request(&budgeted, ThinkingLevel::Off)),
        Some(std::time::Duration::from_secs(3600)),
        "the hour-long tier refreshes on its own clock"
    );

    // The request's own model decides, as it does for `stream`; the adapter's build-time one
    // stands in only for the `unknown` placeholder
    let adaptive = ModelInfo {
        r#ref: ModelRef::new("anthropic", "claude-opus-5"),
        reasoning: true,
        thinking_controls: ThinkingControls { effort: true, ..Default::default() },
        ..Default::default()
    };
    let built_adaptive =
        AnthropicProvider::new("k".into(), None, adaptive.clone(), AnthropicCompat::default());
    assert_eq!(
        built_adaptive.prompt_cache_ttl(&turn_request(&budgeted, ThinkingLevel::High)),
        None,
        "a switch to a budgeted model is judged as that model"
    );
    assert_eq!(
        built_adaptive.prompt_cache_ttl(&turn_request(&ModelInfo::default(), ThinkingLevel::High)),
        five,
        "the placeholder falls back to the adapter's adaptive model"
    );
}

#[test]
fn the_output_limit_is_read_only_out_of_the_rejection_that_names_it() {
    assert_eq!(
        max_tokens_limit(
            "max_tokens: 100001 > 64000, which is the maximum allowed number of output tokens \
                 for claude-sonnet-4-5-20250929"
        ),
        Some(64_000)
    );
    // Another 400 that happens to mention max_tokens teaches nothing.
    assert_eq!(max_tokens_limit("max_tokens: must be greater than thinking.budget_tokens"), None);
    assert_eq!(max_tokens_limit("prompt is too long: 210000 tokens > 200000 maximum"), None);
    assert_eq!(
        max_tokens_limit("max_tokens: 5 > 0, which is the maximum allowed number of output tokens"),
        None,
        "a zero limit is nothing to retry at"
    );
}
