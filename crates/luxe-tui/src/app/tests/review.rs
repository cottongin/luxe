//! Review — the review surface as the app drives it: sources, refreshes, saves and restores.

use super::*;

#[test]
fn guided_review_toggle_clamps_a_split_horizontal_anchor_for_unified_view() {
    let mut app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewReady)
            .expect("built-in review fixture should be valid");
    app.guided_review.as_mut().unwrap().files[0].hunks[0].lines[0].text = "x".repeat(400);
    app.update(Msg::TerminalResized { width: 160, height: 24 });
    // `auto` is the default, so a wide review already opens side by side.
    assert!(review_is_split(&app));

    for _ in 0..100 {
        app.update(Msg::GuidedReviewColumnNext);
    }
    let split_offset = app.guided_review_workspace.diff_column_offset;
    assert!(split_offset > 305);

    // 104 diff cells, less 9 for a unified row's bar, two-digit numbers and sign: 95 of source
    app.update(Msg::GuidedReviewToggleSplit);
    assert_eq!(app.guided_review_workspace.diff_column_offset, 305);
}

#[test]
fn guided_review_resize_clamps_a_split_horizontal_anchor_for_unified_view() {
    let mut app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewReady)
            .expect("built-in review fixture should be valid");
    app.guided_review.as_mut().unwrap().files[0].hunks[0].lines[0].text = "x".repeat(400);
    app.update(Msg::TerminalResized { width: 160, height: 24 });
    assert!(review_is_split(&app), "auto opens side by side");
    for _ in 0..100 {
        app.update(Msg::GuidedReviewColumnNext);
    }
    assert!(app.guided_review_workspace.diff_column_offset > 311);

    app.update(Msg::TerminalResized { width: 100, height: 24 });
    assert!(!review_is_split(&app), "the pane can no longer seat two columns");
    assert_eq!(
        app.guided_review_workspace.layout,
        luxe_core::config::DiffLayout::Auto,
        "and the preference was not rewritten on its behalf"
    );
    assert_eq!(app.guided_review_workspace.diff_column_offset, 311);
}

/// Whether the review on screen is drawing two columns.
pub(super) fn review_is_split(app: &crate::app::App) -> bool {
    let files = app.guided_review.as_ref().map(|review| review.files.as_slice()).unwrap_or(&[]);
    app.guided_review_workspace.split_layout_showing(files)
}

/// A save closes the draft only when it is the draft's own; any other save for the review on
/// screen is taken without touching what the reader is typing.
///
/// The break this catches: the old save chain discarded the open draft on every landed save, which
/// with the desk saving on its own (a dispatch, an agent report) would eat a comment mid-sentence.
#[test]
fn review_saves_close_only_their_own_draft() {
    let mut app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewReady)
            .expect("fixture app");
    let review_id = app.guided_review.as_ref().expect("review").id.clone();
    app.update(Msg::GuidedReviewBeginFinding);
    let mut stored = app.review.session.clone().expect("review session");

    // Something the desk did by itself: taken, and the draft stays open
    stored.revision = 3;
    app.update(Msg::Engine(Event::ReviewSessionSaved {
        request_id: ReviewSessionSaveRequestId::new(),
        session: Box::new(stored.clone()),
    }));
    assert_eq!(app.review.session().map(|session| session.revision), Some(3));
    assert!(app.guided_review_workspace.finding_draft.is_some(), "the draft survives");

    // A save for another review is not this one's
    let mut elsewhere = stored.clone();
    elsewhere.id = luxe_core::guided_review::ReviewId::new();
    elsewhere.revision = 9;
    app.update(Msg::Engine(Event::ReviewSessionSaved {
        request_id: ReviewSessionSaveRequestId::new(),
        session: Box::new(elsewhere),
    }));
    assert_eq!(app.review.session().map(|session| session.revision), Some(3));

    // The draft's own refusal lands on the draft, not in the transcript
    let refused = ReviewSessionSaveRequestId::new();
    app.review.edits.insert(refused.clone(), crate::app::ReviewEditPurpose::Draft);
    let convo_len = app.convo.len();
    app.update(Msg::Engine(Event::ReviewSessionSaveFailed {
        request_id: refused,
        review_id: review_id.clone(),
        message: "That line is not in the review as it stands now.".into(),
    }));
    assert_eq!(
        app.guided_review_workspace.finding_draft.as_ref().and_then(|d| d.error.as_deref()),
        Some("That line is not in the review as it stands now.")
    );
    assert_eq!(app.convo.len(), convo_len);

    // And its own save closes it
    let own = ReviewSessionSaveRequestId::new();
    app.review.edits.insert(own.clone(), crate::app::ReviewEditPurpose::Draft);
    stored.revision = 4;
    app.update(Msg::Engine(Event::ReviewSessionSaved {
        request_id: own,
        session: Box::new(stored),
    }));
    assert!(app.guided_review_workspace.finding_draft.is_none());
    assert!(app.review.edits.is_empty());

    // A refusal nobody here is waiting on is said in the transcript
    app.update(Msg::Engine(Event::ReviewSessionSaveFailed {
        request_id: ReviewSessionSaveRequestId::new(),
        review_id,
        message: "The review's comments could not be saved, so nothing changed.".into(),
    }));
    assert!(matches!(
        app.convo.last().map(|item| item.block()),
        Some(ConvoBlock::System(message))
            if message == "[review feedback error] The review's comments could not be saved, so nothing changed."
    ));
}

