//! The review mirror — what the app keeps about the open review beside the review itself: its
//! feedback session as the desk stored it, what is still in flight, and what was derived from it.

use super::{QueuedReview, ReviewEditPurpose};
use luxe_core::guided_review::{
    AttentionSignal, GuideProvenance, ReviewBudget, ReviewSession, ReviewSessionSaveRequestId,
    SeenReview,
};
use std::collections::HashMap;

/// The open review's session, the requests waiting on the desk, and the review's derived figures.
#[derive(Default)]
pub struct ReviewMirror {
    /// The open review's feedback session, as the engine's review desk last stored it.
    ///
    /// A mirror to render from. It only ever moves to what the desk sent back, so nothing here
    /// edits it; edits go out through [`crate::app::App::send_review_edit`].
    pub(crate) session: Option<ReviewSession>,
    /// Edits sent to the review desk and not answered yet, and what each was for.
    pub(crate) edits: HashMap<ReviewSessionSaveRequestId, ReviewEditPurpose>,
    /// Whether the desk's session for the open review has arrived, rather than the placeholder.
    pub(crate) bound: bool,
    /// The visible review's call-outs, capped at two of a kind; an aggregate row among them marks
    /// a kind that runs past the cap, which the brief then totals from `flags`.
    ///
    /// A pure function of the pinned revision, so it is computed when the review loads rather than
    /// on every frame: it walks every source line, which the render path must not.
    pub(crate) attention: Vec<AttentionSignal>,
    /// Every flag in the visible review, uncapped: what keeps a hunk on the reading list, and
    /// what each file's row and the walkthrough say about that file. Computed with `attention`.
    pub(crate) flags: Vec<AttentionSignal>,
    /// What the reviewer's last look at this project covered, so the dock can lead with the
    /// increment rather than the accumulated total.
    ///
    /// Arrives with a loaded review and outlives it: the dock keeps answering "how much have I not
    /// seen" long after the review surface is closed.
    pub seen: Option<SeenReview>,
    /// What the visible review actually asks of the reviewer, and what is already accounted for.
    ///
    /// Memoized from `flags`, so a hunk that only an "N more" call-out stands for stays on it.
    pub(crate) budget: ReviewBudget,
    /// In-flight commit request, so a late result cannot be mistaken for a fresh one.
    pub(crate) commit_request: Option<ReviewSessionSaveRequestId>,
    /// How the visible review's chapters were produced. `None` until a Guide pass reports back,
    /// which is also how the rail knows to say the sections are still structural.
    pub(crate) provenance: Option<GuideProvenance>,
    /// Exact persisted baseline selection eligible for a LastTurn review.
    pub last_turn: Option<(String, String)>,
    /// A review asked for mid-turn, waiting its turn and saying so in the tray.
    pub queued: Option<QueuedReview>,
    /// File the next loaded review should open on, set when a glance is escalated to a review.
    ///
    /// Held rather than applied because the review is not there yet: the load is a round trip, and
    /// the file the reader was looking at has to survive it.
    pub(crate) pending_file: Option<String>,
    /// Whether the review being prepared opens by itself once whole, rather than offering
    pub open_when_ready: bool,
}

impl ReviewMirror {
    /// The feedback session, once there is one.
    pub(crate) fn session(&self) -> Option<&ReviewSession> {
        self.session.as_ref()
    }

    /// The feedback session to change in place, for a fixture or a test standing in for the desk.
    #[cfg(any(test, feature = "fixture"))]
    pub(crate) fn session_mut(&mut self) -> Option<&mut ReviewSession> {
        self.session.as_mut()
    }
}
