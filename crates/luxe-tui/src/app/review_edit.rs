//! Review edits — comment changes sent to the review desk, and what their answers finish.

use super::*;

/// What an edit sent to the review desk was for, so its answer can finish the job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReviewEditPurpose {
    /// The open comment draft: it closes when this lands, and shows the refusal when it does not
    Draft,
    /// Comments sent to the agent
    Send,
    /// Comments carried over from reading a pull request, sent to the agent on adoption
    Carried(Vec<luxe_core::guided_review::FindingId>),
    /// Anything else, whose refusal goes in the transcript
    Other,
}

impl App {
    /// Sends one edit of the open review to the engine's review desk, answering whether it went.
    ///
    /// Recorded under its request id with what it was for, so its answer can finish the job: close
    /// the draft it saved, say a refusal where the reader is looking, or note that comments went
    /// to the agent. Nothing is applied here; the mirror moves when the desk answers.
    pub(crate) fn send_review_edit(
        &mut self,
        purpose: ReviewEditPurpose,
        edit: luxe_core::engine::ReviewEdit,
    ) -> bool {
        let Some(review_id) = self.guided_review.as_ref().map(|review| review.id.clone()) else {
            tracing::warn!("no review is open to edit");
            return false;
        };
        let request_id = ReviewSessionSaveRequestId::new();
        let action = Action::EditReview { request_id: request_id.clone(), review_id, edit };
        if !self.send_action(action, "edit the review") {
            return false;
        }
        self.review.edits.insert(request_id, purpose);
        true
    }

    /// Whether an edit for `purpose` is still waiting on its answer.
    pub(crate) fn review_edit_pending(&self, purpose: impl Fn(&ReviewEditPurpose) -> bool) -> bool {
        self.review.edits.values().any(purpose)
    }
}
