//! Verification — tests moved out of the module they cover.

use super::*;
use crate::fixture::{seeded_app, FixtureScenario};
use luxe_core::guided_review::{
    DiffSide, FindingAnchor, FindingMessage, FindingMessageAuthor, FindingState,
    ReviewFeedbackMode, SubmissionId,
};
use ratatui::{backend::TestBackend, Terminal};

/// `NeedsVerification` used to be verified on trust: there was no way to see what the agent
/// changed at the anchor without refreshing the whole review and hunting for it.
#[test]
fn the_fix_view_shows_the_anchor_before_and_after() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    let review = app.guided_review.clone().expect("fixture review");
    let file = &review.files[0];
    let hunk = &file.hunks[0];
    let anchored =
        hunk.lines.iter().find(|line| line.new_line.is_some()).expect("an anchored line");

    let session = app.review.session_mut().expect("review session");
    let finding_id = session.findings[0].id.clone();
    {
        let finding = &mut session.findings[0];
        finding.anchor = FindingAnchor::Line {
            file_id: file.id.clone(),
            hunk_id: hunk.id.clone(),
            side: DiffSide::New,
            line: anchored.new_line.expect("new-side line"),
            context_hash: anchored.context_hash(),
        };
        finding.state = FindingState::NeedsVerification;
        finding.thread.push(FindingMessage {
            author: FindingMessageAuthor::Agent,
            body: "Fixed by bounding the retry loop.".into(),
            created_at_ms: 0,
        });
    }
    let submission_id = SubmissionId::new();
    session
        .queue_submission(
            submission_id,
            ReviewFeedbackMode::RequestFixes,
            std::slice::from_ref(&finding_id),
            &review.files,
            0,
        )
        .expect("queue the submission");
    app.guided_review_workspace.selected_finding_id = Some(finding_id);
    app.guided_review_workspace.findings_visible = true;
    app.guided_review_workspace.toggle_finding_fix();

    let backend = TestBackend::new(160, 40);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render the fix view");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("What changed here"), "{text}");
    assert!(text.contains("When you commented"), "the submitted evidence is shown: {text}");
    assert!(text.contains("Now"), "the current state is shown: {text}");
    // The agent's word is presented as a claim, next to the code that settles it.
    assert!(text.contains("a claim, not proof"), "{text}");
    assert!(text.contains("r verify and resolve"), "{text}");
}

/// Remediation usually *moves* code, and "refresh and look for it yourself" abandons the
/// reviewer at the moment the tool should help. A dead anchor reports where the line went.
#[test]
fn a_moved_anchor_reports_where_the_line_went() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    let review = app.guided_review.clone().expect("fixture review");
    let file = &review.files[0];
    let moved_line = file.hunks[1]
        .lines
        .iter()
        .find(|line| line.new_line.is_some())
        .expect("a line in the second hunk");

    let session = app.review.session_mut().expect("review session");
    let finding_id = session.findings[0].id.clone();
    {
        // Anchor the comment at a hunk that no longer exists, but whose source text has
        // resurfaced elsewhere in the review — the shape a moved fix leaves behind.
        let finding = &mut session.findings[0];
        finding.anchor = FindingAnchor::Line {
            file_id: file.id.clone(),
            hunk_id: luxe_core::guided_review::HunkId::from_content(
                &file.id, "vanished", "vanished",
            ),
            side: DiffSide::New,
            line: 4_242,
            context_hash: moved_line.context_hash(),
        };
        finding.state = FindingState::NeedsVerification;
    }
    let submission_id = SubmissionId::new();
    session
        .queue_submission(
            submission_id,
            ReviewFeedbackMode::RequestFixes,
            std::slice::from_ref(&finding_id),
            &review.files,
            0,
        )
        .expect("queue the submission");
    // The submitted snapshot keeps the text the reviewer commented on, which is the handle.
    if let Some(snapshot) = app
        .review
        .session_mut()
        .and_then(|session| session.submissions.last_mut().and_then(|s| s.findings.first_mut()))
    {
        snapshot.evidence.source = Some(moved_line.text.clone());
    }
    app.guided_review_workspace.selected_finding_id = Some(finding_id);
    app.guided_review_workspace.findings_visible = true;
    app.guided_review_workspace.toggle_finding_fix();

    let backend = TestBackend::new(160, 40);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render the fix view");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("that line now lives at"), "{text}");
    assert!(text.contains("src/notification.rs"), "the new home is named: {text}");
    assert!(!text.contains("refresh the review"), "no dead end: {text}");
}