/// The review the engine restores at startup is taken, with its session and where the reading
/// stopped, under a request this front end never sent — and never over a review the reader asked
/// for.
///
/// The defect this replaces: only requested loads were accepted, so the restored review and every
/// comment in it were dropped on each restart.
#[test]
fn a_review_restored_at_startup_comes_back_with_its_comments() {
    let app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewFindings)
            .expect("fixture app");
    let review = app.guided_review.as_deref().cloned().expect("review");
    let mut session = app.review.session.clone().expect("session with comments");
    assert!(!session.findings.is_empty());
    session.progress.walked_chapter_ids = vec![review.chapters[0].id.clone()];

    let (tx, _rx) = mpsc::channel(8);
    let mut restarted = App::new(tx, KeyMap::defaults(), Config::default());
    restarted.update(Msg::Engine(Event::GuidedReviewLoaded {
        request_id: ReviewLoadRequestId::new(),
        review: Box::new(review.clone()),
    }));
    restarted
        .update(Msg::Engine(Event::ReviewSessionRestored { session: Box::new(session.clone()) }));
    assert_eq!(restarted.guided_review.as_ref().map(|r| &r.id), Some(&review.id));
    assert_eq!(restarted.review.session, Some(session));
    assert!(restarted.guided_review_workspace.is_chapter_walked(&review.chapters[0].id));

    // With a review of the reader's own on screen, another unasked-for one is not taken
    let mut other = review.clone();
    other.id = luxe_core::guided_review::ReviewId::new();
    restarted.update(Msg::Engine(Event::GuidedReviewLoaded {
        request_id: ReviewLoadRequestId::new(),
        review: Box::new(other),
    }));
    assert_eq!(restarted.guided_review.as_ref().map(|r| &r.id), Some(&review.id));
}

