//! Inbox — the order the engine serves its actions in, and the wait that keeps serving them.
//!
//! The engine is its action channel's only reader, so it must never send to that channel itself:
//! with the channel full, the send waits for a slot that only the sender could free, and `Cancel`
//! and `Close` queue up behind the deadlock. What the engine hands itself goes through
//! [`Engine::enqueue_continuation`] instead, and [`Engine::next_action`] serves it in its place.
//! Detached tasks still send on the channel, since they are not what reads it.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError};

use super::*;

fn lock(queue: &Mutex<VecDeque<Action>>) -> MutexGuard<'_, VecDeque<Action>> {
    queue.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Engine {
    /// Queues an action the engine hands itself, behind everything already waiting for it.
    pub(crate) fn enqueue_continuation(&self, action: Action) {
        lock(&self.continuations).push_back(action);
    }

    /// Runs the buffered steers, and `content` after them, as an ordinary turn of their own: with
    /// no turn in flight there is nothing for them to steer.
    pub(super) async fn steer_into_turn(&self, content: Option<String>) {
        let mut steers = std::mem::take(&mut *self.pending_steers.lock().await);
        steers.extend(content);
        if steers.is_empty() {
            return;
        }
        self.enqueue_continuation(Action::SubmitUserMessage {
            content: steers.join("\n\n"),
            images: Vec::new(),
            remediation: None,
        });
    }

    /// Settles the steers sent while `/compact` ran, which had no turn to reach: a turn of their
    /// own, unless the compaction was cancelled, which drops them the way a cancel with no turn
    /// running does.
    pub(super) async fn steers_after_compaction(&self) {
        match self.is_cancelled() {
            true => self.pending_steers.lock().await.clear(),
            false => self.steer_into_turn(None).await,
        }
    }

    /// The next action to serve, or `None` once every sender is gone.
    ///
    /// Actions deferred by a turn come first, since they arrived before anything still in the
    /// channel.
    pub(super) async fn next_action(
        &self,
        action_rx: &mut mpsc::Receiver<Action>,
    ) -> Option<Action> {
        self.absorb_continuations(action_rx);
        if let Some(action) = lock(&self.deferred_actions).pop_front() {
            return Some(action);
        }
        action_rx.recv().await
    }

    /// Queues `action` for the main loop, which serves it once the turn in flight is over.
    pub(super) fn defer(&self, action: Action) {
        lock(&self.deferred_actions).push_back(action);
    }

    fn absorb_continuations(&self, action_rx: &mut mpsc::Receiver<Action>) {
        line_up(&mut lock(&self.continuations), &mut lock(&self.deferred_actions), action_rx);
    }

    /// Awaits `work` while serving the actions that must reach it: `Cancel` and `Close`, permission
    /// and question replies, steering, and the glances that never wait.
    ///
    /// Every other action waits in [`Engine::deferred_actions`], in arrival order, so one turn
    /// runs at a time. Selecting on the channel beside the work is what lets a tool block on a
    /// permission prompt without wedging the engine, and what lets a cancel arrive at all.
    pub(crate) async fn serviced<T>(
        &self,
        work: impl std::future::Future<Output = T>,
        event_tx: &mpsc::Sender<Event>,
        action_rx: &mut mpsc::Receiver<Action>,
    ) -> T {
        tokio::pin!(work);
        loop {
            tokio::select! {
                biased;
                result = &mut work => return result,
                maybe = action_rx.recv() => match maybe {
                    Some(Action::Cancel) => {
                        tracing::info!("interrupt received mid-turn");
                        self.interrupt(event_tx).await;
                    }
                    // The front-end is leaving, and nobody is left to answer a prompt or read the
                    // reply, so the work stops as a cancel stops it and the engine closes once it
                    // has unwound, ahead of anything queued behind the turn. Left to run, it
                    // outlived the grace the front-end waits and kept using a checkout the next
                    // session may have moved out of
                    Some(Action::Close) => {
                        tracing::info!("close received mid-turn; stopping the turn first");
                        self.interrupt(event_tx).await;
                        lock(&self.deferred_actions).push_front(Action::Close);
                    }
                    Some(Action::RespondPermission { id, reply }) => {
                        self.permission_broker.resolve(&id, reply);
                    }
                    Some(Action::AnswerQuestion { id, reply }) => {
                        self.question_broker.resolve(&id, reply);
                    }
                    Some(Action::SetPermissionAutoApprove(on)) => {
                        self.permission_auto_approve
                            .store(on, std::sync::atomic::Ordering::Relaxed);
                    }
                    // Admitted immediately, promoted at the next checkpoint (see
                    // `deliver_pending_steers`) — the turn keeps running meanwhile.
                    Some(Action::SteerTurn { content }) => {
                        self.pending_steers.lock().await.push(content);
                    }
                    // Handled here rather than re-queued: this is the one moment it means
                    // anything, and after the turn it would clear a buffer that had since filled
                    // with steers the user does want.
                    Some(Action::WithdrawSteering) => self.withdraw_pending_steers().await,
                    // A glance at one file's diff, which the reader is sitting in front of waiting
                    // for. Re-queuing it left peek showing "Reading the change…" for the length of
                    // the turn, over a diff that was on disk the whole time.
                    //
                    // It qualifies for this arm because it is **synchronous and spawns**: it takes
                    // nothing the turn holds and returns immediately. That is the bar, and it is
                    // why its read-only siblings are not here — `RequestContextComposition` awaits
                    // the transcript this turn is still writing, and an arm that awaits stops the
                    // loop from seeing `Cancel` until it finishes.
                    Some(Action::LoadFileDiff { request_id, path }) => {
                        self.handle_load_file_diff(request_id, path, event_tx);
                    }
                    // The FILES tree and its preview clear the same bar, for the same reader
                    Some(Action::ListDirectory { request_id, path, include_ignored }) => {
                        self.handle_list_directory(request_id, path, include_ignored, event_tx);
                    }
                    Some(Action::ReadProjectFile { request_id, path }) => {
                        self.handle_read_project_file(request_id, path, event_tx);
                    }
                    // A review edit clears the same bar: handing it to the desk is a send that
                    // never waits. Deferred, every comment written while the agent worked on the
                    // last batch sat behind the whole turn, and the review refused the next one.
                    Some(
                        action @ (Action::EditReview { .. }
                        | Action::RecordReviewProgress { .. }
                        | Action::RecordReviewSeen { .. }),
                    ) => self.forward_to_review_desk(action),
                    // Awaits only locks the turn takes and drops at once, so `Cancel` still gets
                    // through. Deferred, a switch made to stop the agent writing reached it only
                    // after the turn had written, and the label never moved meanwhile
                    Some(Action::SetMode(name)) => self.set_mode_mid_turn(name, event_tx).await,
                    // Not actionable mid-turn; the main loop takes it once the turn is over
                    Some(other) => self.defer(other),
                    None => {
                        // The UI channel closed: reject pending prompts and let the work unwind.
                        self.permission_broker.cancel_all();
                        self.question_broker.cancel_all();
                        return (&mut work).await;
                    }
                }
            }
        }
    }
}

