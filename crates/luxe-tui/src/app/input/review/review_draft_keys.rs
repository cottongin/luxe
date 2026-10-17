//! The post preview's `d`: the agent drafts the review's summary, and it opens for the reader.

use crate::app::{App, Msg};
use crate::fixture::{seeded_app, FixtureScenario};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use luxe_core::engine::{Action, DraftTarget};
use luxe_core::event::Event;
use luxe_core::guided_review::FindingAnchor;
use tokio::sync::mpsc;

fn press(app: &mut App, code: KeyCode) {
    app.update(Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn post_preview() -> (App, mpsc::Receiver<Action>) {
    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewPullRequestPost).expect("fixture app");
    let (engine_tx, engine_rx) = mpsc::channel(16);
    app.engine_tx = engine_tx;
    (app, engine_rx)
}

/// The post preview as drawn, flattened to text.
fn preview_text(app: &mut App) -> String {
    use ratatui::{backend::TestBackend, Terminal};
    let mut terminal = Terminal::new(TestBackend::new(140, 44)).expect("test terminal");
    terminal.draw(|frame| crate::view::render(frame, app)).expect("render");
    terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect()
}

/// `d` asks for the summary with the verdict chosen and the line comments that post with it; the
/// draft becomes the summary, read whole in the preview, where `e` opens it and nothing posts.
#[test]
fn d_drafts_the_summary_and_e_edits_it() {
    let (mut app, mut engine_rx) = post_preview();
    press(&mut app, KeyCode::Char('d'));
    let Ok(Action::DraftGithubReply(request)) = engine_rx.try_recv() else {
        panic!("a draft is asked for");
    };
    let DraftTarget::Review(review) = &request.target else { panic!("a review's summary") };
    assert_eq!(review.verdict, "Request changes", "the verdict chosen");
    assert!(!review.comments.is_empty(), "the line comments that post with it");
    let post = app.guided_review_workspace.github_post.as_ref().expect("still open");
    assert_eq!(post.drafting.as_deref(), Some(request.request_id.as_str()));
    let waiting = preview_text(&mut app);
    assert!(
        waiting.contains("is drafting the review"),
        "the wait is drawn where the summary will land: {waiting}"
    );

    press(&mut app, KeyCode::Char('d'));
    assert!(engine_rx.try_recv().is_err(), "once at a time");

    let drafted = "Two small things inline; otherwise this reads well.";
    app.update(Msg::Engine(Event::GithubReplyDrafted {
        request_id: request.request_id.clone(),
        body: Some(drafted.into()),
        sources: vec!["this session".into()],
        error: None,
        note: Some("I haven't read the diff myself this turn.".into()),
    }));
    // The review already has a summary, so the drafted one rewrites it rather than adding a second
    let summary = app
        .review
        .session()
        .and_then(|session| session.findings.iter().find(|f| f.anchor == FindingAnchor::Review))
        .expect("the fixture's summary")
        .clone();
    let Ok(Action::EditReview {
        request_id,
        edit: luxe_core::engine::ReviewEdit::EditComment { finding_id, title, body, .. },
        ..
    }) = engine_rx.try_recv()
    else {
        panic!("the summary goes to the review desk");
    };
    assert_eq!(finding_id, summary.id);
    assert_eq!(body, drafted);
    // What the desk stores for it, answering this edit
    let mut session = app.review.session.clone().expect("session");
    let stored = session.findings.iter_mut().find(|f| f.id == finding_id).expect("summary");
    stored.title = title;
    stored.body = body;
    app.update(Msg::Engine(Event::ReviewSessionSaved { request_id, session: Box::new(session) }));
    assert!(app.guided_review_workspace.finding_draft.is_none(), "it lands in the preview");
    let post = app.guided_review_workspace.github_post.as_ref().expect("still open");
    assert!(!post.posting && post.drafting.is_none(), "drafting posts nothing");
    let landed = preview_text(&mut app);
    assert!(landed.contains("Two small things inline"), "the summary reads whole: {landed}");
    assert!(
        landed.contains("Agent's note") && landed.contains("read the diff myself"),
        "what the agent said beside it is read with it: {landed}"
    );
    assert!(!landed.contains("is drafting"), "and the wait is over: {landed}");

    press(&mut app, KeyCode::Char('e'));
    let draft = app.guided_review_workspace.finding_draft.as_ref().expect("e opens it");
    assert_eq!(draft.body(), drafted);
}

/// A draft that fails says why, and leaves the preview ready to ask again.
#[test]
fn a_failed_draft_says_why() {
    let (mut app, mut engine_rx) = post_preview();
    press(&mut app, KeyCode::Char('d'));
    let Ok(Action::DraftGithubReply(request)) = engine_rx.try_recv() else { panic!("asked") };
    app.update(Msg::Engine(Event::GithubReplyDrafted {
        request_id: request.request_id,
        body: None,
        sources: Vec::new(),
        error: Some("The answer had no luxe-draft block to read.".into()),
        note: None,
    }));
    assert!(app.guided_review_workspace.finding_draft.is_none());
    let post = app.guided_review_workspace.github_post.as_ref().expect("still open");
    assert_eq!(post.drafting, None, "ready to ask again");
}

/// The drafting turn's brief is one line in the transcript, not a page in the reader's colors;
/// what the reader types is theirs as written.
#[test]
fn a_brief_is_one_line_in_the_transcript() {
    use crate::app::ConvoBlock;
    let (tx, _rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());
    app.update(Msg::Engine(Event::SessionLoaded { session_id: "s".into(), messages: Vec::new() }));
    for (entry, text) in [
        ("brief", "Draft the summary of my review of pull request #197, which I will post as …"),
        ("mine", "Draft a summary for me later, please."),
    ] {
        app.update(Msg::Engine(Event::MessageCommitted {
            session_id: "s".into(),
            entry_id: luxe_core::session::EntryId(entry.into()),
            message: luxe_core::message::Message {
                role: luxe_core::message::Role::User,
                content: vec![luxe_core::message::ContentBlock::Text(text.into())],
            },
        }));
    }
    let blocks: Vec<&ConvoBlock> = app.convo.iter().map(|item| item.block()).collect();
    assert!(matches!(blocks[0], ConvoBlock::System(line) if line == "Drafted the review summary"));
    assert!(
        matches!(blocks[1], ConvoBlock::User { text, .. } if text.starts_with("Draft a summary"))
    );
}
