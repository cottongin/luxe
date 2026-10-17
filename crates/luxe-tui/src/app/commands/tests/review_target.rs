//! Review targets — which review a command asks for, and the keys inside it.

use super::*;

/// Ctrl+G asks for a review; it does not arrive on one.
///
/// The chord used to switch surface immediately and leave the reader watching a capture that had
/// not finished — and then a guide rewriting the prose under them. It now starts the work and says
/// so in the tray; a second press, once that row offers, is what opens it.
#[test]
fn ctrl_g_asks_for_a_review_and_a_second_press_opens_the_ready_one() {
    use luxe_core::keymap::KeyName;
    // The confirmed default chord: Ctrl+G opens Guided Review.
    assert_eq!(
        KeyMap::defaults().resolve(true, false, false, &KeyName::Char('g')),
        Some(UiAction::ReviewOpen)
    );

    // The receiver is kept alive: a dropped one turns the capture request into "engine
    // unavailable" and the tray row is cleared, which is right and is not what this is about.
    let (mut app, _engine_rx) =
        crate::fixture::seeded_session("nord", FixtureScenario::ChatIdle).expect("chat fixture");
    assert_eq!(app.current_view, crate::app::ViewMode::Chat);
    assert!(dispatch_action(&mut app, UiAction::ReviewOpen));
    assert_eq!(
        app.current_view,
        crate::app::ViewMode::Chat,
        "the conversation is where the waiting is visible"
    );
    assert_eq!(
        app.review.queued.as_ref().map(|queued| queued.state),
        Some(crate::app::QueuedReviewState::Capturing)
    );

    app.review.queued.as_mut().expect("queued").state = crate::app::QueuedReviewState::Ready;
    assert!(dispatch_action(&mut app, UiAction::ReviewOpen));
    assert_eq!(app.current_view, crate::app::ViewMode::GuidedReview, "and now it opens");
}

#[test]
fn diff_target_parser_accepts_only_bounded_source_forms() {
    assert_eq!(parse_review_target(None, None), Ok(ReviewTarget::WorkingTree));
    assert_eq!(
        parse_review_target(Some("base origin/main"), None),
        Ok(ReviewTarget::Base { reference: "origin/main".into() })
    );
    assert_eq!(
        parse_review_target(Some("commit deadbeef"), None),
        Ok(ReviewTarget::Commit { oid: "deadbeef".into() })
    );
    let last_turn = ("session-1".into(), "turn-1".into());
    assert_eq!(
        parse_review_target(Some("turn"), Some(&last_turn)),
        Ok(ReviewTarget::LastTurn { session_id: "session-1".into(), turn_id: "turn-1".into() })
    );
    assert_eq!(parse_review_target(Some("turn"), None), Err(LAST_TURN_UNAVAILABLE));
    for invalid in ["base", "patch @@ -1", "base ref extra", "base \u{0007}"] {
        assert_eq!(parse_review_target(Some(invalid), None), Err(REVIEW_TARGET_USAGE));
    }
}

/// A pull request by number, by `#number`, or by the address a browser shows, including GitHub
/// Enterprise and the tail a copied URL tends to carry.
#[test]
fn a_pull_request_is_named_the_way_a_reader_would() {
    let checkout = |number| Ok(ReviewTarget::PullRequest { number, repository: None });
    let upstream = |number, slug: &str| {
        Ok(ReviewTarget::PullRequest { number, repository: Some(slug.into()) })
    };
    assert_eq!(parse_review_target(Some("pr 123"), None), checkout(123));
    assert_eq!(parse_review_target(Some("pr #7"), None), checkout(7));
    for (url, number, slug) in [
        ("https://github.com/acme/app/pull/42", 42, "acme/app"),
        ("github.com/acme/app/pull/42/files", 42, "acme/app"),
        ("https://github.com/acme/app/pull/42?w=1#discussion_r1", 42, "acme/app"),
        ("https://ghe.example.com/Platform/Tools/pull/9/commits", 9, "Platform/Tools"),
    ] {
        assert_eq!(parse_review_target(Some(&format!("pr {url}")), None), upstream(number, slug));
    }
    for invalid in [
        "pr",
        "pr 0",
        "pr -3",
        "pr #",
        "pr abc",
        "pr 12 34",
        "pr https://github.com/acme/app/issues/42",
        "pr https://github.com/acme/app/pull/",
        "pr https://github.com/acme/app/pull/x",
        "pr 99999999999999999999999",
    ] {
        assert_eq!(parse_review_target(Some(invalid), None), Err(REVIEW_TARGET_USAGE), "{invalid}");
    }
}

