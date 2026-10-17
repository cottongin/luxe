use super::*;
use crate::message::Role;

fn assistant(blocks: Vec<ContentBlock>) -> Message {
    Message { role: Role::Assistant, content: blocks }
}

fn tool_result(call_id: &str, content: Vec<ToolOutput>) -> Message {
    Message {
        role: Role::Tool,
        content: vec![ContentBlock::ToolResult {
            call_id: call_id.to_string(),
            content,
            is_error: false,
        }],
    }
}

/// The shape that bricked a real session, copied out of its `.luxe/sessions/*.jsonl`: a turn
/// whose stream was dropped before it produced anything committed one empty text block, and
/// every request after it came back `400 text content blocks must be non-empty`.
#[test]
fn the_empty_reply_that_ended_a_session_is_dropped_with_its_message() {
    let mut messages = vec![
        Message::user("Strata is open."),
        assistant(vec![ContentBlock::Text(String::new())]),
        Message::user("Please continue."),
    ];

    let scrub = scrub(&mut messages);

    assert_eq!(scrub.blocks_dropped, 1);
    assert_eq!(scrub.messages_dropped, 1, "an emptied message is its own 400");
    assert_eq!(messages.len(), 2);
    assert!(messages.iter().all(|message| message.role == Role::User));
}

/// Whitespace is not content: Anthropic rejects `" "` for the same reason it rejects `""`.
#[test]
fn a_whitespace_only_block_counts_as_empty() {
    let mut messages = vec![assistant(vec![
        ContentBlock::Text("  \n ".into()),
        ContentBlock::Text("real answer".into()),
    ])];

    let scrub = scrub(&mut messages);

    assert_eq!(scrub.blocks_dropped, 1);
    assert!(!scrub.is_clean(), "a transcript that had to be changed was not clean");
    assert_eq!(messages[0].content.len(), 1);
}

/// A result that came back with no output at all: nothing to drop, something to fill.
#[test]
fn a_result_with_no_output_at_all_is_still_answered() {
    let mut messages = vec![tool_result("call_a", vec![])];

    let scrub = scrub(&mut messages);

    assert_eq!(scrub, Scrub { results_filled: 1, ..Scrub::default() });
    assert!(!scrub.is_clean());
}

/// An image pasted with no caption is an ordinary thing to send, and the empty caption that
/// rides along with it is the whole message's undoing.
#[test]
fn an_image_with_no_caption_keeps_its_image() {
    let mut messages = vec![Message {
        role: Role::User,
        content: vec![
            ContentBlock::Text(String::new()),
            ContentBlock::Image {
                data: "iVBOR".into(),
                mime: "image/png".into(),
                name: Some("shot.png".into()),
            },
        ],
    }];

    let scrub = scrub(&mut messages);

    assert_eq!(scrub.blocks_dropped, 1);
    assert_eq!(scrub.messages_dropped, 0, "the picture is the message");
    assert_eq!(messages[0].content.len(), 1);
}

/// A result with nothing in it is filled, never dropped: dropping it would strand the call it
/// answers, which is the same 400 through the other door.
#[test]
fn an_empty_tool_result_is_filled_rather_than_dropped() {
    let mut messages = vec![tool_result("call_a", vec![ToolOutput::Text(String::new())])];

    let scrub = scrub(&mut messages);

    assert_eq!(scrub.results_filled, 1);
    assert_eq!(messages.len(), 1, "the result stays, so its call keeps its answer");
    let ContentBlock::ToolResult { content, .. } = &messages[0].content[0] else {
        panic!("expected a tool result");
    };
    assert_eq!(content, &[ToolOutput::Text(NO_OUTPUT.to_string())]);
}

/// A screenshot's pixels are output even when its text is not.
#[test]
fn a_result_that_is_only_an_image_is_left_alone() {
    let image = ToolOutput::Image {
        data: "iVBOR".into(),
        mime: "image/png".into(),
        name: Some("shot.png".into()),
    };
    let mut messages = vec![tool_result("call_a", vec![ToolOutput::Text("".into()), image])];

    let scrub = scrub(&mut messages);

    assert_eq!(scrub.blocks_dropped, 1);
    assert_eq!(scrub.results_filled, 0);
    let ContentBlock::ToolResult { content, .. } = &messages[0].content[0] else {
        panic!("expected a tool result");
    };
    assert_eq!(content.len(), 1);
}

/// A sendable transcript comes out untouched. The outgoing copy is an append to the last
/// request's, so rewriting the prefix re-bills the conversation at the cache write rate.
#[test]
fn a_sendable_transcript_is_left_byte_identical() {
    let original = vec![
        Message::user("go"),
        assistant(vec![
            ContentBlock::Text("working".into()),
            ContentBlock::ToolCall {
                id: "call_a".into(),
                name: "read".into(),
                arguments: serde_json::json!({"path": "src/lib.rs"}),
            },
        ]),
        tool_result("call_a", vec![ToolOutput::Text("fn main() {}".into())]),
    ];
    let mut messages = original.clone();

    assert!(scrub(&mut messages).is_clean());
    assert_eq!(messages, original);
}

/// Idempotent, for the same reason: the second pass over a scrubbed transcript must not move
/// a single byte, or the prefix would differ between two turns of one session.
#[test]
fn scrubbing_twice_changes_nothing_the_second_time() {
    let mut messages = vec![
        Message::user("look"),
        assistant(vec![ContentBlock::Text(String::new())]),
        tool_result("call_a", vec![]),
    ];

    assert!(!scrub(&mut messages).is_clean());
    let once = messages.clone();
    assert!(scrub(&mut messages).is_clean());
    assert_eq!(messages, once);
}
