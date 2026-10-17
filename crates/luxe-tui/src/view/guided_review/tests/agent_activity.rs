//! Agent activity — tests moved out of the module they cover.

use super::*;
use crate::fixture::{seeded_app, FixtureScenario};
use ratatui::{backend::TestBackend, Terminal};

/// After submitting, the review used to go silent with no sign the agent was working. It now
/// names the comments in flight.
#[test]
fn the_rail_reports_that_the_agent_is_working_on_a_comment() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    let backend = TestBackend::new(160, 40);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let render_text = |terminal: &mut Terminal<TestBackend>, app: &App| {
        terminal.draw(|frame| render(frame, frame.area(), app)).expect("render");
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect::<String>()
    };

    assert!(!render_text(&mut terminal, &app).contains("Agent working"));

    let session = app.review.session_mut().expect("review session");
    let finding_id = session.findings[0].id.clone();
    let submission_id = luxe_core::guided_review::SubmissionId::new();
    session
        .queue_submission(
            submission_id.clone(),
            luxe_core::guided_review::ReviewFeedbackMode::RequestFixes,
            std::slice::from_ref(&finding_id),
            &[],
            0,
        )
        .expect("queue the submission");
    session.dispatch_submission(&submission_id).expect("dispatch it");

    let working = render_text(&mut terminal, &app);
    assert!(
        working.contains("Agent working on your comment"),
        "the rail names the work in flight: {working}"
    );
}