/// A failed refresh must not lose where the reading had got to.
#[test]
fn refresh_command_preserves_walked_chapters_after_a_delivery_failure() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let chapter_id = app.guided_review.as_ref().expect("fixture review").chapters[0].id.clone();
    app.guided_review_workspace.walked_chapters.insert(chapter_id.clone());

    refresh_guided_review(&mut app);

    assert!(app.guided_review_workspace.is_chapter_walked(&chapter_id));
    assert!(matches!(app.guided_review_workspace.load_state, GuidedReviewLoadState::Failed { .. }));
}

#[tokio::test]
async fn diff_without_arguments_loads_the_working_tree_target() {
    let (tx, mut rx) = mpsc::channel(1);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    let command = crate::commands::CommandRegistry::all()
        .parse_slash("/diff")
        .expect("diff command should parse");

    run(&mut app, &command);

    let Some(Action::LoadGuidedReview { request_id, target }) = rx.recv().await else {
        panic!("diff should send a guided review request");
    };
    assert_eq!(target, ReviewTarget::WorkingTree);
    assert_eq!(app.active_guided_review_request, Some(request_id));
    // `/diff` is the same request the key makes, so it waits the same way: the capture starts, the
    // tray says so, and the surface opens when there is a whole review to open.
    assert_eq!(app.current_view, crate::app::ViewMode::Chat);
    assert_eq!(
        app.review.queued.as_ref().map(|queued| queued.state),
        Some(crate::app::QueuedReviewState::Capturing)
    );
    assert_eq!(
        app.guided_review_workspace.load_state,
        GuidedReviewLoadState::Loading { target: ReviewTarget::WorkingTree }
    );
}

/// A review asked for mid-turn waits where the reader can see it waiting.
///
/// The engine will not capture one while a turn is running — `turn.rs` re-queues anything it
/// cannot act on — so switching to the review surface here put the reader in front of a
/// full-screen "Capturing the working tree" that had not started, with the agent still producing
/// the change they wanted to read and no way to watch it. Nothing about the surface said so.
#[tokio::test]
async fn a_review_asked_for_mid_turn_waits_in_the_tray_instead_of_taking_the_screen() {
    use crate::app::{AppState, QueuedReviewState};

    let (tx, mut rx) = mpsc::channel(4);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.status.state = AppState::Streaming;
    assert!(app.turn_in_flight());

    super::start_guided_review(&mut app, ReviewTarget::WorkingTree);
    assert_eq!(
        app.current_view,
        crate::app::ViewMode::Chat,
        "the conversation is where the turn it is waiting on is visible"
    );
    assert!(rx.try_recv().is_err(), "and nothing was asked of the engine yet");
    assert_eq!(
        app.review.queued.as_ref().map(|queued| queued.state),
        Some(QueuedReviewState::Waiting)
    );

    // The turn ends: now there is a revision worth pinning, so the capture goes out — and still
    // does not take the screen, because the reader has been in the conversation the whole time.
    app.status.state = AppState::Idle;
    super::flush_queued_review(&mut app);
    assert!(matches!(rx.try_recv(), Ok(Action::LoadGuidedReview { .. })), "the capture starts");
    assert_eq!(app.current_view, crate::app::ViewMode::Chat);
    assert_eq!(
        app.review.queued.as_ref().map(|queued| queued.state),
        Some(QueuedReviewState::Capturing)
    );

    // Captured. The row turns into an offer, and only the reader takes it up.
    app.review.queued.as_mut().expect("queued").state = QueuedReviewState::Ready;
    assert_eq!(app.current_view, crate::app::ViewMode::Chat);
    super::start_guided_review(&mut app, ReviewTarget::WorkingTree);
    assert_eq!(app.current_view, crate::app::ViewMode::GuidedReview, "now it opens");
    assert!(app.review.queued.is_none(), "and the row has done its job");
    assert!(rx.try_recv().is_err(), "without capturing a second time");
}

