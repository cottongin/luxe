//! Fixing someone else's pull request — the offer, and the comments that travel with it.
//!
//! Reading a pull request that is not checked out, the agent cannot change it, so a comment would
//! stop at "post it to GitHub". Here the reader can take it further: the engine says whether the
//! pull request can be checked out and pushed to, and on confirming, the TUI exits into a new
//! session in a worktree of its own, carrying the unsent comments to become the agent's work
//! there. This session stays as it was.

use luxe_core::engine::{Action, ReviewEdit};
use luxe_core::guided_review::{
    FindingId, FindingState, ReviewBase, ReviewFinding, ReviewSession, ReviewTarget,
};

use super::App;
use crate::app::ReviewEditPurpose;
use crate::guided_review::PullRequestFix;
use crate::main_loop::FixLaunch;

/// Opens the offer on the pull request under review, and asks whether it can be fixed here,
/// answering whether it opened.
pub(super) fn open(app: &mut App) -> bool {
    let Some(ReviewBase::PullRequest { number, .. }) =
        app.guided_review.as_ref().map(|review| review.source.base.clone())
    else {
        return false;
    };
    let reviewed_on = match app.guided_review.as_ref().map(|review| &review.target) {
        Some(ReviewTarget::PullRequest { repository: Some(other), .. }) => Some(other.clone()),
        _ => None,
    };
    if !app.send_quietly(
        Action::CheckPullRequestFix { number, repository: reviewed_on },
        "check a pull request fix",
    ) {
        app.status.flash_toast(format!("Could not ask whether #{number} can be fixed here"));
        return false;
    }
    let comments = carried(app).len();
    let workspace = &mut app.guided_review_workspace;
    workspace.help_visible = false;
    workspace.close_feedback_window();
    workspace.pull_request_fix =
        Some(PullRequestFix { number, branch: None, refusal: None, comments });
    true
}

/// GitHub answered whether the pull request can be fixed here.
pub(crate) fn checked(app: &mut App, number: u64, branch: Option<String>, refusal: Option<String>) {
    if let Some(fix) =
        app.guided_review_workspace.pull_request_fix.as_mut().filter(|fix| fix.number == number)
    {
        fix.branch = branch;
        fix.refusal = refusal;
    }
}

/// Leaves this session for a new one with the pull request checked out, taking the comments.
pub(super) fn confirm(app: &mut App) {
    let Some(number) = app
        .guided_review_workspace
        .pull_request_fix
        .as_ref()
        .filter(|fix| fix.branch.is_some())
        .map(|fix| fix.number)
    else {
        return;
    };
    let carried = carried(app);
    app.guided_review_workspace.pull_request_fix = None;
    app.outbox.pull_request_fix = Some(FixLaunch { number, carried });
}

/// The comments that go with it: written here, not yet sent or posted.
fn carried(app: &App) -> Vec<ReviewFinding> {
    app.review
        .session()
        .map(|session| {
            session
                .findings
                .iter()
                .filter(|finding| {
                    matches!(finding.state, FindingState::Draft | FindingState::Open)
                        && finding.posted.is_none()
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Adopts the carried comments into the checked-out pull request's review once its session is
/// here, and sends the ones still placed to the agent in the same save: until then they exist
/// nowhere in this session.
pub(crate) fn adopt_carried(app: &mut App) {
    let matches = match (app.guided_review.as_ref(), app.carried_findings.as_ref()) {
        (Some(review), Some(launch)) => {
            review.target == ReviewTarget::OwnPullRequest { number: launch.number }
        }
        _ => false,
    };
    if !matches {
        return;
    }
    let Some(launch) = app.carried_findings.take() else {
        return;
    };
    if launch.carried.is_empty() {
        return;
    }
    let carried: Vec<FindingId> = launch.carried.iter().map(|finding| finding.id.clone()).collect();
    // Choosing to fix them was the instruction, so they go to the agent without a second ask
    let edit = ReviewEdit::AdoptCarried { findings: launch.carried, send: true };
    app.send_review_edit(ReviewEditPurpose::Carried(carried), edit);
}

/// The carried comments are in the review. When they went to the agent too, that is the thing to
/// watch, and the review waits on Ctrl+G.
pub(crate) fn carried_saved(app: &mut App, session: &ReviewSession, carried: &[FindingId]) {
    let sent = session.submissions.iter().any(|submission| {
        submission.findings.iter().any(|finding| carried.contains(&finding.finding_id))
    });
    if sent {
        app.review.open_when_ready = false;
        app.status.flash_toast("Your comments went to the agent · ctrl+g opens the review");
    }
}

impl App {
    /// Opens a checked-out pull request's review on launch, holding the comments carried to it
    /// until it loads.
    pub(crate) fn open_pull_request_fix(&mut self, launch: FixLaunch) {
        let target = ReviewTarget::OwnPullRequest { number: launch.number };
        self.carried_findings = Some(launch);
        self.review.open_when_ready = true;
        super::super::commands::start_guided_review(self, target);
    }
}
