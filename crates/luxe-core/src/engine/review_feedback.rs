//! Review feedback — comments from Guided Review, queued and then sent as a turn.
//!
//! The review desk stores a submission, then sends it here as `SubmitReviewFeedback`. It is held as
//! a message until the next turn boundary and goes out as `DispatchQueuedReviewFeedback` → an
//! ordinary turn carrying the submission's evidence, so its remediation report can be matched back
//! to it. Questions run that turn held to reading ([`super::read_only`]). What happens to it on the
//! way is told to the desk, which stores it.

use super::*;
use crate::guided_review::{
    ReviewBase, ReviewFeedbackMode, ReviewId, ReviewSource, SubmissionId, SubmittedFinding,
};

/// Why a pull request review's comments cannot go to the agent as fixes.
pub const PULL_REQUEST_FIXES_REFUSED: &str = "This pull request is not checked out here, so the \
     agent cannot change it. Questions (a) still go to the agent, and your comments stay with the \
     review.";

/// A submission's message and evidence, waiting for its turn.
pub(super) struct PendingReviewFeedback {
    content: String,
    remediation: RemediationTurnContext,
}

impl Engine {
    /// Holds a submission for the next turn boundary, once per submission id.
    ///
    /// Fixing a pull request means changing commits that are not checked out here, so the agent
    /// would edit the working tree instead: a different revision, and not the reviewer's to change.
    /// That is refused; questions and comments still go through.
    pub(super) async fn handle_submit_review_feedback(
        &mut self,
        submission_id: SubmissionId,
        review_id: ReviewId,
        source: ReviewSource,
        mode: ReviewFeedbackMode,
        findings: Vec<SubmittedFinding>,
        event_tx: &mpsc::Sender<Event>,
    ) {
        if !self.accepted_feedback_submissions.insert(submission_id.clone()) {
            debug!(submission_id = %submission_id.as_str(), "duplicate review feedback submission ignored");
            return;
        }
        if mode == ReviewFeedbackMode::RequestFixes
            && matches!(source.base, ReviewBase::PullRequest { .. })
        {
            self.review_desk.send(super::review_desk::DeskCommand::Failed(submission_id.clone()));
            let _ = event_tx
                .send(Event::ReviewFeedbackTurnFailed {
                    submission_id,
                    message: PULL_REQUEST_FIXES_REFUSED.into(),
                })
                .await;
            return;
        }
        let content = crate::guided_review::format_review_feedback_message(
            &submission_id,
            &review_id,
            &source,
            mode,
            &findings,
        );
        self.pending_feedback.insert(
            submission_id.clone(),
            PendingReviewFeedback {
                content,
                remediation: RemediationTurnContext {
                    answers_only: mode == ReviewFeedbackMode::Question,
                    reports: mode == ReviewFeedbackMode::RequestFixes,
                    submission_id: Some(submission_id.clone()),
                    review_id,
                    revision_id: source.content_digest.clone(),
                    finding_ids: findings
                        .iter()
                        .map(|finding| finding.finding_id.clone())
                        .collect(),
                },
            },
        );
        self.enqueue_continuation(Action::DispatchQueuedReviewFeedback { submission_id });
    }

    /// Drops a submission still waiting for its turn.
    pub(super) fn cancel_queued_review_feedback(&mut self, submission_id: &SubmissionId) {
        if self.pending_feedback.remove(submission_id).is_some() {
            self.accepted_feedback_submissions.remove(submission_id);
            debug!(submission_id = %submission_id.as_str(), "cancelled queued review feedback");
        }
    }

    /// Sends a waiting submission as a turn, unless it was cancelled in the meantime.
    pub(super) async fn dispatch_queued_review_feedback(
        &mut self,
        submission_id: SubmissionId,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let Some(feedback) = self.pending_feedback.remove(&submission_id) else {
            debug!(submission_id = %submission_id.as_str(), "cancelled review feedback dispatch skipped");
            return;
        };
        self.review_desk.send(super::review_desk::DeskCommand::Dispatched(submission_id.clone()));
        if event_tx
            .send(Event::ReviewFeedbackTurnDispatched { submission_id: submission_id.clone() })
            .await
            .is_err()
        {
            warn!(submission_id = %submission_id.as_str(), "review feedback dispatch event receiver closed");
        }
        self.enqueue_continuation(Action::SubmitUserMessage {
            content: feedback.content,
            images: Vec::new(),
            remediation: Some(feedback.remediation),
        });
    }
}
