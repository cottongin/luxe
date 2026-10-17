//! Loading — fetching, failing and refreshing, and the recovery each offers.

use super::*;

/// A failed load keeps its way out, and a failed refresh of a review on screen says why when
/// there is room to.
#[test]
fn a_failed_load_names_its_recovery_at_every_width() {
    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewRefreshing).expect("refresh fixture");
    app.guided_review_workspace.mark_failed(
        ReviewTarget::WorkingTree,
        GuidedReviewFailure { kind: GuidedReviewFailureKind::Capture, message: "gone".into() },
    );
    assert!(resolve_footer(&app, 90).0.starts_with("Refresh failed: gone"));
    assert_eq!(resolve_footer(&app, 89).0, "r retry · s change source · esc back to chat");
    assert_eq!(resolve_footer(&app, 60).0, "r retry · s change source · esc back to chat");
    assert_eq!(resolve_footer(&app, 59).0, "r retry · esc back");
    app.guided_review = None;
    assert_eq!(
        resolve_footer(&app, 120).0,
        "r retry · s change source · esc back to chat",
        "with no review behind it there is nothing to refresh"
    );
}

/// Fetching waits on the network, so the loading row says that is what it is doing.
#[test]
fn loading_a_pull_request_says_it_is_fetching() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    app.guided_review = None;
    app.guided_review_workspace.load_state = GuidedReviewLoadState::Loading {
        target: ReviewTarget::PullRequest { number: 42, repository: None },
    };
    let rows = frame_rows(&mut app, 120, 30);
    assert!(rows.iter().any(|row| row.contains("Fetching pull request #42")), "{rows:?}");
    assert!(rows.iter().any(|row| row.contains("Your working tree is not touched")), "{rows:?}");
}

#[test]
fn wide_header_shows_available_source_totals() {
    use ratatui::{backend::TestBackend, Terminal};

    let app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let review = app.guided_review.as_ref().expect("fixture review");
    let additions = review.stats.additions.expect("fixture additions");
    let deletions = review.stats.deletions.expect("fixture deletions");
    let backend = TestBackend::new(160, 32);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render wide header");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    let files = review.stats.files;
    let noun = if files == 1 { "file" } else { "files" };
    assert!(text.contains(&format!("{files} {noun}")));
    assert!(text.contains(&format!("+{additions} −{deletions}")));
}

#[test]
fn findings_drawer_offers_cancellation_only_for_a_queued_current_source_submission() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    let finding_id = app
        .review
        .session()
        .expect("review session")
        .findings
        .first()
        .expect("fixture finding")
        .id
        .clone();
    app.review
        .session_mut()
        .expect("review session")
        .queue_submission(
            luxe_core::guided_review::SubmissionId::new(),
            luxe_core::guided_review::ReviewFeedbackMode::RequestFixes,
            std::slice::from_ref(&finding_id),
            &[],
            0,
        )
        .expect("queue submission");
    app.guided_review_workspace.selected_finding_id = Some(finding_id);
    app.guided_review_workspace.findings_visible = true;

    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render findings drawer");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("x cancel sending"));
    assert!(!text.contains("x delete"));
}

#[test]
fn findings_drawer_offers_retry_only_for_a_failed_current_source_submission() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    let finding_id = app
        .review
        .session()
        .expect("review session")
        .findings
        .first()
        .expect("fixture finding")
        .id
        .clone();
    let submission_id = luxe_core::guided_review::SubmissionId::new();
    let session = app.review.session_mut().expect("review session");
    session
        .queue_submission(
            submission_id.clone(),
            luxe_core::guided_review::ReviewFeedbackMode::RequestFixes,
            std::slice::from_ref(&finding_id),
            &[],
            0,
        )
        .expect("queue submission");
    session.dispatch_submission(&submission_id).expect("dispatch submission");
    session.fail_submission(&submission_id).expect("fail submission");
    app.guided_review_workspace.selected_finding_id = Some(finding_id);
    app.guided_review_workspace.findings_visible = true;

    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render findings drawer");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("r retry sending"));
    assert!(!text.contains("r resolve"));
}