#[test]
fn guided_review_events_ignore_stale_results_and_apply_the_active_request() {
    use luxe_core::event::{GuidedReviewFailure, GuidedReviewFailureKind};
    use luxe_core::guided_review::{
        CheckSummary, GuidedReview, ReviewContext, ReviewId, ReviewRevisionId, ReviewStats,
        ReviewTarget,
    };

    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.update(Msg::Engine(Event::LastTurnAvailable {
        session_id: "session-1".into(),
        turn_id: "turn-1".into(),
    }));
    assert_eq!(app.review.last_turn, Some(("session-1".into(), "turn-1".into())));
    let active = ReviewLoadRequestId::new();
    let stale = ReviewLoadRequestId::new();
    app.active_guided_review_request = Some(active.clone());
    app.update(Msg::Engine(Event::GuidedReviewLoading {
        request_id: active.clone(),
        target: ReviewTarget::WorkingTree,
    }));
    // The transcript says nothing about a review being prepared: the tray above the composer is
    // already saying it, where the reader is, instead of at whatever point the stream reached.
    let quiet = app.convo.len();

    app.update(Msg::Engine(Event::GuidedReviewLoaded {
        request_id: stale.clone(),
        review: Box::new(empty_guided_review()),
    }));
    assert!(app.guided_review.is_none());

    app.update(Msg::Engine(Event::GuidedReviewLoaded {
        request_id: active.clone(),
        review: Box::new(empty_guided_review()),
    }));
    assert!(app.guided_review.is_some());
    assert!(!app.guided_review_workspace.noise_details_visible);
    assert_eq!(app.convo.len(), quiet, "and nothing about it landing, either");

    let expected_revision = app.guided_review.as_ref().unwrap().revision_id.clone();
    app.guided_review_workspace.toggle_noise_details();
    super::commands::refresh_guided_review(&mut app);
    let Action::RefreshGuidedReview {
        request_id: refresh_request,
        expected_revision: refresh_revision,
        ..
    } = rx.try_recv().unwrap()
    else {
        panic!("refreshing a pinned review should send an optimistic request");
    };
    assert_eq!(refresh_revision, expected_revision);
    assert_eq!(app.active_guided_review_request, Some(refresh_request.clone()));
    let pinned_id = app.guided_review.as_ref().unwrap().id.clone();
    app.update(Msg::Engine(Event::GuidedReviewFailed {
        request_id: refresh_request,
        target: ReviewTarget::WorkingTree,
        error: GuidedReviewFailure {
            kind: GuidedReviewFailureKind::Capture,
            message: "pinned review source could not be verified".into(),
        },
    }));
    assert!(app.active_guided_review_request.is_none());
    assert_eq!(app.guided_review.as_ref().map(|review| &review.id), Some(&pinned_id));
    assert!(app.guided_review_workspace.noise_details_visible);
    assert!(matches!(
        app.convo.last().map(|item| item.block()),
        Some(ConvoBlock::System(text))
            if text.contains("[review error] pinned review source could not be verified")
    ));

    super::commands::start_guided_review(&mut app, ReviewTarget::WorkingTree);
    let Action::LoadGuidedReview { request_id: next, .. } = rx.try_recv().unwrap() else {
        panic!("starting a replacement review should send a request");
    };
    assert!(app.guided_review.is_some(), "keep the pinned review visible while refreshing");

    let convo_len = app.convo.len();
    app.update(Msg::Engine(Event::GuidedReviewLoaded {
        request_id: active.clone(),
        review: Box::new(empty_guided_review()),
    }));
    assert_eq!(app.active_guided_review_request, Some(next.clone()));
    assert!(app.guided_review_workspace.noise_details_visible);

    app.update(Msg::Engine(Event::GuidedReviewFailed {
        request_id: active,
        target: ReviewTarget::WorkingTree,
        error: GuidedReviewFailure {
            kind: GuidedReviewFailureKind::Capture,
            message: "late failure".into(),
        },
    }));
    assert_eq!(app.active_guided_review_request, Some(next.clone()));
    assert_eq!(app.convo.len(), convo_len);

    app.update(Msg::Engine(Event::GuidedReviewLoaded {
        request_id: next,
        review: Box::new(empty_guided_review()),
    }));
    assert!(app.active_guided_review_request.is_none());
    assert!(!app.guided_review_workspace.noise_details_visible);

    fn empty_guided_review() -> GuidedReview {
        let revision_id = ReviewRevisionId::from_canonical_bytes(b"");
        GuidedReview {
            conversation: None,
            id: ReviewId::new(),
            revision_id: revision_id.clone(),
            target: ReviewTarget::WorkingTree,
            source: crate::fixture::fixture_source(revision_id),
            context: ReviewContext {
                title: "No changes".into(),
                goal: None,
                phase_index: None,
                phase_title: None,
                planning_excerpt: None,
                acceptance_excerpt: None,
                source_intent: None,
                source_commits: 0,
                pull_request: None,
            },
            files: Vec::new(),
            chapters: Vec::new(),
            checks: CheckSummary::default(),
            stats: ReviewStats::default(),
            generated_at_ms: 0,
            insight: None,
            provenance: None,
            seen: None,
            brief: None,
        }
    }
}

#[test]
fn terminal_resize_updates_the_stored_workspace_classification() {
    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());

    for (width, expected) in [
        (79, WorkspaceLayout::Compact),
        (80, WorkspaceLayout::Standard),
        (135, WorkspaceLayout::Standard),
        (136, WorkspaceLayout::Wide),
    ] {
        app.update(Msg::TerminalResized { width, height: 40 });
        assert_eq!(app.workspace_layout, expected);
    }
}