/// Idle only changes when the work starts, never whether the reader is dropped into it.
#[tokio::test]
async fn a_review_asked_for_while_idle_captures_at_once_but_still_does_not_open() {
    use crate::app::QueuedReviewState;

    let (tx, mut rx) = mpsc::channel(1);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    assert!(!app.turn_in_flight());

    super::start_guided_review(&mut app, ReviewTarget::WorkingTree);
    assert!(
        matches!(rx.try_recv(), Ok(Action::LoadGuidedReview { .. })),
        "nothing to wait for, so the capture goes out immediately"
    );
    assert_eq!(
        app.current_view,
        crate::app::ViewMode::Chat,
        "but a half-built review is no better to arrive on for having been quick"
    );
    assert_eq!(
        app.review.queued.as_ref().map(|queued| queued.state),
        Some(QueuedReviewState::Capturing)
    );
}

/// The wait after a capture is the model writing the guide, which is most of it. The row moves from
/// capturing to writing when the review arrives, and to ready when the guide does.
#[tokio::test]
async fn the_review_row_follows_the_capture_then_the_guide() {
    use crate::app::QueuedReviewState;
    use luxe_core::event::Event;

    let (tx, mut rx) = mpsc::channel(1);
    let config = Config {
        notifications: Some(luxe_core::config::Notifications::All(true)),
        ..Config::default()
    };
    let mut app = App::new(tx, KeyMap::defaults(), config);
    let notifier = crate::notify::MockNotifier::new();
    let calls = notifier.calls.clone();
    app.notifier = Box::new(notifier);
    super::start_guided_review(&mut app, ReviewTarget::WorkingTree);
    let Ok(Action::LoadGuidedReview { request_id, .. }) = rx.try_recv() else {
        panic!("the capture goes out");
    };
    assert_eq!(app.review.queued.as_ref().map(|q| q.state), Some(QueuedReviewState::Capturing));
    assert_eq!(app.agent_state(), crate::herdr::AgentState::Working, "herdr hears it as work");

    let review = crate::fixture::fixture_demo_guided_review();
    let (review_id, revision_id, chapters) =
        (review.id.clone(), review.revision_id.clone(), review.chapters.clone());
    app.update(crate::app::Msg::Engine(Event::GuidedReviewLoaded {
        request_id: request_id.clone(),
        review: Box::new(review),
    }));
    assert!(
        matches!(
            app.review.queued.as_ref().map(|q| q.state),
            Some(QueuedReviewState::Writing { .. })
        ),
        "captured, and now the guide is being written"
    );

    app.update(crate::app::Msg::Engine(Event::GuidedReviewGuideUpdated {
        request_id,
        review_id,
        revision_id,
        chapters,
        provenance: luxe_core::guided_review::GuideProvenance::Model { batches: 1 },
        brief: None,
    }));
    assert_eq!(app.review.queued.as_ref().map(|q| q.state), Some(QueuedReviewState::Ready));
    // Minutes after it was asked for, which is the point of saying so
    let bodies: Vec<String> = calls.lock().unwrap().iter().map(|(_, body)| body.clone()).collect();
    assert_eq!(bodies, ["Review ready"]);
    // And the conversation keeps a line saying so, the way it keeps one for a posted review
    let said: Vec<&str> = app
        .convo
        .iter()
        .filter_map(|item| match item.block() {
            crate::app::ConvoBlock::System(line) => Some(line.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].starts_with("[review] Review of working tree is ready · "), "{said:?}");
    assert_eq!(app.agent_state(), crate::herdr::AgentState::Idle, "and done");
}

#[tokio::test]
async fn diff_turn_uses_only_the_retained_baseline_target() {
    let (tx, mut rx) = mpsc::channel(1);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    let command = crate::commands::CommandRegistry::all()
        .parse_slash("/diff turn")
        .expect("diff turn command should parse");

    run(&mut app, &command);
    assert!(rx.try_recv().is_err());
    assert!(matches!(
        app.convo.last().map(|item| item.block()),
        Some(ConvoBlock::System(message)) if message == LAST_TURN_UNAVAILABLE
    ));

    app.review.last_turn = Some(("session-1".into(), "turn-1".into()));
    run(&mut app, &command);
    let Some(Action::LoadGuidedReview { target, .. }) = rx.recv().await else {
        panic!("retained turn should send a guided review request");
    };
    assert_eq!(
        target,
        ReviewTarget::LastTurn { session_id: "session-1".into(), turn_id: "turn-1".into() }
    );
}

/// The review key on an emptied working tree opens the change, not a review of nothing.
///
/// A commit takes the diff against `HEAD` to zero at the exact moment there is most to read. The
/// turn's own baseline is retained content rather than a Git range, so it still holds the whole
/// change — and this is the key the dock's affordance routes to.
#[tokio::test]
async fn the_review_key_falls_back_to_the_retained_turn_once_the_work_is_committed() {
    use luxe_core::workspace::{ProjectIdentity, WorkspaceScopeId, WorkspaceSnapshot};

    let (tx, mut rx) = mpsc::channel(2);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.workspace = Some(WorkspaceSnapshot::empty(
        WorkspaceScopeId::new("scope-a"),
        ProjectIdentity::non_git("/project".into(), "project", "/project"),
    ));
    app.review.last_turn = Some(("session-1".into(), "turn-1".into()));

    // A project that was clean all along still means the working tree — there is no committed
    // work of this session's to fall back to.
    assert!(dispatch_action(&mut app, UiAction::ReviewOpen));
    let Some(Action::LoadGuidedReview { target, .. }) = rx.recv().await else {
        panic!("expected a review request")
    };
    assert_eq!(target, ReviewTarget::WorkingTree);

    app.current_view = crate::app::ViewMode::Chat;
    app.worktree_emptied = true;
    assert!(dispatch_action(&mut app, UiAction::ReviewOpen));
    let Some(Action::LoadGuidedReview { target, .. }) = rx.recv().await else {
        panic!("expected a review request")
    };
    assert_eq!(
        target,
        ReviewTarget::LastTurn { session_id: "session-1".into(), turn_id: "turn-1".into() },
        "the committed change is what there is to review"
    );
}

#[test]
fn review_navigation_actions_dispatch_only_inside_the_review_surface() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let (engine_tx, mut engine_rx) = mpsc::channel(1);
    app.engine_tx = engine_tx;
    let first_hunk = app.guided_review_workspace.position.hunk_id.clone();

    assert!(dispatch_action(&mut app, UiAction::ReviewHunkNext));
    assert_ne!(app.guided_review_workspace.position.hunk_id, first_hunk);

    let line_offset = app.guided_review_workspace.position.line_offset;
    assert!(dispatch_action(&mut app, UiAction::ReviewLineNext));
    assert!(app.guided_review_workspace.position.line_offset > line_offset);

    app.guided_review_workspace.diff_column_offset = 12;
    assert!(dispatch_action(&mut app, UiAction::ReviewLineStart));
    assert_eq!(app.guided_review_workspace.diff_column_offset, 0);

    assert!(dispatch_action(&mut app, UiAction::ReviewFileNext));

    assert!(app.guided_review_workspace.guide_visible);
    assert!(dispatch_action(&mut app, UiAction::ReviewToggleGuide));
    assert!(!app.guided_review_workspace.guide_visible, "the guide pane can be put away");

    // Walking the section records where the reading got to; drain it so the assertions below are
    // about the comment save.
    while engine_rx.try_recv().is_ok() {}

    assert!(dispatch_action(&mut app, UiAction::ReviewAddFinding));
    // One field: everything printable lands in the comment, `Tab` included, and the durable title
    // is derived from it.
    for character in ['R', 'e'] {
        crate::app::input::handle_key(
            &mut app,
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(character),
                crossterm::event::KeyModifiers::NONE,
            ),
        );
    }
    let draft = app.guided_review_workspace.finding_draft.as_ref().expect("open draft");
    assert_eq!(draft.body(), "Re");
    assert_eq!(draft.resolved_title(), "Re", "the title comes from the comment's first line");
    // The verb picked it and nothing in the composer can change it: `c` asks for work.
    assert_eq!(draft.severity, luxe_core::guided_review::FindingSeverity::Issue);
    assert!(app.editor.current_input().is_empty());
    crate::app::input::handle_key(
        &mut app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::CONTROL,
        ),
    );
    assert!(app.guided_review_workspace.finding_draft.is_some());
    assert_eq!(app.review.session().expect("review session").findings.len(), 0);
    let Some(Action::EditReview {
        edit: luxe_core::engine::ReviewEdit::Comment { draft, send: false },
        ..
    }) = engine_rx.try_recv().ok()
    else {
        panic!("expected the comment to be sent to the review desk");
    };
    assert_eq!(draft.body, "Re");
    assert!(app.review_edit_pending(|purpose| *purpose == crate::app::ReviewEditPurpose::Draft));
    app.update(crate::app::Msg::GuidedReviewSaveFinding);
    assert!(engine_rx.try_recv().is_err());
    crate::app::input::handle_key(
        &mut app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ),
    );
    assert!(app.guided_review_workspace.finding_draft.is_none());

    assert!(dispatch_action(&mut app, UiAction::ReviewAddFinding));
    crate::app::input::handle_key(
        &mut app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::CONTROL,
        ),
    );
    assert!(app.guided_review_workspace.finding_draft.is_some());
    assert_eq!(app.review.session().expect("review session").findings.len(), 0);
    assert!(engine_rx.try_recv().is_err());
    // A save that cannot proceed says why instead of doing nothing at all (here: the previous
    // save from this test is still in flight).
    assert_eq!(
        app.guided_review_workspace.finding_draft.as_ref().and_then(|draft| draft.error.as_deref()),
        Some("Still saving the last change \u{2014} press again in a moment.")
    );
    // Pasting into the composer stays in the composer instead of reaching the chat editor.
    app.update(crate::app::Msg::PasteText("bound the retry\nloop".into()));
    let draft = app.guided_review_workspace.finding_draft.as_ref().expect("open draft");
    assert_eq!(draft.body(), "bound the retry\nloop");
    assert!(draft.error.is_none(), "editing clears the rejection reason");
    assert!(app.editor.current_input().is_empty(), "the chat composer never saw the paste");
    crate::app::input::handle_key(
        &mut app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ),
    );
    assert!(app.guided_review_workspace.finding_draft.is_none());

    assert!(dispatch_action(&mut app, UiAction::ReviewFindings));
    assert!(app.guided_review_workspace.findings_visible);
    crate::app::input::handle_key(
        &mut app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ),
    );
    assert!(!app.guided_review_workspace.findings_visible);
    assert_eq!(app.current_view, crate::app::ViewMode::GuidedReview);

    let layout = app.guided_review_workspace.layout;
    assert!(dispatch_action(&mut app, UiAction::ReviewToggleSplit));
    assert_ne!(app.guided_review_workspace.layout, layout);

    assert!(dispatch_action(&mut app, UiAction::ReviewExit));
    assert_eq!(app.current_view, crate::app::ViewMode::Chat);
    // Going back to a review already captured is immediate: it is pinned and complete, and there
    // is nothing to wait for.
    assert!(dispatch_action(&mut app, UiAction::ReviewOpen));
    assert_eq!(app.current_view, crate::app::ViewMode::GuidedReview);
    assert!(app.review.queued.is_none(), "and nothing is re-captured to get there");
    assert_eq!(
        app.guided_review_workspace.load_state,
        crate::guided_review::GuidedReviewLoadState::Ready,
        "the review it returns to is the one it left, still ready"
    );

    app.current_view = crate::app::ViewMode::Chat;
    assert!(!dispatch_action(&mut app, UiAction::ReviewHunkPrev));
}

