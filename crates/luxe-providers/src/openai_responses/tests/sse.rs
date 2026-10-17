//! SSE — events decoded and whole turns streamed.

use super::*;

#[test]
fn sse_function_call_added() {
    let sse = concat!(
        "event: response.output_item.added\n",
        "data: {\"output_index\":2,\"item\":{\"type\":\"function_call\",",
        "\"call_id\":\"call_xyz\",\"name\":\"bash\"}}\n\n"
    );
    let events = parse_responses_sse(sse);
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0],
        ProviderEvent::ToolCallStart { index: 2, id, name }
        if id == "call_xyz" && name == "bash"
    ));
}

#[test]
fn sse_function_call_arguments_delta() {
    let sse = concat!(
        "event: response.function_call_arguments.delta\n",
        "data: {\"output_index\":2,\"delta\":\"{\\\"cmd\\\":\\\"ls\\\"}\"}\n\n"
    );
    let events = parse_responses_sse(sse);
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0],
        ProviderEvent::ToolCallDelta { index: 2, json_delta }
        if json_delta.contains("cmd")
    ));
}

#[test]
fn sse_tool_call_start_and_delta_same_index() {
    // CRITICAL: ToolCallStart and ToolCallDelta must carry the same index.
    let sse = concat!(
        "event: response.output_item.added\n",
        "data: {\"output_index\":3,\"item\":{\"type\":\"function_call\",",
        "\"call_id\":\"call_99\",\"name\":\"read\"}}\n\n",
        "event: response.function_call_arguments.delta\n",
        "data: {\"output_index\":3,\"delta\":\"{\\\"path\\\":\\\"a.rs\\\"}\"}\n\n",
    );
    let events = parse_responses_sse(sse);
    assert_eq!(events.len(), 2);
    let start_index = match &events[0] {
        ProviderEvent::ToolCallStart { index, .. } => *index,
        other => panic!("expected ToolCallStart, got {other:?}"),
    };
    let delta_index = match &events[1] {
        ProviderEvent::ToolCallDelta { index, .. } => *index,
        other => panic!("expected ToolCallDelta, got {other:?}"),
    };
    assert_eq!(
        start_index, delta_index,
        "ToolCallStart and ToolCallDelta must share the same index"
    );
}

#[test]
fn sse_non_function_call_output_item_skipped() {
    // output_item.added where item.type != "function_call" should be silently ignored.
    let sse = concat!(
        "event: response.output_item.added\n",
        "data: {\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"msg_1\"}}\n\n"
    );
    let events = parse_responses_sse(sse);
    assert!(events.is_empty());
}

#[test]
fn sse_reasoning_summary_delta() {
    let sse = "event: response.reasoning_summary_text.delta\ndata: {\"delta\":\"thinking...\"}\n\n";
    let events = parse_responses_sse(sse);
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], ProviderEvent::ThinkingDelta { index: 0, delta } if delta == "thinking...")
    );
}

/// A summary that arrives in several parts is several paragraphs, and the deltas carry no
/// separator between them — so concatenating them ran the end of one into the start of the
/// next. The break goes before every part but the first, or each reasoning block would open
/// with a blank line.
#[test]
fn sse_reasoning_summary_parts_are_separated_but_never_led_by_a_break() {
    let sse = concat!(
        "event: response.reasoning_summary_part.added
",
        "data: {\"output_index\":0,\"summary_index\":0}

",
        "event: response.reasoning_summary_text.delta
",
        "data: {\"output_index\":0,\"delta\":\"First, the parser.\"}

",
        "event: response.reasoning_summary_part.added
",
        "data: {\"output_index\":0,\"summary_index\":1}

",
        "event: response.reasoning_summary_text.delta
",
        "data: {\"output_index\":0,\"delta\":\"Then the writer.\"}

",
    );

    let thinking: String = parse_responses_sse(sse)
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::ThinkingDelta { delta, .. } => Some(delta.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(thinking, "First, the parser.\n\nThen the writer.");
    assert!(!thinking.starts_with('\n'), "the first part opens the block: {thinking:?}");
}

#[test]
fn sse_completed_end_turn() {
    let sse = concat!(
        "event: response.completed\n",
        "data: {\"response\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":5},",
        "\"output\":[{\"type\":\"message\"}]}}\n\n"
    );
    let events = parse_responses_sse(sse);
    assert_eq!(events.len(), 2); // Usage + Done
    assert!(
        matches!(&events[0], ProviderEvent::Usage(u) if u.prompt_tokens == 10 && u.completion_tokens == 5 && u.total_tokens == 15)
    );
    assert!(matches!(&events[1], ProviderEvent::Done { reason: StopReason::EndTurn }));
}

#[test]
fn sse_completed_tool_call() {
    let sse = concat!(
        "event: response.completed\n",
        "data: {\"response\":{\"usage\":{\"input_tokens\":20,\"output_tokens\":8},",
        "\"output\":[{\"type\":\"function_call\"}]}}\n\n"
    );
    let events = parse_responses_sse(sse);
    assert_eq!(events.len(), 2); // Usage + Done
    assert!(matches!(&events[1], ProviderEvent::Done { reason: StopReason::ToolCall }));
}

#[test]
fn sse_completed_no_usage() {
    let sse = "event: response.completed\ndata: {\"response\":{\"output\":[]}}\n\n";
    let events = parse_responses_sse(sse);
    assert_eq!(events.len(), 1); // Just Done, no Usage
    assert!(matches!(&events[0], ProviderEvent::Done { reason: StopReason::EndTurn }));
}