#[test]
fn dock_visibility_actions_use_the_current_layout_without_touching_composer_input() {
    use crate::workspace_dock::DockRuntimeOverride;

    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.update(Msg::TerminalResized { width: 136, height: 40 });
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT)));
    assert_eq!(app.dock.runtime_override, DockRuntimeOverride::Hidden);
    assert!(app.editor.current_input().is_empty());

    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT)));
    assert_eq!(app.dock.runtime_override, DockRuntimeOverride::Shown);
    assert!(app.dispatch_action(UiAction::HideDock));
    assert_eq!(app.dock.runtime_override, DockRuntimeOverride::Hidden);
    assert!(app.dispatch_action(UiAction::ShowDock));
    assert_eq!(app.dock.runtime_override, DockRuntimeOverride::Shown);
    assert!(app.dispatch_action(UiAction::ResetDockAuto));
    assert_eq!(app.dock.runtime_override, DockRuntimeOverride::None);
}

#[test]
fn dock_keys_do_not_intercept_composer_input() {
    // The dock is informational: no keyboard focus, so ordinary keys always reach the editor.
    use luxe_core::workspace::{ProjectIdentity, WorkspaceScopeId};

    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    let mut workspace = WorkspaceSnapshot::empty(
        WorkspaceScopeId::new("dock-keys"),
        ProjectIdentity::non_git("/project".into(), "project", "/project"),
    );
    workspace.todos.total = 1;
    app.workspace = Some(workspace);
    app.dock.show();

    app.update(Msg::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)));
    app.update(Msg::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    assert_eq!(
        app.editor.current_input(),
        " ",
        "space types into the composer; Tab is not a dock key"
    );
}

#[test]
fn available_last_turn_source_dispatches_its_pinned_target() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new(tx, luxe_core::keymap::KeyMap::defaults(), Config::default());
    app.review.last_turn = Some(("session-7".into(), "turn-12".into()));

    app.update(Msg::GuidedReviewToggleSourcePicker);
    app.update(Msg::GuidedReviewSourcePickerNext);
    app.update(Msg::GuidedReviewSelectSourcePicker);

    assert!(matches!(
        rx.try_recv(),
        Ok(Action::LoadGuidedReview {
            target: luxe_core::guided_review::ReviewTarget::LastTurn { session_id, turn_id },
            ..
        }) if session_id == "session-7" && turn_id == "turn-12"
    ));
}

#[test]
fn failed_guided_review_change_source_dispatches_a_replacement_load() {
    let seeded =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewReady)
            .expect("built-in review fixture should be valid");
    let review = seeded.guided_review.expect("fixture review");
    let (tx, mut rx) = mpsc::channel(1);
    let mut app = App::new(tx, luxe_core::keymap::KeyMap::defaults(), Config::default());
    app.guided_review = Some(review);
    app.guided_review_workspace.mark_failed(
        luxe_core::guided_review::ReviewTarget::WorkingTree,
        luxe_core::event::GuidedReviewFailure {
            kind: luxe_core::event::GuidedReviewFailureKind::Capture,
            message: "capture failed".into(),
        },
    );

    app.update(Msg::GuidedReviewToggleSourcePicker);
    app.update(Msg::GuidedReviewSelectSourcePicker);

    assert!(matches!(
        rx.try_recv(),
        Ok(Action::LoadGuidedReview {
            target: luxe_core::guided_review::ReviewTarget::WorkingTree,
            ..
        })
    ));
    assert!(matches!(
        app.guided_review_workspace.load_state,
        crate::guided_review::GuidedReviewLoadState::Loading {
            target: luxe_core::guided_review::ReviewTarget::WorkingTree
        }
    ));
    assert!(app.guided_review.is_some(), "replacement loading must keep the pinned review");
}

#[test]
fn refresh_guided_review_keeps_the_pinned_revision_until_replacement_arrives() {
    let mut app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewFindings)
            .expect("fixture app");
    let (engine_tx, mut engine_rx) = mpsc::channel(1);
    app.engine_tx = engine_tx;
    let review = app.guided_review.clone().expect("pinned review");

    super::commands::refresh_guided_review(&mut app);

    assert_eq!(app.guided_review, Some(review.clone()));
    let Action::RefreshGuidedReview { review_id, expected_revision, .. } =
        engine_rx.try_recv().expect("refresh action")
    else {
        panic!("refresh must use the pinned review as its optimistic baseline");
    };
    assert_eq!(review_id, review.id);
    assert_eq!(expected_revision, review.revision_id);
    assert!(matches!(
        app.guided_review_workspace.load_state,
        crate::guided_review::GuidedReviewLoadState::Loading { .. }
    ));
}

