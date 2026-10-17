//! Stream — copilot headers, termination and reasoning deltas.

use super::*;

/// Drives the decoder the way the live reader does: one record at a time, in arrival order.
pub(super) fn decode_records(records: &[&str]) -> Vec<ProviderEvent> {
    let mut decoder = CompatDecoder::default();
    let mut events = Vec::new();
    for record in records {
        decoder.record(record, &mut events);
    }
    decoder.finish(&mut events);
    events
}

pub(super) fn request_with(messages: Vec<Message>) -> CompletionRequest {
    CompletionRequest {
        model: luxe_core::provider::ModelInfo::default(),
        messages,
        system: None,
        tools: vec![],
        thinking: ThinkingLevel::Off,
        max_tokens: None,
        ..Default::default()
    }
}

/// Copilot's endpoint expects to be told whether a request is the person asking or the agent
/// continuing. Getting it backwards is invisible locally and shows up as throttling.
#[test]
fn copilot_headers_report_who_initiated_the_turn() {
    let compat = CompatFlags { copilot_headers: true, ..CompatFlags::default() };

    let asked = request_with(vec![Message::user("hi")]);
    let headers = copilot_request_headers(&compat, &asked);
    assert!(headers.contains(&("X-Initiator", "user")), "{headers:?}");
    assert!(headers.contains(&("Openai-Intent", "conversation-edits")));

    // A tool result means the agent is continuing on its own.
    let continuing =
        request_with(vec![Message::user("hi"), Message::tool_result("call-1", "output", false)]);
    let headers = copilot_request_headers(&compat, &continuing);
    assert!(headers.contains(&("X-Initiator", "agent")), "{headers:?}");

    // So does an assistant turn being extended.
    let assistant = request_with(vec![Message::assistant("thinking…")]);
    assert!(copilot_request_headers(&compat, &assistant).contains(&("X-Initiator", "agent")));
}

#[test]
fn copilot_flags_a_turn_that_carries_an_image() {
    let compat = CompatFlags { copilot_headers: true, ..CompatFlags::default() };
    let with_image = request_with(vec![Message {
        role: luxe_core::message::Role::User,
        content: vec![ContentBlock::Image {
            data: "…".into(),
            mime: "image/png".into(),
            name: None,
        }],
    }]);
    assert!(
        copilot_request_headers(&compat, &with_image).contains(&("Copilot-Vision-Request", "true"))
    );

    let text_only = request_with(vec![Message::user("hi")]);
    assert!(!copilot_request_headers(&compat, &text_only)
        .iter()
        .any(|(name, _)| *name == "Copilot-Vision-Request"));
}

/// Every other provider must see none of this.
#[test]
fn copilot_headers_are_absent_for_everyone_else() {
    let headers =
        copilot_request_headers(&CompatFlags::default(), &request_with(vec![Message::user("hi")]));
    assert!(headers.is_empty(), "{headers:?}");
}

/// `include_usage` puts the token counts in a chunk *after* `finish_reason`, and the turn loop
/// stops on the first `Done` — so `Done` has to wait for `[DONE]`.
#[test]
fn trailing_usage_still_arrives_before_done() {
    let events = decode_records(&[
        r#"data: {"choices":[{"index":0,"delta":{"content":"hi"}}]}"#,
        r#"data: {"choices":[{"index":0,"finish_reason":"stop"}]}"#,
        r#"data: {"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":5,"total_tokens":105}}"#,
        "data: [DONE]",
    ]);
    let done = events.iter().position(|e| matches!(e, ProviderEvent::Done { .. })).unwrap();
    let usage = events.iter().position(|e| matches!(e, ProviderEvent::Usage(_))).unwrap();
    assert!(usage < done, "usage must precede Done so the engine processes it: {events:?}");
    assert_eq!(
        events.iter().filter(|e| matches!(e, ProviderEvent::Done { .. })).count(),
        1,
        "exactly one Done: {events:?}"
    );
}

/// A body that ends without the `[DONE]` sentinel still has to terminate the turn.
#[test]
fn a_body_without_the_done_sentinel_still_terminates() {
    let events = decode_records(&[
        r#"data: {"choices":[{"index":0,"delta":{"content":"hi"}}]}"#,
        r#"data: {"choices":[{"index":0,"finish_reason":"length"}]}"#,
    ]);
    assert!(
        matches!(events.last(), Some(ProviderEvent::Done { reason: StopReason::Length })),
        "{events:?}"
    );
}

/// The reported failure, in the one shape that produced it: a **tool-call** turn whose body
/// ends without `[DONE]`.
///
/// A `tool_calls` finish used to emit `ToolCallEnd` and nothing else, so the decoder had no
/// deferred `Done` to flush and the stream simply stopped. The engine reads its stop reason
/// off that event, so the turn ended reporting *nothing* — and the truncation warning, the
/// only thing that reads the reason, could not fire on the turn that needed it. Every stream
/// terminates now, whatever the body does.
#[test]
fn a_tool_call_turn_terminates_however_its_body_ends() {
    let call = r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"read","arguments":"{}"}}]}}]}"#;
    let finish = r#"data: {"choices":[{"index":0,"finish_reason":"tool_calls"}]}"#;

    for (label, records) in [
        ("no sentinel", vec![call, finish]),
        ("with sentinel", vec![call, finish, "data: [DONE]"]),
        // OpenRouter repeats the finish chunk to carry usage; one turn is still one `Done`.
        ("repeated finish", vec![call, finish, finish, "data: [DONE]"]),
    ] {
        let events = decode_records(&records);
        let dones: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                ProviderEvent::Done { reason } => Some(reason.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(dones, vec![StopReason::ToolCall], "{label}: {events:?}");
    }
}

/// The same model reached three ways must report the same reason. Anthropic Messages and the
/// Responses API both answer `ToolCall`; this adapter answered `EndTurn`, so a front-end
/// reading the stop reason saw the provider, not the turn.
#[test]
fn a_tool_call_finish_reports_a_tool_call_not_an_end_of_turn() {
    let events = decode_records(&[
        r#"data: {"choices":[{"index":0,"finish_reason":"tool_calls"}]}"#,
        "data: [DONE]",
    ]);
    assert!(
        matches!(events.last(), Some(ProviderEvent::Done { reason: StopReason::ToolCall })),
        "{events:?}"
    );
}

/// An empty body is still a finished call — the engine has to be able to stop waiting.
#[test]
fn a_body_with_nothing_in_it_still_terminates() {
    let events = decode_records(&[]);
    assert!(
        matches!(events.as_slice(), [ProviderEvent::Done { reason: StopReason::EndTurn }]),
        "{events:?}"
    );
}

/// Reasoning has no standard field name on this API; both spellings map to thinking.
#[test]
fn reasoning_deltas_are_decoded_under_either_field_name() {
    let deepseek =
        decode_records(&[r#"data: {"choices":[{"delta":{"reasoning_content":"pondering"}}]}"#]);
    let openrouter =
        decode_records(&[r#"data: {"choices":[{"delta":{"reasoning":"pondering"}}]}"#]);
    for events in [deepseek, openrouter] {
        assert!(
            events.iter().any(|event| matches!(
                event,
                ProviderEvent::ThinkingDelta { delta, .. } if delta == "pondering"
            )),
            "{events:?}"
        );
    }
}
