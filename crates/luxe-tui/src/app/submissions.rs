//! The submission tray — messages held above the composer mid-turn, and how each leaves it: run,
//! recalled, or landed as never sent.

use super::{App, AppState, ConvoBlock, MidTurnDelivery, PendingSubmission};
use luxe_core::engine::Action;

impl App {
    /// Pins a mid-turn submission above the composer instead of admitting it to the transcript.
    pub fn hold_submission(&mut self, submission: PendingSubmission) {
        self.pending_submissions.push(submission);
    }

    /// How many held submissions are waiting on a turn of their own.
    pub fn queued_count(&self) -> usize {
        self.pending_submissions
            .iter()
            .filter(|held| held.delivery == MidTurnDelivery::Queue)
            .count()
    }

    /// A turn this front-end asked for is under way, re-sending a message already on screen, so
    /// its commit adopts that copy rather than adding a second.
    pub(crate) fn begin_own_turn(&mut self) {
        self.own_commit_pending = true;
        self.status.state = AppState::Streaming;
        self.status.on_turn_start();
    }

    /// Records a submission that could not be delivered at all, so it still shows up in the
    /// transcript instead of vanishing along with the failure.
    pub fn record_unsent_submission(&mut self, submission: PendingSubmission) {
        self.land_submission(submission, true);
    }

    /// Admits a held submission into the transcript, optionally marking that it never ran.
    fn land_submission(&mut self, held: PendingSubmission, never_sent: bool) {
        let images = held.images;
        // A held message joining the transcript is the user's own content, so it is never queued
        // behind an animation: whatever the last turn was still revealing appears with it.
        self.reveal_catch_up();
        self.push_convo(ConvoBlock::User {
            text: held.display,
            attached: held.attached,
            chips: held.chips,
        });
        if never_sent {
            if let Some(id) = self.convo.last().map(|item| item.id.clone()) {
                self.never_sent.insert(id);
            }
        }
        self.push_attached_images(&images);
    }

    /// The running turn took the steering in, so its tray rows retire.
    ///
    /// The transcript copy is *not* pushed here: the engine commits each delivered steer as a real
    /// user entry and announces it with `MessageCommitted`, which the reducer already lands — with
    /// a persisted id, so copy/expand/selection work on it. Landing it a second time from the
    /// delivery signal is what made every steer appear twice.
    pub fn retire_steering(&mut self) {
        self.pending_submissions.retain(|held| held.delivery != MidTurnDelivery::Steer);
    }

    /// Hands the follow-ups waiting on a turn of their own back to the composer.
    ///
    /// Steering is deliberately left held: the engine buffers a steer that missed its checkpoint
    /// and delivers it at the start of the next turn, so it really is still pending and the tray
    /// should keep saying so.
    pub fn return_queued_messages(&mut self) -> usize {
        self.restore_pending(|held| held.delivery == MidTurnDelivery::Queue)
    }

    /// Cancel clears the engine's steer buffer along with everything else, so nothing held can
    /// still arrive — and everything waiting comes back to the composer.
    pub fn cancel_pending_submissions(&mut self) -> usize {
        self.restore_pending(|_| true)
    }

    /// Takes every waiting submission back out of the tray and into the composer, newest last.
    ///
    /// The recall key, and what an interrupt does with whatever was still queued. A message that
    /// never ran is worth more editable in the composer than as a `never sent` row in the
    /// transcript: the reason a steer is usually pulled back is to say it better.
    ///
    /// A steer reaches the engine when it is submitted, so recalling one asks the engine to drop its
    /// buffer too. The one race — a checkpoint delivering it in the instant before the key lands —
    /// resolves in the direction that loses nothing: the delivered steer appears in the transcript
    /// as the engine committed it, and its text is also in the composer to keep or clear.
    pub fn restore_pending(&mut self, take: impl Fn(&PendingSubmission) -> bool) -> usize {
        let (restoring, waiting) =
            std::mem::take(&mut self.pending_submissions).into_iter().partition::<Vec<_>, _>(&take);
        self.pending_submissions = waiting;
        if restoring.is_empty() {
            return 0;
        }
        if restoring.iter().any(|held| held.delivery == MidTurnDelivery::Steer) {
            self.send_action(Action::WithdrawSteering, "withdraw steering");
        }
        let count = restoring.len();
        self.editor.prepend_messages(restoring.into_iter().map(|held| (held.draft, held.images)));
        count
    }

    /// Promotes the oldest queued follow-up into a real turn (strictly one at a time, FIFO).
    ///
    /// Called when a turn ends. Returns whether a turn was started.
    pub fn promote_next_queued(&mut self) -> bool {
        let Some(index) = self
            .pending_submissions
            .iter()
            .position(|held| held.delivery == MidTurnDelivery::Queue)
        else {
            return false;
        };
        let message = self.pending_submissions.remove(index);
        self.streaming = false;
        self.status.state = AppState::Streaming;
        self.status.on_turn_start();
        let action = Action::SubmitUserMessage {
            content: message.content.clone(),
            images: message
                .images
                .iter()
                .map(|image| luxe_core::engine::ImageAttachment {
                    data: image.data.clone(),
                    mime: image.mime.clone(),
                    name: Some(image.filename.clone()),
                })
                .collect(),
            remediation: None,
        };
        if !self.send_action(action, "send a queued message") {
            self.land_submission(message, true);
            self.push_convo(ConvoBlock::System(
                "[engine unavailable — channel closed]".to_string(),
            ));
            self.status.state = AppState::Idle;
            return false;
        }
        // It is a real turn now, so it reads as one: the message joins the transcript ahead of
        // the output it is about to produce.
        self.land_submission(message, false);
        self.own_commit_pending = true;
        self.review.last_turn = None;
        true
    }
}