#[test]
fn failed_guided_review_retry_dispatches_a_refresh_only_from_failed_state() {
    let seeded =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewReady)
            .expect("built-in review fixture should be valid");
    let review = seeded.guided_review.expect("fixture review");
    let (tx, mut rx) = mpsc::channel(1);
    let mut app = App::new(tx, luxe_core::keymap::KeyMap::defaults(), Config::default());
    app.guided_review = Some(review);
    app.guided_review_workspace.mark_failed(
        luxe_core::guided_review::ReviewTarget::WorkingTree,
        luxe_core::event::GuidedReviewFailure {
            kind: luxe_core::event::GuidedReviewFailureKind::Capture,
            message: "capture failed".into(),
        },
    );

    app.update(Msg::GuidedReviewRetry);

    assert!(matches!(rx.try_recv(), Ok(Action::RefreshGuidedReview { .. })));
    app.guided_review_workspace.mark_ready();
    app.update(Msg::GuidedReviewRetry);
    assert!(rx.try_recv().is_err(), "ready reviews must not treat r as retry");
}

/// A paste big enough to bury the conversation is previewed, not dropped: the transcript keeps
/// the full text (so a copy and a re-send get everything) and the renderer collapses it until the
/// reader asks for the rest.
#[tokio::test]
async fn a_large_pasted_message_is_previewed_but_kept_whole() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());

    app.status.model = "test-model".into();
    let payload = (1..=40).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
    app.update(Msg::PasteText(payload.clone()));
    app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));

    match rx.try_recv() {
        Ok(Action::SubmitUserMessage { content, .. }) => assert_eq!(content, payload),
        other => panic!("expected the expanded paste, got {other:?}"),
    }
    let Some(item) = app.convo.first() else {
        panic!("the submission is in the transcript");
    };
    let (id, ConvoBlock::User { text, .. }) = (&item.id, item.block()) else {
        panic!("the submission is in the transcript");
    };
    assert_eq!(text, &payload, "the transcript keeps every line");

    // Collapsed by default; selecting it and pressing Enter reveals the rest, and so does a click.
    let id = id.clone();
    assert!(!app.message_is_expanded(&id));
    app.selected_block = Some(0);
    assert!(app.toggle_selected_tool_details(), "a previewed message is expandable");
    assert!(app.message_is_expanded(&id));
    app.toggle_selected_tool_details();
    assert!(!app.message_is_expanded(&id), "and collapses again");

    assert!(app.toggle_block_details_at(0), "a click on the message expands it too");
    assert!(app.message_is_expanded(&id));
}

/// `E` in a review opens the file under the cursor, at its line in the new file, and says so
/// instead when the working tree does not have it.
#[test]
fn a_review_opens_the_file_under_the_cursor() {
    let mut app =
        crate::fixture::seeded_app("luxe", crate::fixture::FixtureScenario::GuidedReviewReady)
            .expect("the fixture");
    let review = app.guided_review.as_ref().expect("a review");
    let (file, line) =
        app.guided_review_workspace.selected_new_line(&review.files).expect("a file");
    let relative = file.new_path.clone().expect("a new path");
    assert!(line.is_some(), "the cursor is on a line of the new file");

    let dir = tempfile::tempdir().unwrap();
    app.project_root = Some(dir.path().to_path_buf());
    app.update(Msg::DispatchUiAction(luxe_core::keymap::UiAction::ReviewOpenInEditor));
    assert!(app.outbox.editor_open.is_none(), "not in this working tree: nothing to open");

    let path = dir.path().join(&relative);
    std::fs::create_dir_all(path.parent().expect("a folder")).unwrap();
    std::fs::write(&path, "").unwrap();
    // The key itself, the way a terminal reports a capital
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('E'), KeyModifiers::SHIFT)));
    assert_eq!(app.outbox.editor_open.take(), Some(crate::launcher::OpenInEditor { path, line }));
}