#[test]
fn resolving_the_selected_finding_asks_the_desk() {
    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("findings fixture app");
    let (engine_tx, mut engine_rx) = mpsc::channel(1);
    app.engine_tx = engine_tx;
    let finding_id =
        app.guided_review_workspace.selected_finding_id.clone().expect("selected finding");

    app.update(crate::app::Msg::GuidedReviewToggleSelectedFindingResolved);

    assert!(matches!(
        engine_rx.try_recv(),
        Ok(Action::EditReview { edit: luxe_core::engine::ReviewEdit::Resolve { finding_id: resolved }, .. })
            if resolved == finding_id
    ));
    assert!(app.review_edit_pending(|_| true));
}

#[test]
fn deleting_the_selected_draft_asks_the_desk() {
    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("findings fixture app");
    let draft_id = app
        .review
        .session()
        .expect("review session")
        .findings
        .iter()
        .find(|finding| finding.state == luxe_core::guided_review::FindingState::Draft)
        .expect("draft finding")
        .id
        .clone();
    app.guided_review_workspace.selected_finding_id = Some(draft_id.clone());
    let original_len = app.review.session().expect("review session").findings.len();
    let (engine_tx, mut engine_rx) = mpsc::channel(1);
    app.engine_tx = engine_tx;

    app.update(crate::app::Msg::GuidedReviewDeleteSelectedFindingDraft);

    assert!(matches!(
        engine_rx.try_recv(),
        Ok(Action::EditReview {
            edit: luxe_core::engine::ReviewEdit::DeleteComment { finding_id },
            ..
        }) if finding_id == draft_id
    ));
    assert_eq!(
        app.review.session().expect("review session").findings.len(),
        original_len,
        "gone only once the desk says so"
    );
}

