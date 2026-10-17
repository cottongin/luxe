//! SSE flows — whole streams decoded end to end.

use super::*;

#[test]
fn test_integration_streaming_conversation() {
    // Simulate a complete streaming conversation with text, thinking, and tool calls
    let sse_text = r#"event: content_block_start
data: {"type": "content_block_start", "content_block": {"type": "thinking", "id": "think_1"}}

event: content_block_delta
data: {"type": "content_block_delta", "delta": {"type": "thinking_delta", "thinking": "I need to"}}

event: content_block_delta
data: {"type": "content_block_delta", "delta": {"type": "thinking_delta", "thinking": "."}}

event: content_block_stop
data: {}

event: content_block_start
data: {"type": "content_block_start", "content_block": {"type": "text", "id": "block_1"}}

event: content_block_delta
data: {"type": "content_block_delta", "delta": {"type": "text_delta", "text": "Hello"}}

event: content_block_delta
data: {"type": "content_block_delta", "delta": {"type": "text_delta", "text": " world"}}

event: content_block_start
data: {"type": "content_block_start", "content_block": {"type": "tool_use", "id": "toolu_123", "name": "bash"}}

event: content_block_delta
data: {"type": "content_block_delta", "delta": {"type": "input_json_delta", "partial_json": "{\"command\":"}}

event: content_block_delta
data: {"type": "content_block_delta", "delta": {"type": "input_json_delta", "partial_json": ": \"ls\"}"}}

event: content_block_stop
data: {}

event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 50}}

event: message_stop
data: {}"#;

    let events = parse_anthropic_sse_text(sse_text);

    // Should have multiple events
    assert!(events.len() >= 8);

    // Verify we got thinking, text, and tool events
    let has_thinking = events.iter().any(|e| matches!(e, ProviderEvent::ThinkingDelta { .. }));
    let has_text = events.iter().any(|e| matches!(e, ProviderEvent::TextDelta { .. }));
    let has_tool_start = events.iter().any(|e| matches!(e, ProviderEvent::ToolCallStart { .. }));
    let has_tool_delta = events.iter().any(|e| matches!(e, ProviderEvent::ToolCallDelta { .. }));
    let has_done = events.iter().any(|e| matches!(e, ProviderEvent::Done { .. }));

    assert!(has_thinking, "Should have thinking events");
    assert!(has_text, "Should have text events");
    assert!(has_tool_start, "Should have tool call start");
    assert!(has_tool_delta, "Should have tool call delta");
    assert!(has_done, "Should have done event");
}

#[test]
fn tool_call_deltas_accumulate_at_the_blocks_own_index() {
    // A realistic stream: text at index 0, then a tool call at index 1 whose two
    // `input_json_delta` chunks also carry index 1. The tool args must accumulate against the
    // ToolCallStart's index (regression: a per-event counter gave the deltas a different index,
    // so args never assembled and every tool call arrived empty).
    let sse_text = r#"event: content_block_start
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Sure."}}

event: content_block_start
data: {"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "toolu_9", "name": "task"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{\"action\":"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": " \"list\"}"}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 1}"#;

    let events = parse_anthropic_sse_text(sse_text);

    // The tool start and both deltas must share the SAME index.
    let start_index = events.iter().find_map(|e| match e {
        ProviderEvent::ToolCallStart { index, name, .. } if name == "task" => Some(*index),
        _ => None,
    });
    assert_eq!(start_index, Some(1), "tool call keeps Anthropic's block index");

    // Reproduce turn.rs accumulation: append deltas whose index matches the start.
    let mut args = String::new();
    for e in &events {
        if let ProviderEvent::ToolCallDelta { index, json_delta } = e {
            if *index == 1 {
                args.push_str(json_delta);
            }
        }
    }
    assert_eq!(args, r#"{"action": "list"}"#, "tool arguments fully assembled");
    let parsed: serde_json::Value = serde_json::from_str(&args).unwrap();
    assert_eq!(parsed["action"], "list");
}

#[test]
fn test_parse_error_event() {
    let event_str = r#"event: error
data: {"type": "error", "error": {"message": "Rate limit exceeded", "type": "rate_limit_error"}}"#;

    let event = parse_sse_event(event_str).unwrap();
    match event {
        AnthropicSseEvent::Error(error) => {
            let msg = error
                .message
                .or_else(|| error.error.as_ref().and_then(|e| e.message.clone()))
                .unwrap();
            assert_eq!(msg, "Rate limit exceeded");
        }
        _ => panic!("Expected Error event"),
    }
}