/// A removed line opens where it used to be: the next line of the new file that survived it.
#[test]
fn a_removed_line_opens_at_the_line_that_followed_it() {
    use luxe_core::guided_review::{
        FileId, HunkId, LineRange, NoiseKind, ReviewHunk, ReviewLine, ReviewLineKind,
    };
    let line = |kind, old_line: Option<u32>, new_line: Option<u32>| ReviewLine {
        kind,
        old_line,
        new_line,
        text: String::new(),
    };
    let file = FileId::from_path_lineage(None, Some(std::path::Path::new("src/lib.rs")));
    let hunk = ReviewHunk {
        id: HunkId::from_content(&file, "a", "b"),
        old_range: LineRange::new(10, 2),
        new_range: LineRange::new(10, 2),
        heading: None,
        symbol: None,
        lines: vec![
            line(ReviewLineKind::Context, Some(10), Some(10)),
            line(ReviewLineKind::Deletion, Some(11), None),
            line(ReviewLineKind::Addition, None, Some(11)),
        ],
        noise: NoiseKind::Semantic,
    };
    assert_eq!(crate::guided_review::new_side_line(&hunk, 0), Some(10));
    assert_eq!(crate::guided_review::new_side_line(&hunk, 1), Some(11), "where it used to be");
    assert_eq!(crate::guided_review::new_side_line(&hunk, 2), Some(11));
}

/// Between a review loading and its session arriving, the screen shows the right comments: a
/// refresh keeps the ones it has, and a different review never shows the last one's.
///
/// The breaks this catches, both visible: a refresh blanking every comment for a moment and
/// forgetting the walk, and a new review drawing the previous review's comments on its own lines.
#[test]
fn a_loading_review_shows_its_own_comments_until_its_session_arrives() {
    let mut app =
        crate::fixture::seeded_app("nord", crate::fixture::FixtureScenario::GuidedReviewFindings)
            .expect("fixture app");
    let review = app.guided_review.as_deref().cloned().expect("review");
    let comments = app.review.session().expect("session").findings.len();
    assert!(comments > 0);
    app.review.bound = true;

    let mut refreshed = review.clone();
    refreshed.revision_id =
        luxe_core::guided_review::ReviewRevisionId::from_canonical_bytes(b"next");
    let refresh = ReviewLoadRequestId::new();
    app.active_guided_review_request = Some(refresh.clone());
    app.update(Msg::Engine(Event::GuidedReviewLoaded {
        request_id: refresh,
        review: Box::new(refreshed),
    }));
    assert_eq!(app.review.session().map(|s| s.findings.len()), Some(comments));
    assert!(app.review.bound, "the walk on screen is kept across a refresh");

    let mut other = review;
    other.id = luxe_core::guided_review::ReviewId::new();
    let load = ReviewLoadRequestId::new();
    app.active_guided_review_request = Some(load.clone());
    app.update(Msg::Engine(Event::GuidedReviewLoaded {
        request_id: load,
        review: Box::new(other.clone()),
    }));
    let session = app.review.session().expect("a placeholder");
    assert_eq!(session.id, other.id);
    assert!(session.findings.is_empty(), "none of the last review's comments");
    assert!(!app.review.bound);
}

/// Every flagged hunk stays on the reading list, however few of them the call-outs have room
/// for. The call-outs show two of a kind and count the rest; the reading list must not lose the
/// rest along with them, even where each hunk has an excuse that would otherwise fold it away.
#[test]
fn every_flagged_hunk_stays_on_the_reading_list_past_the_call_out_cap() {
    let review =
        crate::fixture::flagged_review(&["src/a.rs", "src/b.rs", "src/c.rs", "src/d.rs"], &[]);
    let (tx, _rx) = mpsc::channel(8);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.update(Msg::Engine(Event::GuidedReviewLoaded {
        request_id: ReviewLoadRequestId::new(),
        review: Box::new(review.clone()),
    }));

    assert!(app.review.attention.iter().any(|signal| signal.aggregate), "the call-outs are capped");
    let dropped: Vec<String> = review
        .files
        .iter()
        .filter(|file| file.hunks.iter().any(|hunk| !app.review.budget.hunk_needs_review(&hunk.id)))
        .filter_map(|file| file.new_path.as_ref().map(|path| path.display().to_string()))
        .collect();
    assert!(dropped.is_empty(), "flagged but folded away: {dropped:?}");
}