#[test]
fn edited_draft_ctrl_enter_sends_the_new_wording() {
    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("findings fixture app");
    let finding = app
        .review
        .session()
        .expect("review session")
        .findings
        .iter()
        .find(|finding| finding.state == luxe_core::guided_review::FindingState::Draft)
        .expect("draft finding")
        .clone();
    let original_len = app.review.session().expect("review session").findings.len();
    let original_title = finding.title.clone();
    assert!(app.guided_review_workspace.begin_edit_finding_draft(&finding));
    app.guided_review_workspace
        .finding_draft
        .as_mut()
        .expect("local editor")
        .set_body("Keep bounded retry handling");
    let (engine_tx, mut engine_rx) = mpsc::channel(1);
    app.engine_tx = engine_tx;

    crate::app::input::handle_key(
        &mut app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::CONTROL,
        ),
    );

    let Some(Action::EditReview {
        edit: luxe_core::engine::ReviewEdit::EditComment { finding_id, title, body, .. },
        ..
    }) = engine_rx.try_recv().ok()
    else {
        panic!("expected the edit to be sent to the review desk");
    };
    assert_eq!(finding_id, finding.id, "the same comment, not a second one");
    assert_eq!(title, "Keep bounded retry handling");
    assert_eq!(body, "Keep bounded retry handling");
    assert_eq!(app.review.session().expect("review session").findings.len(), original_len);
    assert_eq!(
        app.review
            .session()
            .expect("review session")
            .findings
            .iter()
            .find(|candidate| candidate.id == finding.id)
            .expect("local draft")
            .title,
        original_title,
        "the mirror changes only when the desk answers"
    );
}

