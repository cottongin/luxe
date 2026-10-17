//! Requested review — `/review`: a second opinion in a fresh context, handed back to the agent.
//!
//! The reviewer is a separate engine on its own session ([`super::OnDemandReview`]), so it reads the
//! change without the reasoning that produced it, which is most of what makes a review worth asking
//! for. It runs on a task of its own so the session stays usable meanwhile, and its verdict comes
//! back through the action channel as [`Action::ReviewReturned`]: findings it would fix become the
//! agent's next turn, the way a refused phase does in the loop, and a clean verdict is only shown.

use super::*;
use crate::event::RequestedReviewVerdict;
use crate::phase_review::{PhaseReviewOutcome, PhaseReviewRequest, ReviewReason};
use tracing::info;

impl Engine {
    /// Starts a review of the uncommitted work, unless one is running or none is set up.
    pub(super) async fn request_review(
        &self,
        focus: Option<String>,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let notice = |text: &str| Event::Notice(text.to_string());
        let (Some(review), Some(action_tx)) =
            (self.config.reviewer.clone(), self.config.action_tx.clone())
        else {
            let _ = event_tx.send(notice("This session has no reviewer to ask.")).await;
            return;
        };
        if self.review_running.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let _ = event_tx
                .send(notice("A review is already running. Its findings will show here."))
                .await;
            return;
        }
        info!(model = %review.label, ?focus, "starting a requested review");
        let _ = event_tx.send(Event::ReviewStarted { model: review.label.clone() }).await;
        tokio::spawn(async move {
            let outcome = run_review(&review, focus).await;
            let _ = action_tx.send(Action::ReviewReturned(outcome)).await;
        });
    }

    /// Shows what a requested review found, and hands any findings to the agent as a turn.
    pub(super) async fn review_returned(
        &self,
        outcome: PhaseReviewOutcome,
        event_tx: &mpsc::Sender<Event>,
    ) {
        self.review_running.store(false, std::sync::atomic::Ordering::SeqCst);
        let (verdict, findings) = match &outcome {
            PhaseReviewOutcome::Continue => (RequestedReviewVerdict::Continue, Vec::new()),
            PhaseReviewOutcome::Fix(findings) => (RequestedReviewVerdict::Fix, findings.clone()),
            PhaseReviewOutcome::Unavailable(reason) => {
                (RequestedReviewVerdict::Unavailable, vec![reason.clone()])
            }
        };
        info!(?verdict, findings = findings.len(), "a requested review came back");
        let _ = event_tx.send(Event::ReviewFinished { verdict, findings: findings.clone() }).await;
        if verdict == RequestedReviewVerdict::Fix {
            self.submit_to_self(review_turn(&findings));
        }
    }
}

/// Gathers the evidence and asks the reviewer, reporting a failure to gather as no review at all.
async fn run_review(review: &OnDemandReview, focus: Option<String>) -> PhaseReviewOutcome {
    let diff = match review.context.diff().await {
        Ok(diff) => diff,
        Err(error) => return PhaseReviewOutcome::Unavailable(error),
    };
    if diff.trim().is_empty() {
        return PhaseReviewOutcome::Unavailable(
            "There is nothing uncommitted to review: the working tree matches HEAD.".into(),
        );
    }
    let planning = review.context.planning(0).await;
    let files = review.context.files(&diff, &planning).await;
    let request = PhaseReviewRequest {
        reason: ReviewReason::Requested { focus },
        phase: 0,
        diff,
        planning,
        gate: String::new(),
        files,
    };
    review.reviewer.review(request).await
}

/// The turn a review's findings become: the findings, and what to do with the ones that are wrong.
///
/// The agent has the context the reviewer was denied on purpose, so it is the right one to tell a
/// real defect from a misreading; a reviewer is a second opinion, not an order.
fn review_turn(findings: &[String]) -> String {
    let list: String = findings.iter().map(|finding| format!("- {finding}\n")).collect();
    format!(
        "A reviewer in a fresh context read the uncommitted changes and would fix these:\n\n{list}\n\
         Fix the ones that hold up. Where one is wrong or out of scope, say why instead of \
         changing the code."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The agent is told it may disagree, or a reviewer's misreading becomes a change nobody asked for.
    #[test]
    fn a_review_turn_carries_every_finding_and_leave_to_disagree() {
        let turn = review_turn(&["src/a.rs:3 unwraps a fallible read".into(), "no test".into()]);
        assert!(turn.contains("- src/a.rs:3 unwraps a fallible read\n"), "{turn}");
        assert!(turn.contains("- no test\n"), "{turn}");
        assert!(turn.contains("say why instead"), "{turn}");
    }
}
