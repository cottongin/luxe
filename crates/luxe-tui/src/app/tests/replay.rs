//! What a transcript shows of a user message, live and on resume: the reader's words, not the
//! notes and context that ride along for the model.

use super::*;

/// A resumed transcript shows what the user wrote, and none of the notes that rode with it.
#[test]
fn a_replayed_mode_note_is_not_a_user_message() {
    let (mut app, _rx) = mode_app();
    let plan = luxe_core::mode::Mode::plan(Vec::new());
    let message = luxe_core::message::Message {
        role: luxe_core::message::Role::User,
        content: vec![
            luxe_core::ContentBlock::Text("plan the limiter".into()),
            luxe_core::ContentBlock::Text(plan.entered()),
            luxe_core::ContentBlock::Text(plan.reminder().unwrap()),
        ],
    };
    app.update(Msg::Engine(Event::SessionLoaded {
        session_id: "s".into(),
        messages: vec![luxe_core::event::SessionMessage {
            entry_id: luxe_core::session::EntryId::new(),
            message,
        }],
    }));
    let blocks: Vec<_> = app.convo.iter().map(|item| item.block().clone()).collect();
    assert!(matches!(&blocks[0], ConvoBlock::User { text, .. } if text == "plan the limiter"));
    assert_eq!(blocks.len(), 1, "a mode note is the model's, not the reader's: {blocks:#?}");
}

/// Context a hook added to a user message is the model's, not something the reader typed: a
/// resumed session shows the message alone.
///
/// The break this catches: replay made every text block of a user message its own item, so after
/// a resume a `UserPromptSubmit` hook's context showed as a second message in the reader's style.
#[test]
fn a_replayed_hook_context_is_not_a_user_message() {
    let (mut app, _rx) = mode_app();
    let message = luxe_core::message::Message {
        role: luxe_core::message::Role::User,
        content: vec![
            luxe_core::ContentBlock::Text("fix the limiter".into()),
            luxe_core::ContentBlock::Text("[hook] branch main, 3 files changed".into()),
        ],
    };
    app.update(Msg::Engine(Event::SessionLoaded {
        session_id: "s".into(),
        messages: vec![luxe_core::event::SessionMessage {
            entry_id: luxe_core::session::EntryId::new(),
            message,
        }],
    }));
    let blocks: Vec<_> = app.convo.iter().map(|item| item.block().clone()).collect();
    assert!(matches!(&blocks[0], ConvoBlock::User { text, .. } if text == "fix the limiter"));
    assert_eq!(blocks.len(), 1, "a hook's context is the model's, not the reader's: {blocks:#?}");
}

/// The live path skips it too, so a message committed with hook context draws the same as it
/// will on resume.
#[test]
fn a_committed_hook_context_is_not_a_user_message() {
    let (mut app, _rx) = mode_app();
    app.session_id = Some("s".into());
    app.update(Msg::Engine(Event::MessageCommitted {
        session_id: "s".into(),
        entry_id: luxe_core::session::EntryId::new(),
        message: luxe_core::message::Message {
            role: luxe_core::message::Role::User,
            content: vec![
                luxe_core::ContentBlock::Text("also check the tests".into()),
                luxe_core::ContentBlock::Text("[hook] policy reminder".into()),
            ],
        },
    }));
    let users =
        app.convo.iter().filter(|item| matches!(item.block(), ConvoBlock::User { .. })).count();
    assert_eq!(users, 1);
}
