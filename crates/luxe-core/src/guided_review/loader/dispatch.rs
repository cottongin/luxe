//! LocalGit — target dispatch and load/refresh.

use super::*;

impl LocalGit {
    /// Captures and pins the exact immutable source selected by a review target.
    pub async fn capture_and_persist_target_source(
        &self,
        target: &ReviewTarget,
        policy: UntrackedSourcePolicy,
    ) -> Result<WorkingTreeCapture, GuidedReviewError> {
        match target {
            ReviewTarget::WorkingTree => self.capture_and_persist_working_tree_source(policy).await,
            ReviewTarget::LastTurn { session_id, turn_id } => {
                self.capture_and_persist_last_turn_source(session_id, turn_id, policy).await
            }
            ReviewTarget::Base { reference } => {
                self.capture_and_persist_base_working_tree_source(reference, policy).await
            }
            ReviewTarget::Commit { oid } => self.capture_and_persist_commit_source(oid).await,
            ReviewTarget::PullRequest { number, .. } => {
                self.capture_and_persist_pull_request_source(*number).await
            }
            ReviewTarget::OwnPullRequest { number } => {
                self.capture_and_persist_own_pull_request_source(*number, policy).await
            }
        }
    }

    /// Captures a target and combines its pinned source with already bounded project context.
    pub async fn load_target_review(
        &self,
        id: ReviewId,
        target: ReviewTarget,
        context: ReviewContext,
        policy: UntrackedSourcePolicy,
    ) -> Result<GuidedReview, GuidedReviewError> {
        let capture = self.capture_and_persist_target_source(&target, policy).await?;
        // A commit-backed source can say what it is; ask it before falling back to a generic label.
        let context = match self.source_intent(&capture.source.base).await {
            Some(intent) => {
                context.with_source_intent(intent.title, intent.summary, intent.commits)
            }
            None => context,
        };
        GuidedReview::from_pinned_artifact(
            id,
            target,
            capture.source,
            context,
            &capture.artifact,
            self.exclusions(),
            capture_time_ms(),
        )
        .map_err(GuidedReviewError::Build)
    }

    /// Refreshes a review only when the caller still displays the expected pinned revision.
    pub async fn refresh_target_review(
        &self,
        review: &GuidedReview,
        expected_revision: &ReviewRevisionId,
        context: ReviewContext,
        policy: UntrackedSourcePolicy,
    ) -> Result<GuidedReview, GuidedReviewError> {
        if expected_revision != &review.revision_id {
            return Err(GuidedReviewError::StaleRevision {
                expected: expected_revision.clone(),
                current: review.revision_id.clone(),
            });
        }
        self.preserving_revision(review.revision_id.clone())
            .load_target_review(review.id.clone(), review.target.clone(), context, policy)
            .await
    }
}
