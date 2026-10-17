//! SSE — event decoding and usage.

use super::*;

#[test]
fn test_parse_sse_event_text_delta() {
    let event_str = r#"event: content_block_delta
data: {"type": "content_block_delta", "delta": {"type": "text_delta", "text": "Hello"}}"#;

    let event = parse_sse_event(event_str).unwrap();
    match event {
        AnthropicSseEvent::ContentBlockDelta(delta) => {
            assert_eq!(delta.delta_type, "content_block_delta");
            let inner = delta.delta.unwrap();
            assert_eq!(inner.inner_type, "text_delta");
            assert_eq!(inner.text.unwrap(), "Hello");
        }
        _ => panic!("Expected ContentBlockDelta"),
    }
}

#[test]
fn sse_maps_signature_delta_and_redacted_thinking() {
    let text = concat!(
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"reasoning\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"SIG==\"}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"redacted_thinking\",\"data\":\"ENC==\"}}\n\n",
    );
    let events = parse_anthropic_sse_text(text);
    assert!(
        events.iter().any(|e| matches!(e, ProviderEvent::ThinkingSignature { index: 0, signature } if signature == "SIG==")),
        "signature_delta maps to ThinkingSignature: {events:?}"
    );
    assert!(
        events.iter().any(
            |e| matches!(e, ProviderEvent::RedactedThinking { index: 1, data } if data == "ENC==")
        ),
        "redacted_thinking start maps to RedactedThinking: {events:?}"
    );
}

#[test]
fn test_parse_sse_event_tool_call_start() {
    let event_str = r#"event: content_block_start
data: {"type": "content_block_start", "content_block": {"type": "tool_use", "id": "toolu_123", "name": "bash"}}"#;

    let event = parse_sse_event(event_str).unwrap();
    match event {
        AnthropicSseEvent::ContentBlockStart(block) => {
            assert_eq!(block.block_type, "content_block_start");
            // The id and name are in the nested content_block field
            // For now, just verify we got a ContentBlockStart event
        }
        _ => panic!("Expected ContentBlockStart"),
    }
}

#[test]
fn test_parse_sse_event_message_delta() {
    let event_str = r#"event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 1}}"#;

    let event = parse_sse_event(event_str).unwrap();
    match event {
        AnthropicSseEvent::MessageDelta(delta) => {
            let stop_reason = delta
                .stop_reason
                .or_else(|| delta.delta.as_ref().and_then(|d| d.stop_reason.clone()));
            assert_eq!(stop_reason.unwrap(), "end_turn");
        }
        _ => panic!("Expected MessageDelta"),
    }
}

#[test]
fn test_parse_anthropic_sse_text_multiple_events() {
    let text = r#"event: content_block_start
data: {"type": "content_block_start", "content_block": {"type": "text", "id": "block_1"}}

event: content_block_delta
data: {"type": "content_block_delta", "delta": {"type": "text_delta", "text": "Hello"}}

event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 1}}"#;

    let events = parse_anthropic_sse_text(text);
    // Two text deltas, then a Usage event (from message_delta), then Done.
    assert_eq!(events.len(), 4);
    assert!(events.iter().any(|e| matches!(e, ProviderEvent::Usage(_))));
}

#[test]
fn test_anthropic_usage_normalizes_input_and_cache() {
    // message_start carries input + cache; message_delta carries the final output count.
    let text = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":500,",
        "\"cache_read_input_tokens\":4000,\"cache_creation_input_tokens\":200,",
        "\"output_tokens\":1}}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},",
        "\"usage\":{\"output_tokens\":320}}",
    );
    let events = parse_anthropic_sse_text(text);
    let usage = events
        .iter()
        .find_map(|e| match e {
            ProviderEvent::Usage(u) => Some(u),
            _ => None,
        })
        .expect("a Usage event");
    // prompt_tokens is normalized to all input: 500 uncached + 4000 read + 200 write.
    assert_eq!(usage.prompt_tokens, 4700);
    assert_eq!(usage.cache_read_tokens, 4000);
    assert_eq!(usage.cache_write_tokens, 200);
    assert_eq!(usage.completion_tokens, 320);
    assert_eq!(usage.total_tokens, 5020);
}

/// Writes kept for an hour are split out of the write count, because they cost 2x input rather
/// than 1.25x. The shape is Copilot's `/v1/messages` reply, copied from a live one.
#[test]
fn hour_long_cache_writes_are_counted_apart() {
    let text = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"cache_creation\":",
        "{\"ephemeral_1h_input_tokens\":9813,\"ephemeral_5m_input_tokens\":0},",
        "\"cache_creation_input_tokens\":9813,\"cache_read_input_tokens\":0,",
        "\"input_tokens\":7,\"output_tokens\":1}}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},",
        "\"usage\":{\"output_tokens\":10}}",
    );
    let events = parse_anthropic_sse_text(text);
    let usage = events
        .iter()
        .find_map(|e| match e {
            ProviderEvent::Usage(u) => Some(u),
            _ => None,
        })
        .expect("a Usage event");
    assert_eq!((usage.cache_write_tokens, usage.cache_write_1h_tokens), (9813, 9813));
}

/// Thinking is billed inside `output_tokens` and broken out in `output_tokens_details`, the only
/// place Anthropic reports the reasoning share of a turn.
#[test]
fn anthropic_usage_carries_the_thinking_share_of_the_output() {
    let text = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":105,",
        "\"output_tokens\":1}}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},",
        "\"usage\":{\"output_tokens\":4000,\"output_tokens_details\":{\"thinking_tokens\":3999}}}",
    );
    let events = parse_anthropic_sse_text(text);
    let usage = events
        .iter()
        .find_map(|e| match e {
            ProviderEvent::Usage(u) => Some(u),
            _ => None,
        })
        .expect("a Usage event");
    assert_eq!(usage.completion_tokens, 4_000);
    assert_eq!(usage.reasoning_tokens, 3_999, "the reasoning subset of the output");
}