#[test]
fn source_labels_preserve_the_selected_local_target() {
    assert_eq!(source_label(&ReviewTarget::WorkingTree), "working tree");
    assert_eq!(
        source_label(&ReviewTarget::Base { reference: "origin/main".into() }),
        "base origin/main"
    );
}

#[test]
fn tiny_review_shows_the_minimum_size_and_return_affordance() {
    use ratatui::{backend::TestBackend, Terminal};

    let app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("review fixture");
    let backend = TestBackend::new(39, 4);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render tiny review");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("Review needs 40 × 5 cells"));
    assert!(text.contains("Resize, or esc back to chat"));
}

#[test]
fn long_failed_refresh_detail_keeps_wide_recovery_actions_visible() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewRefreshing).expect("refresh fixture");
    app.guided_review_workspace.mark_failed(
        ReviewTarget::WorkingTree,
        GuidedReviewFailure { kind: GuidedReviewFailureKind::Capture, message: "界".repeat(100) },
    );
    let backend = TestBackend::new(100, 16);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render long failure");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("Refresh failed:"));
    assert!(text.contains("r retry"));
    assert!(text.contains("s change source"));
    assert!(text.contains("esc back to chat"));
}

#[test]
fn failed_refresh_keeps_the_pinned_diff_visible_with_recovery_controls() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewRefreshing).expect("refresh fixture");
    app.guided_review_workspace.mark_failed(
        ReviewTarget::WorkingTree,
        GuidedReviewFailure {
            kind: GuidedReviewFailureKind::Capture,
            message: "source changed while capturing".into(),
        },
    );
    let backend = TestBackend::new(160, 32);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render failed refresh");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("Refresh failed"));
    assert!(text.contains("revision retained"));
    assert!(text.contains("source changed while capturing"));
    assert!(text.contains("src/notification.rs"));
    assert!(text.contains("r retry"));
    assert!(text.contains("s change source"));
}

#[test]
fn refresh_keeps_the_pinned_diff_visible_with_a_stale_indicator() {
    use ratatui::{backend::TestBackend, Terminal};

    let app = seeded_app("nord", FixtureScenario::GuidedReviewRefreshing).expect("refresh fixture");
    let backend = TestBackend::new(160, 32);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render refreshing review");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("Refreshing"));
    assert!(text.contains("revision retained"));
    assert!(text.contains("src/notification.rs"));
    assert!(text.contains("pub struct Notification"));
}

#[test]
fn load_failure_shows_typed_error_and_width_appropriate_recovery_controls() {
    use ratatui::{backend::TestBackend, Terminal};

    let app = seeded_app("nord", FixtureScenario::GuidedReviewError).expect("error fixture");
    let wide_backend = TestBackend::new(100, 30);
    let mut wide_terminal = Terminal::new(wide_backend).expect("wide terminal");
    wide_terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render wide error");
    let wide: String =
        wide_terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();
    let narrow_backend = TestBackend::new(58, 20);
    let mut narrow_terminal = Terminal::new(narrow_backend).expect("narrow terminal");
    narrow_terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render narrow error");
    let narrow: String =
        narrow_terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(wide.contains("Guided Review could not load"));
    assert!(wide.contains("Could not capture a complete immutable"));
    assert!(wide.contains("r retry"));
    assert!(wide.contains("s change source"));
    assert!(narrow.contains("r retry"));
    assert!(!narrow.contains("s change source"));
}

#[test]
fn unavailable_last_turn_is_visibly_disabled_in_source_picker() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    app.guided_review_workspace.source_picker =
        ReviewSourcePickerState::Open { selection: ReviewSourcePickerSelection::LastTurn };
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render source picker");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("× Last Turn"));
    assert!(text.contains("unavailable — no completed turn"));
}
