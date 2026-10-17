//! A question's thread, from the feedback window: `c` asks for the fix, `a` asks more.

use crate::app::{App, Msg};
use crate::fixture::{seeded_app, FixtureScenario};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use luxe_core::engine::Action;
use luxe_core::guided_review::{FindingMessageAuthor, FindingSeverity, FindingState};
use tokio::sync::mpsc;

fn press(app: &mut App, code: KeyCode) {
    app.update(Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

/// A review whose first comment is a question the agent answered, selected in the open window.
fn answered_question() -> (App, mpsc::Receiver<Action>, luxe_core::guided_review::FindingId) {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    let (engine_tx, engine_rx) = mpsc::channel(16);
    app.engine_tx = engine_tx;
    let session = app.review.session_mut().expect("review session");
    let question = &mut session.findings[0];
    question.severity = FindingSeverity::Question;
    question.state = FindingState::Open;
    question.thread.push(luxe_core::guided_review::FindingMessage {
        author: FindingMessageAuthor::Agent,
        body: "It is a bug: the router repeats the scheduler's decision.".into(),
        created_at_ms: 1,
    });
    let id = question.id.clone();
    app.guided_review_workspace.selected_finding_id = Some(id.clone());
    app.guided_review_workspace.findings_visible = true;
    (app, engine_rx, id)
}

/// `c` on an answered question opens a fix request, and saving it asks for the fix: the desk makes
/// the question an issue with the reviewer's direction at the end of its thread, which is what
/// sends it as a request for fixes rather than another question. It goes in the same save.
#[test]
fn c_on_an_answered_question_asks_for_the_fix() {
    let (mut app, mut engine_rx, id) = answered_question();
    press(&mut app, KeyCode::Char('c'));
    let draft = app.guided_review_workspace.finding_draft.as_mut().expect("a reply");
    assert_eq!(draft.reply_to.as_ref(), Some(&id));
    assert_eq!(draft.severity, FindingSeverity::Issue, "a fix request, not another question");
    draft.set_body("Fix it in the router.");
    app.update(Msg::GuidedReviewSaveFinding);

    let Ok(Action::EditReview { edit, .. }) = engine_rx.try_recv() else {
        panic!("the fix request goes to the review desk");
    };
    assert_eq!(
        edit,
        luxe_core::engine::ReviewEdit::AskForFix {
            finding_id: id,
            body: "Fix it in the router.".into(),
            send: true,
        }
    );
}

/// `a` continues the question as a question; on a comment it has no question to continue.
#[test]
fn a_asks_more_on_a_question_and_nothing_on_a_comment() {
    let (mut app, _engine_rx, id) = answered_question();
    press(&mut app, KeyCode::Char('a'));
    let draft = app.guided_review_workspace.finding_draft.as_ref().expect("a follow-up");
    assert_eq!(draft.reply_to.as_ref(), Some(&id));
    assert_eq!(draft.severity, FindingSeverity::Question);
    press(&mut app, KeyCode::Esc);

    app.guided_review_workspace.findings_visible = true;
    let comment = app.review.session().unwrap().findings[1].id.clone();
    app.guided_review_workspace.selected_finding_id = Some(comment);
    press(&mut app, KeyCode::Char('a'));
    assert!(app.guided_review_workspace.finding_draft.is_none());
}

/// A question's brief lands in the transcript as the reader's question, under where they asked
/// it, so the answer that follows it is not an answer to nothing. A fix request stays one line.
#[test]
fn a_question_lands_in_the_transcript_as_what_was_asked() {
    use crate::app::ConvoBlock;
    use luxe_core::event::Event;
    let (tx, _rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());
    app.update(Msg::Engine(Event::SessionLoaded { session_id: "s".into(), messages: Vec::new() }));
    let brief = "Guided review feedback\nSubmission: submission:1\nReview: review:2\n\
        Source revision: revision:3\nMode: Question\n\n\
        Severity meanings:\n- Question: answer it; make no edit for it\n\n\
        - Question · Why two passes?\n\
        \x20 Finding ID: finding:1\n\
        \x20 Evidence: src/a.rs:3 (new)\n\
        \x20 Why two passes?\n\
        Do not rewrite unrelated code.";
    for (entry, text) in
        [("asked", brief.to_owned()), ("fixes", brief.replace("Question\n", "RequestFixes\n"))]
    {
        app.update(Msg::Engine(Event::MessageCommitted {
            session_id: "s".into(),
            entry_id: luxe_core::session::EntryId(entry.into()),
            message: luxe_core::message::Message::user(text),
        }));
    }
    let blocks: Vec<&ConvoBlock> = app.convo.iter().map(|item| item.block()).collect();
    let ConvoBlock::User { text, chips, .. } = blocks[0] else {
        panic!("the reader's own words: {blocks:?}");
    };
    assert_eq!(text, "Asked in review · src/a.rs:3 (new)\nWhy two passes?");
    assert_eq!(
        chips.iter().map(|chip| &text[chip.clone()]).collect::<Vec<_>>(),
        ["Asked in review · src/a.rs:3 (new)"]
    );
    assert!(
        matches!(blocks[1], ConvoBlock::System(line) if line == "Sent your review comments to the agent")
    );
}
