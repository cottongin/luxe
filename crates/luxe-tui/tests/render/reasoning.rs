//! Reasoning — a reasoning block keeps its shape as it commits.

use super::*;

/// Streams `parts` as reasoning deltas, one per provider block index.
pub(super) fn stream_reasoning(app: &mut App, parts: &[(usize, &str)]) {
    for (block, delta) in parts {
        app.update(Msg::Engine(luxe_core::Event::ThinkingDelta {
            block: *block,
            delta: (*delta).to_string(),
        }));
    }
}

pub(super) fn commit_reasoning(app: &mut App, parts: &[&str], answer: Option<&str>) {
    use luxe_core::message::{ContentBlock, Message, Role};
    let mut content: Vec<ContentBlock> = parts
        .iter()
        .map(|text| ContentBlock::Thinking {
            text: (*text).to_string(),
            signature: None,
            id: None,
            encrypted: None,
            opaque: None,
        })
        .collect();
    if let Some(answer) = answer {
        content.push(ContentBlock::Text(answer.to_string()));
    }
    app.update(Msg::Engine(luxe_core::Event::MessageCommitted {
        session_id: "s".into(),
        entry_id: luxe_core::session::EntryId("e1".into()),
        message: Message { role: Role::Assistant, content },
    }));
}

pub(super) fn chat_app() -> App {
    let mut app = create_app();
    app.update(Msg::Engine(luxe_core::Event::SessionLoaded {
        session_id: "s".into(),
        messages: vec![],
    }));
    app
}

/// The symptom, stated as the invariant: committing a message must not change what is on screen.
///
/// A message can carry several reasoning items. Core keeps one block per provider output index, so
/// if the stream reports them all as block 0 the front-end merges them into one item and the commit
/// has to tear the tail down and rebuild it — which the reader sees as the reasoning reflowing, its
/// spacing changing, under them.
#[test]
fn committing_a_multi_part_reasoning_message_does_not_reflow_it() {
    let backend = test_backend(100, 40);
    let mut app = chat_app();
    let parts = ["**Verifying the capture**", "Checking whether the tool is loaded yet."];

    stream_reasoning(&mut app, &[(0, parts[0]), (1, parts[1])]);
    app.reveal_catch_up();
    let streamed = rows_of(&render_app(&mut app, &backend));

    commit_reasoning(&mut app, &parts, None);
    app.reveal_catch_up();
    let committed = rows_of(&render_app(&mut app, &backend));

    assert_eq!(
        streamed, committed,
        "the transcript changed when the message committed — nothing about it should move"
    );
    assert_eq!(
        app.convo.len(),
        2,
        "each reasoning item is its own block, streamed and committed alike"
    );
}

/// Reasoning that carries no text is a replay token, not content: OpenAI returns an item holding
/// only `id`/`encrypted_content` whenever summaries are off. Drawing it would invent a block the
/// stream never produced — and inventing one is what forces the commit to rebuild the tail.
#[test]
fn reasoning_with_no_text_is_never_drawn_and_never_reflows() {
    use luxe_core::message::{ContentBlock, Message, Role};

    let backend = test_backend(100, 40);
    let mut app = chat_app();
    app.update(Msg::Engine(luxe_core::Event::TokenDelta { block: 0, delta: "Done.".into() }));
    app.reveal_catch_up();
    let streamed = rows_of(&render_app(&mut app, &backend));

    app.update(Msg::Engine(luxe_core::Event::MessageCommitted {
        session_id: "s".into(),
        entry_id: luxe_core::session::EntryId("e1".into()),
        message: Message {
            role: Role::Assistant,
            content: vec![
                // The replay token: no summary text, only what must be sent back next turn.
                ContentBlock::Thinking {
                    text: String::new(),
                    signature: None,
                    id: Some("rs_1".into()),
                    encrypted: Some("ENC==".into()),
                    opaque: None,
                },
                ContentBlock::Text("Done.".into()),
            ],
        },
    }));
    app.reveal_catch_up();

    assert_eq!(rows_of(&render_app(&mut app, &backend)), streamed, "nothing moved");
    assert_eq!(app.convo.len(), 1, "the replay token is not a transcript item");
}

/// The ordinary case has to keep working: one reasoning item, then the answer, refreshed in place.
#[test]
fn a_single_reasoning_item_still_commits_in_place() {
    let backend = test_backend(100, 40);
    let mut app = chat_app();

    stream_reasoning(&mut app, &[(0, "Thinking about it.")]);
    app.update(Msg::Engine(luxe_core::Event::TokenDelta {
        block: 0,
        delta: "Here is the answer.".into(),
    }));
    app.reveal_catch_up();
    let streamed = rows_of(&render_app(&mut app, &backend));

    commit_reasoning(&mut app, &["Thinking about it."], Some("Here is the answer."));
    app.reveal_catch_up();

    assert_eq!(rows_of(&render_app(&mut app, &backend)), streamed);
    assert_eq!(app.convo.len(), 2, "reasoning and answer, not duplicated");
}
