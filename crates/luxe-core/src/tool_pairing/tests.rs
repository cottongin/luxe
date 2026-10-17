use super::*;

fn call(id: &str) -> ContentBlock {
    ContentBlock::ToolCall {
        id: id.to_string(),
        name: "read".to_string(),
        arguments: serde_json::json!({"path": "src/lib.rs"}),
    }
}

fn result(id: &str) -> ContentBlock {
    ContentBlock::ToolResult {
        call_id: id.to_string(),
        content: vec![ToolOutput::Text("ok".to_string())],
        is_error: false,
    }
}

fn assistant(blocks: Vec<ContentBlock>) -> Message {
    Message { role: Role::Assistant, content: blocks }
}

fn tool(blocks: Vec<ContentBlock>) -> Message {
    Message { role: Role::Tool, content: blocks }
}

fn answered_ids(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect()
}

/// The shape that bricked a real session: three calls in one assistant message, no tool message
/// at all, and a user message chained straight after it.
#[test]
fn an_unanswered_batch_is_answered_in_place() {
    let mut messages = vec![
        Message::user("look at the site"),
        assistant(vec![call("call_a"), call("call_b"), call("call_c")]),
        Message::user("never mind, do this instead"),
    ];

    let repair = repair(&mut messages);

    assert_eq!(repair.calls_answered, 3);
    assert_eq!(repair.results_dropped, 0);
    assert_eq!(messages.len(), 4, "one tool message inserted");
    assert_eq!(messages[2].role, Role::Tool, "results follow the calls that made them");
    assert_eq!(answered_ids(&messages), ["call_a", "call_b", "call_c"]);
    assert_eq!(messages[3].role, Role::User, "the steer stays after the pair it interrupted");
}

/// A cancelled batch commonly gets *some* results. Those must not be duplicated, and the
/// stragglers belong in the tool message that already exists.
#[test]
fn a_partially_answered_batch_extends_the_existing_tool_message() {
    let mut messages = vec![
        Message::user("go"),
        assistant(vec![call("call_a"), call("call_b")]),
        tool(vec![result("call_a")]),
    ];

    let repair = repair(&mut messages);

    assert_eq!(repair.calls_answered, 1);
    assert_eq!(messages.len(), 3, "no second tool message");
    assert_eq!(answered_ids(&messages), ["call_a", "call_b"]);
    let ContentBlock::ToolResult { is_error, .. } = &messages[2].content[1] else {
        panic!("expected a synthesized result");
    };
    assert!(is_error, "an unfinished call is not a success");
}

/// The inverse gap: a compaction cut or a dropped assistant message strands the results.
#[test]
fn results_with_no_call_are_dropped_and_empty_messages_go_with_them() {
    let mut messages =
        vec![Message::user("go"), tool(vec![result("call_gone")]), Message::assistant("done")];

    let repair = repair(&mut messages);

    assert_eq!(repair.results_dropped, 1);
    assert_eq!(repair.calls_answered, 0);
    assert_eq!(messages.len(), 2, "the emptied tool message is removed, not left blank");
    assert!(messages.iter().all(|message| !message.content.is_empty()));
}

/// A valid transcript must come out untouched — the request prefix has to stay stable for
/// prompt caching, so "repair" may not mean "rewrite".
#[test]
fn a_valid_transcript_is_left_byte_identical() {
    let original = vec![
        Message::user("go"),
        assistant(vec![ContentBlock::Text("working".into()), call("call_a")]),
        tool(vec![result("call_a")]),
        Message::assistant("done"),
    ];
    let mut messages = original.clone();

    let repair = repair(&mut messages);

    assert!(repair.is_clean());
    assert_eq!(messages, original);
}

/// Repair is idempotent: a synthesized result satisfies the next pass, so a long session
/// carrying one interruption does not accumulate answers to the same call.
#[test]
fn repairing_twice_changes_nothing_the_second_time() {
    let mut messages = vec![Message::user("go"), assistant(vec![call("call_a")])];

    assert_eq!(repair(&mut messages).calls_answered, 1);
    let once = messages.clone();
    assert!(repair(&mut messages).is_clean());
    assert_eq!(messages, once);
}

/// Interleaved reasoning and text must survive untouched — the OpenAI Responses API and
/// Anthropic both reject a reordered assistant message during a tool-use turn.
#[test]
fn reasoning_and_text_blocks_keep_their_order() {
    let mut messages = vec![
        Message::user("go"),
        assistant(vec![
            ContentBlock::Thinking {
                text: "thinking".into(),
                signature: None,
                id: Some("rs_1".into()),
                encrypted: Some("opaque".into()),
                opaque: None,
            },
            ContentBlock::Text("here goes".into()),
            call("call_a"),
        ]),
    ];

    repair(&mut messages);

    assert!(matches!(messages[1].content[0], ContentBlock::Thinking { .. }));
    assert!(matches!(messages[1].content[1], ContentBlock::Text(_)));
    assert!(matches!(messages[1].content[2], ContentBlock::ToolCall { .. }));
}

/// The three explanations exist because they make different promises about side effects. A
/// model that reads "safe to retry" on an aborted call runs it twice.
#[test]
fn only_a_call_that_never_started_promises_no_side_effects() {
    assert!(Unfinished::NeverStarted.message().contains("no effect"));
    assert!(Unfinished::Aborted.message().contains("partially completed"));
    assert!(Unfinished::Unrecorded.message().contains("unknown"));
    for state in [Unfinished::Aborted, Unfinished::Unrecorded] {
        assert!(
            !state.message().contains("Safe to retry"),
            "{state:?} must not promise a clean slate"
        );
    }
}