/// The composer is the one place a reviewer writes prose, and it was push/pop-char only. It now
/// runs on Luxe's real editor: a cursor you can move, word operations, and Unicode safety.
#[test]
fn the_comment_composer_supports_real_editing() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    assert!(dispatch_action(&mut app, UiAction::ReviewAddFinding));
    let key = KeyEvent::new;

    for character in "bound retries".chars() {
        crate::app::input::handle_key(&mut app, key(KeyCode::Char(character), KeyModifiers::NONE));
    }
    // Home, then type: the insertion lands at the cursor, not blindly at the end.
    crate::app::input::handle_key(&mut app, key(KeyCode::Home, KeyModifiers::NONE));
    for character in "please ".chars() {
        crate::app::input::handle_key(&mut app, key(KeyCode::Char(character), KeyModifiers::NONE));
    }
    let draft = app.guided_review_workspace.finding_draft.as_ref().expect("open draft");
    assert_eq!(draft.body(), "please bound retries");

    // Enter inserts a newline rather than firing the chat submit behind the review.
    crate::app::input::handle_key(&mut app, key(KeyCode::End, KeyModifiers::CONTROL));
    crate::app::input::handle_key(&mut app, key(KeyCode::Enter, KeyModifiers::NONE));
    crate::app::input::handle_key(&mut app, key(KeyCode::Char('x'), KeyModifiers::NONE));
    let draft = app.guided_review_workspace.finding_draft.as_ref().expect("open draft");
    assert_eq!(draft.body(), "please bound retries\nx");
    assert!(app.editor.current_input().is_empty(), "the chat composer stayed untouched");

    // Backspace and arrow movement work over multi-byte text.
    for character in "é→".chars() {
        crate::app::input::handle_key(&mut app, key(KeyCode::Char(character), KeyModifiers::NONE));
    }
    crate::app::input::handle_key(&mut app, key(KeyCode::Backspace, KeyModifiers::NONE));
    let draft = app.guided_review_workspace.finding_draft.as_ref().expect("open draft");
    assert_eq!(draft.body(), "please bound retries\nxé", "backspace removes one whole character");
}
