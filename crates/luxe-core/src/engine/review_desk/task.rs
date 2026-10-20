//! Task — the desk's channel, the task that runs it, and the engine's side of both.

use super::*;

// === The engine's side ===
impl crate::engine::Engine {
    /// Starts the desk on this engine's event stream, once.
    pub(in crate::engine) fn start_review_desk(&self, event_tx: &mpsc::Sender<Event>) {
        let inbox =
            self.review_desk_inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        let Some(inbox) = inbox else {
            return;
        };
        let desk = ReviewDesk::new(self.config.guided_review_project_root.clone());
        spawn(desk, inbox, event_tx.clone(), self.config.action_tx.clone());
    }

    /// Hands a front end's review edit, progress or seen record to the desk.
    ///
    /// Synchronous, so the turn's `select!` can do it too: that is what lets a reviewer comment
    /// while the agent is still working on the last batch.
    pub(in crate::engine) fn forward_to_review_desk(&self, action: Action) {
        let command = match action {
            Action::EditReview { request_id, review_id, edit } => {
                DeskCommand::Edit { request_id, review_id, edit }
            }
            Action::RecordReviewProgress { review_id, progress } => {
                DeskCommand::RecordProgress { review_id, progress }
            }
            Action::RecordReviewSeen { review_id } => DeskCommand::RecordSeen { review_id },
            Action::ShowOpenReview { request_id } => DeskCommand::Show { request_id },
            other => {
                debug!(?other, "not a review desk action");
                return;
            }
        };
        self.review_desk.send(command);
    }

    /// Waits, briefly, for the desk to handle everything already sent to it.
    ///
    /// An edit is forwarded the moment it arrives, so a front end that saves a comment and closes
    /// at once would otherwise stop the engine with the write still queued — and a process that
    /// exits drops it. Bounded, so a desk stuck on the disk cannot hold shutdown hostage.
    pub(in crate::engine) async fn flush_review_desk(&self) {
        let (done, flushed) = tokio::sync::oneshot::channel();
        self.review_desk.send(DeskCommand::Flush(done));
        if tokio::time::timeout(FLUSH_DEADLINE, flushed).await.is_err() {
            warn!("the review desk did not finish its last writes before shutdown");
        }
    }

    /// The open review and its session, when `review_id` is the open one and, if given, still at
    /// `revision`; otherwise why not, as a sentence for the flow that asked.
    ///
    /// Everything that acts on a review by id reads it here, so a flow can never act on a review
    /// other than the one on screen, or on a revision the reader has not seen.
    pub(in crate::engine) async fn open_review(
        &self,
        review_id: &ReviewId,
        revision: Option<&ReviewRevisionId>,
    ) -> Result<(GuidedReview, ReviewSession), String> {
        let (reply, answer) = tokio::sync::oneshot::channel();
        self.review_desk.send(DeskCommand::Snapshot(reply));
        let open = answer.await.ok().flatten();
        let Some((review, session)) = open.filter(|(review, _)| &review.id == review_id) else {
            return Err("That review is no longer open. Open it again, then try this.".into());
        };
        if revision.is_some_and(|revision| revision != &review.revision_id) {
            return Err("The review was refreshed since this was shown, so nothing was done. \
                        Look at it again, then try once more."
                .into());
        }
        Ok((review, session))
    }

    /// A handle for work that reports to the desk from a task of its own.
    pub(in crate::engine) fn review_desk(&self) -> ReviewDeskHandle {
        self.review_desk.clone()
    }
}

/// How long shutdown waits for the desk's last writes.
const FLUSH_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

// === The task ===

/// The sending half of the desk's channel, held by the engine and the tasks it spawns.
#[derive(Debug, Clone)]
pub(crate) struct ReviewDeskHandle(mpsc::UnboundedSender<DeskCommand>);

/// The receiving half, waiting for [`spawn`].
pub(crate) type DeskInbox = mpsc::UnboundedReceiver<DeskCommand>;

impl ReviewDeskHandle {
    /// A handle and the inbox it feeds.
    ///
    /// Unbounded on purpose: sending must never wait, since the turn forwards edits from its
    /// `select!`, where waiting would hold up `Cancel`. What arrives is human-rate, plus a few
    /// lifecycle notes per turn.
    pub(crate) fn channel() -> (Self, DeskInbox) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self(tx), rx)
    }

    /// Hands `command` to the desk. After the engine shut down there is nobody to tell.
    pub(crate) fn send(&self, command: DeskCommand) {
        if self.0.send(command).is_err() {
            debug!("the review desk has stopped; dropping a command");
        }
    }
}

/// Runs `desk` until every handle is dropped, forwarding what it does.
///
/// Args: `(desk, inbox, event_tx, action_tx)` — `action_tx` is the engine's own queue, for the
/// submissions a stored edit starts; `None` (an engine with no self-continuation) drops them.
pub(crate) fn spawn(
    mut desk: ReviewDesk,
    mut inbox: DeskInbox,
    event_tx: mpsc::Sender<Event>,
    action_tx: Option<mpsc::Sender<Action>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(command) = inbox.recv().await {
            for effect in desk.handle(command).await {
                match effect {
                    Effect::Event(event) => {
                        if event_tx.send(event).await.is_err() {
                            debug!("the front end is gone; the review desk keeps saving");
                        }
                    }
                    Effect::Engine(action) => match action_tx.as_ref() {
                        Some(action_tx) => {
                            if action_tx.send(*action).await.is_err() {
                                debug!("the engine stopped before a review submission reached it");
                            }
                        }
                        None => warn!("this engine cannot queue review feedback turns"),
                    },
                }
            }
        }
    })
}