#[test]
fn sse_usage_captures_cache_and_reasoning_details() {
    let sse = concat!(
        "event: response.completed\n",
        "data: {\"response\":{\"usage\":{\"input_tokens\":2000,\"output_tokens\":300,",
        "\"input_tokens_details\":{\"cached_tokens\":1500},",
        "\"output_tokens_details\":{\"reasoning_tokens\":250}},\"output\":[]}}\n\n",
    );
    let events = parse_responses_sse(sse);
    let usage = events
        .iter()
        .find_map(|e| match e {
            ProviderEvent::Usage(u) => Some(u),
            _ => None,
        })
        .expect("a Usage event");
    assert_eq!(usage.prompt_tokens, 2000);
    assert_eq!(usage.cache_read_tokens, 1500);
    assert_eq!(usage.reasoning_tokens, 250);
}

/// A write to the cache is billed, so it has to be read off the usage block rather than
/// assumed absent. The literal is a real `gpt-6-luna` response's usage, from the harness eval's
/// meter: it was priced as plain input, and the session's cost came out short.
#[test]
fn a_cache_write_is_read_off_the_responses_usage() {
    let sse = concat!(
        "event: response.completed\n",
        "data: {\"response\":{\"usage\":{\"input_tokens\": 7196, \"input_tokens_details\": ",
        "{\"cache_write_tokens\": 128, \"cached_tokens\": 7065}, \"output_tokens\": 46, ",
        "\"output_tokens_details\": {\"reasoning_tokens\": 0}, \"total_tokens\": 7242},",
        "\"output\":[]}}\n\n",
    );
    let usage = parse_responses_sse(sse)
        .into_iter()
        .find_map(|e| match e {
            ProviderEvent::Usage(u) => Some(u),
            _ => None,
        })
        .expect("a Usage event");
    assert_eq!((usage.cache_read_tokens, usage.cache_write_tokens), (7065, 128));
    assert_eq!(usage.prompt_tokens, 7196, "the prompt still counts everything it held");
}

#[test]
fn sse_error_event() {
    let sse = "event: error\ndata: {\"message\":\"rate limit exceeded\"}\n\n";
    let events = parse_responses_sse(sse);
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], ProviderEvent::Error { message, .. } if message == "rate limit exceeded")
    );
}

#[test]
fn sse_response_failed() {
    let sse = concat!(
        "event: response.failed\n",
        "data: {\"response\":{\"error\":{\"message\":\"model overloaded\"}}}\n\n"
    );
    let events = parse_responses_sse(sse);
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], ProviderEvent::Error { message, .. } if message == "model overloaded")
    );
}

#[test]
fn sse_unknown_event_type_ignored() {
    let sse = "event: response.some.future.event\ndata: {\"foo\":\"bar\"}\n\n";
    let events = parse_responses_sse(sse);
    assert!(events.is_empty());
}

#[test]
fn sse_full_turn_stream() {
    // Simulate a complete text-only turn.
    let sse = concat!(
        "event: response.output_text.delta\n",
        "data: {\"delta\":\"Hello\"}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"delta\":\", world!\"}\n\n",
        "event: response.completed\n",
        "data: {\"response\":{\"usage\":{\"input_tokens\":5,\"output_tokens\":3},",
        "\"output\":[{\"type\":\"message\"}]}}\n\n",
    );
    let events = parse_responses_sse(sse);
    // TextDelta("Hello"), TextDelta(", world!"), Usage, Done
    assert_eq!(events.len(), 4);
    assert!(matches!(&events[0], ProviderEvent::TextDelta { delta, .. } if delta == "Hello"));
    assert!(matches!(&events[1], ProviderEvent::TextDelta { delta, .. } if delta == ", world!"));
    assert!(matches!(&events[2], ProviderEvent::Usage(_)));
    assert!(matches!(&events[3], ProviderEvent::Done { reason: StopReason::EndTurn }));
}

#[test]
fn sse_full_tool_call_stream() {
    // Simulate a turn that ends with a tool call.
    let sse = concat!(
        "event: response.output_item.added\n",
        "data: {\"output_index\":0,\"item\":{\"type\":\"function_call\",",
        "\"call_id\":\"call_1\",\"name\":\"bash\"}}\n\n",
        "event: response.function_call_arguments.delta\n",
        "data: {\"output_index\":0,\"delta\":\"{\\\"cmd\\\":\\\"ls\\\"}\"}\n\n",
        "event: response.completed\n",
        "data: {\"response\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":4},",
        "\"output\":[{\"type\":\"function_call\"}]}}\n\n",
    );
    let events = parse_responses_sse(sse);
    // ToolCallStart, ToolCallDelta, Usage, Done(ToolCall)
    assert_eq!(events.len(), 4);
    assert!(
        matches!(&events[0], ProviderEvent::ToolCallStart { index: 0, id, name } if id == "call_1" && name == "bash")
    );
    assert!(matches!(&events[1], ProviderEvent::ToolCallDelta { index: 0, .. }));
    assert!(matches!(&events[2], ProviderEvent::Usage(_)));
    assert!(matches!(&events[3], ProviderEvent::Done { reason: StopReason::ToolCall }));
}

// -----------------------------------------------------------------------
// In-place tool loading
// -----------------------------------------------------------------------

pub(super) fn schema(name: &str) -> ToolSchema {
    ToolSchema {
        name: name.to_string(),
        description: format!("The {name} tool."),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
    }
}
