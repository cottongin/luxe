//! Guided-review runtime state — the retained turn baseline and active-request tracker.
//!
//! The open review itself is the desk's ([`super::review_desk`]); this only decides which load
//! request is current, so a superseded one vanishes instead of overwriting a newer one.

use super::*;

#[derive(Debug, Clone)]
pub(crate) struct RetainedTurnBaseline {
    pub(crate) session_id: String,
    pub(crate) turn_id: AgentTurnId,
}

#[derive(Default)]
pub(crate) struct ActiveGuidedReview {
    pub(crate) request_id: Option<ReviewLoadRequestId>,
    pub(crate) revision_id: Option<ReviewRevisionId>,
    /// Cancels the Guide pass of whichever request is current.
    ///
    /// A review owns its own signal rather than borrowing the turn's. Sharing one meant a guide
    /// died whenever *any* turn was cancelled, and — because the turn's flag stays raised until
    /// the next turn lowers it — a guide started after a cancel read `true` before its first
    /// chunk and never ran at all.
    cancel: Option<watch::Sender<bool>>,
}

/// A cancel signal belonging to one review request.
///
/// A newtype rather than a bare `watch::Receiver<bool>`, so the turn's signal cannot be handed to
/// the Guide pass by mistake — the only way to obtain one is
/// [`ActiveGuidedReview::begin_request`]. It was a bare receiver, the engine had a
/// `cancel_tx.subscribe()` to hand, and the two were interchangeable at every call site: a guide
/// died whenever any turn was cancelled, and a guide started *after* a cancel read an
/// already-raised flag before its first chunk and silently fell back to structural grouping.
/// Nothing but the type system was ever going to keep those apart.
#[derive(Debug, Clone)]
pub(crate) struct GuideCancel(watch::Receiver<bool>);

impl GuideCancel {
    /// A signal nothing will ever raise, for the direct `guide_for_review` entry point.
    ///
    /// It has no tracker and no session behind it, so there is nobody to cancel it — but it still
    /// must not be the turn's, which would abort it the moment anything else was cancelled.
    pub(crate) fn detached(receiver: watch::Receiver<bool>) -> Self {
        Self(receiver)
    }

    /// The raw signal, for the one call that actually streams from a provider.
    pub(crate) fn receiver(&self) -> watch::Receiver<bool> {
        self.0.clone()
    }

    /// Whether this review has been told to stop.
    pub(crate) fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }
}

impl ActiveGuidedReview {
    /// Makes `request_id` the current one and hands back the revision to protect plus a cancel
    /// signal scoped to this request.
    ///
    /// Superseding a request cancels the one it replaced: the guide it was writing can no longer
    /// be published, so letting it run to completion only spends tokens on an answer with nowhere
    /// to go.
    pub(crate) fn begin_request(
        &mut self,
        request_id: ReviewLoadRequestId,
    ) -> (Option<ReviewRevisionId>, GuideCancel) {
        if let Some(previous) = self.cancel.take() {
            let _ = previous.send(true);
        }
        let (cancel_tx, cancel_rx) = watch::channel(false);
        self.cancel = Some(cancel_tx);
        self.request_id = Some(request_id);
        (self.revision_id.clone(), GuideCancel(cancel_rx))
    }

    pub(crate) fn complete_if_current(
        &mut self,
        request_id: &ReviewLoadRequestId,
        revision_id: ReviewRevisionId,
    ) -> bool {
        if !self.is_current(request_id) {
            return false;
        }

        self.revision_id = Some(revision_id);
        true
    }

    /// Cancels the Guide pass of the current request and forgets the request, so nothing it finds
    /// is published: the engine is stopping.
    ///
    /// Every pass holds a sender on the event stream, and a front end waits for that stream to end
    /// before it exits; a pass left running on a large review held `close` up for minutes.
    pub(crate) fn end(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(true);
        }
        self.request_id = None;
    }

    pub(crate) fn is_current(&self, request_id: &ReviewLoadRequestId) -> bool {
        self.request_id.as_ref() == Some(request_id)
    }
}