impl Engine {
    /// Stops the work in flight: the cancel flag, and every prompt it is waiting on answered no.
    async fn interrupt(&self, event_tx: &mpsc::Sender<Event>) {
        self.set_cancelled(true);
        self.permission_broker.cancel_all();
        self.question_broker.cancel_all();
        let state = self.state_snapshot("cancelled", false).await;
        let _ = event_tx.send(Event::StateChanged(state)).await;
    }
}

/// Lines `continuations` up behind what was already in the channel when they were queued, which
/// is where a send would have put them. A `Close` among them goes first, as it does mid-turn: the
/// front-end is leaving, and the work queued behind it is not wanted.
///
/// Bounded by what is queued now, so a sender that keeps refilling the channel cannot hold the
/// engine here.
fn line_up(
    continuations: &mut VecDeque<Action>,
    deferred: &mut VecDeque<Action>,
    action_rx: &mut mpsc::Receiver<Action>,
) {
    if continuations.is_empty() {
        return;
    }
    for _ in 0..action_rx.len() {
        match action_rx.try_recv() {
            Ok(Action::Close) => deferred.push_front(Action::Close),
            Ok(action) => deferred.push_back(action),
            Err(_) => break,
        }
    }
    deferred.extend(continuations.drain(..));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A continuation waits behind what was already in the channel, the place a send would have
    /// put it, and behind what a turn deferred before that.
    #[tokio::test]
    async fn a_continuation_is_served_after_what_was_already_waiting() {
        let (tx, mut rx) = mpsc::channel(4);
        let mut deferred = VecDeque::from([Action::Close]);
        tx.send(Action::RefreshWorkspace).await.unwrap();
        tx.send(Action::Cancel).await.unwrap();
        let mut continuations = VecDeque::from([Action::NewSession]);

        line_up(&mut continuations, &mut deferred, &mut rx);

        let order: Vec<String> = deferred.iter().map(|action| format!("{action:?}")).collect();
        assert_eq!(order, ["Close", "RefreshWorkspace", "Cancel", "NewSession"]);
        assert!(continuations.is_empty() && rx.is_empty());
    }

    /// A close drained out of the channel goes ahead of everything waiting, where a running turn
    /// would have served it: behind them, the next turn would start and never see it.
    #[tokio::test]
    async fn a_close_drained_from_the_channel_is_served_first() {
        let (tx, mut rx) = mpsc::channel(4);
        let submission = Action::SubmitUserMessage {
            content: "queued".into(),
            images: Vec::new(),
            remediation: None,
        };
        let mut deferred = VecDeque::from([submission]);
        tx.send(Action::Close).await.unwrap();
        let mut continuations = VecDeque::from([Action::NewSession]);

        line_up(&mut continuations, &mut deferred, &mut rx);

        assert!(
            matches!(
                deferred.make_contiguous(),
                [Action::Close, Action::SubmitUserMessage { .. }, Action::NewSession]
            ),
            "{deferred:?}"
        );
    }

    /// With nothing handed over, the channel is left to `recv`, so an idle engine still waits.
    #[tokio::test]
    async fn no_continuation_leaves_the_channel_alone() {
        let (tx, mut rx) = mpsc::channel(4);
        tx.send(Action::RefreshWorkspace).await.unwrap();
        let mut deferred = VecDeque::new();

        line_up(&mut VecDeque::new(), &mut deferred, &mut rx);

        assert!(deferred.is_empty());
        assert_eq!(rx.len(), 1);
    }
}
