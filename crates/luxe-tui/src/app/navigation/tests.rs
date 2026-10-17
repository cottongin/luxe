//! Leaving a session for another: what every way out leaves behind.

use super::*;
use crate::dashboard::{DashOutcome, DashboardState};
use luxe_core::config::Config;
use luxe_core::keymap::KeyMap;
use tokio::sync::mpsc;

/// What leaving left: the view, whether the dashboard and transcript are gone, whether the app is
/// at rest, and what the engine was asked for.
fn left(app: &App, rx: &mut mpsc::Receiver<Action>) -> (ViewMode, bool, bool, bool, String) {
    let asked = rx.try_recv().expect("the engine was asked for the next session");
    let idle = matches!(app.status.state, AppState::Idle);
    let asked = format!("{asked:?}");
    (app.current_view, app.dashboard.is_none(), app.convo.is_empty(), idle, asked)
}

fn on_the_start_screen() -> (App, mpsc::Receiver<Action>) {
    let (tx, rx) = mpsc::channel(8);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.enter_dashboard(DashboardState::new(Vec::new()));
    (app, rx)
}

/// Each door off the start screen lands in the conversation, at rest, with the engine asked for
/// the session that door names.
#[tokio::test]
async fn every_way_off_the_start_screen_leaves_the_same_state() {
    let outcomes = [
        (DashOutcome::NewSession, "NewSession"),
        (DashOutcome::Resume("s-older".into()), "LoadSession(\"s-older\")"),
        (DashOutcome::ResumeLatest, "ResumeLatest"),
    ];
    for (outcome, asked) in outcomes {
        let (mut app, mut rx) = on_the_start_screen();
        app.handle_dashboard_outcome(outcome);
        let state = left(&app, &mut rx);
        assert_eq!(state, (ViewMode::Chat, true, true, true, asked.to_string()));
    }
}

/// `/new` in a session empties the transcript and forgets the session it left, so the sidebar does
/// not keep naming it until the engine answers.
#[tokio::test]
async fn new_in_a_session_starts_empty_and_forgets_the_old_one() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.session_id = Some("s-current".into());
    app.push_convo(crate::app::ConvoBlock::System("earlier".into()));
    app.status.state = AppState::Streaming;
    app.streaming = true;

    let new = crate::commands::CommandRegistry::all().parse_slash("/new").expect("/new");
    super::super::commands::dispatch(&mut app, new);

    let state = left(&app, &mut rx);
    assert_eq!(state, (ViewMode::Chat, true, true, true, "NewSession".to_string()));
    assert_eq!(app.session_id, None);
    assert!(!app.streaming);
}

/// Leaving the review closes its feedback window, and the details or fix view open over it, so
/// coming back to the same review opens on the diff.
///
/// The break this catches: the window's flag outlived the visit, so a review left with it open
/// reopened with it open; and closing only the window stranded a details view over the diff with
/// no window to take the Esc that closes it.
#[test]
fn leaving_the_review_closes_its_feedback_window() {
    let opened: [fn(&mut crate::guided_review::GuidedReviewWorkspace); 3] = [
        |_| {},
        |workspace| workspace.toggle_finding_details(),
        |workspace| workspace.toggle_finding_fix(),
    ];
    for open in opened {
        let scenario = crate::fixture::FixtureScenario::GuidedReviewFindings;
        let mut app = crate::fixture::seeded_app("luxe", scenario).expect("the fixture");
        assert!(app.guided_review_workspace.findings_visible, "the fixture opens with it open");
        open(&mut app.guided_review_workspace);
        app.update(crate::app::Msg::ExitGuidedReview);
        crate::app::commands::open_or_start_guided_review(&mut app);
        assert_eq!(app.current_view, ViewMode::GuidedReview, "back on the same review");
        let workspace = &app.guided_review_workspace;
        assert!(!workspace.findings_visible, "the window did not come back with it");
        assert!(!workspace.finding_details_visible, "nor its details");
        assert!(!workspace.finding_fix_visible, "nor its fix");
    }
}
