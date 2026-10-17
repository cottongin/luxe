//! Review source — loading a review, and what the surface says when it is ready or failed.

use super::*;

impl GuidedReviewWorkspace {
    /// Starts displaying a target without discarding any pinned review already on screen.
    pub(crate) fn begin_loading(&mut self, target: ReviewTarget) {
        self.source_picker = ReviewSourcePickerState::Closed;
        self.source_reference_entry = None;
        self.load_state = GuidedReviewLoadState::Loading { target };
    }

    /// Records that the active target produced a ready pinned review.
    pub(crate) fn mark_ready(&mut self) {
        self.load_state = GuidedReviewLoadState::Ready;
        self.noise_details_visible = false;
    }

    /// Records a typed source failure for recovery controls.
    pub(crate) fn mark_failed(&mut self, target: ReviewTarget, error: GuidedReviewFailure) {
        self.load_state = GuidedReviewLoadState::Failed { target, error };
    }
}
