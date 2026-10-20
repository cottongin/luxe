//! Agent turn orchestration — everything that wraps one `run_turn` call.
//!
//! `Action::SubmitUserMessage` is not just "run a turn": a review baseline is captured on both
//! sides of it, `BeforeAgentStart` may abort or add context, the outcome has to be reported back
//! to whichever review submission asked for it, and `AgentEnd` decides whether the engine keeps
//! going. Those five concerns lived inline in the action loop, where the loop's own `break` and
//! `continue` were the only way to say what happened next; [`TurnFlow`] says it explicitly instead.

use super::*;

/// What the engine's action loop does once a turn is finished.
pub(crate) enum TurnFlow {
    /// Keep serving actions.
    Continue,
    /// A hook asked the engine to stop. The loop ends.
    Stop,
}

impl Engine {
    /// Runs one agent turn end to end, with its baselines, hooks, and outcome reporting.
    pub(crate) async fn run_agent_turn(
        &mut self,
        content: String,
        images: Vec<ImageAttachment>,
        remediation: Option<RemediationTurnContext>,
        event_tx: &mpsc::Sender<Event>,
        action_rx: &mut mpsc::Receiver<Action>,
    ) -> TurnFlow {
        // Resolved per turn: no switch can land mid-turn, but one can between turns
        let hook_cx = &self.hook_context(event_tx).await;
        let feedback_submission_id =
            remediation.as_ref().and_then(|context| context.submission_id.clone());
        let turn_id = AgentTurnId::new();
        debug!(turn_id = %turn_id.as_str(), "starting agent turn");
        // First, so a hook refusing the turn below still ends one that had started
        let _ = event_tx.send(Event::TurnStarted).await;

        *self.last_turn_baseline.lock().await = None;
        let retained_baseline = match self.capture_pre_turn_baseline(&turn_id).await {
            Ok(baseline) => baseline,
            Err(error) => {
                warn!(turn_id = %turn_id.as_str(), %error, "pre-turn review baseline unavailable");
                None
            }
        };

        let Some(injected) = self.before_agent_start(hook_cx, event_tx).await else {
            return TurnFlow::Continue;
        };

        let turn = self.run_turn(content, images, injected, remediation, event_tx.clone());
        let outcome = self.serviced(turn, event_tx, action_rx).await;
        self.report_turn_outcome(outcome, feedback_submission_id, hook_cx, event_tx).await;

        if let Some(baseline) = retained_baseline {
            self.retain_post_turn_baseline(baseline, &turn_id, event_tx).await;
        }
        self.after_agent_end(hook_cx, event_tx).await
    }

    /// Runs the turn a re-send readied, or reports why it could not be readied.
    ///
    /// Args: `(readied message, what was asked, events, actions)`.
    pub(crate) async fn run_prepared_turn(
        &mut self,
        ready: Result<String, EngineError>,
        purpose: &str,
        event_tx: &mpsc::Sender<Event>,
        action_rx: &mut mpsc::Receiver<Action>,
    ) -> TurnFlow {
        match ready {
            Ok(content) => {
                self.run_agent_turn(content, Vec::new(), None, event_tx, action_rx).await
            }
            Err(error) => {
                error!(%error, purpose, "could not ready the turn");
                let _ = event_tx.send(Event::Error(format!("{purpose}: {error}"))).await;
                TurnFlow::Continue
            }
        }
    }

    /// Fires `BeforeAgentStart`, returning the context to inject, or `None` to skip the turn.
    ///
    /// A hook gets one chance per turn to abort it (`StopLoop`) or to add system-prompt context.
    /// Aborting here is not a failure — the turn simply does not happen — so nothing is reported
    /// beyond the `TurnEnded` that tells the front-end to stop waiting.
    async fn before_agent_start(
        &self,
        hook_cx: &HookCtx,
        event_tx: &mpsc::Sender<Event>,
    ) -> Option<Option<String>> {
        let outcome = self
            .config
            .hook_registry
            .invoke(
                HookPoint::BeforeAgentStart,
                &HookEvent::Other(serde_json::Value::Null),
                hook_cx,
            )
            .await;
        match outcome {
            HookOutcome::StopLoop { reason } => {
                tracing::info!(%reason, "before-agent-start hook requested stop");
                let _ = event_tx.send(Event::TurnEnded { reason: StopReason::Other(reason) }).await;
                None
            }
            HookOutcome::InjectContext(context) => Some(Some(context)),
            _ => Some(None),
        }
    }

    /// Reports how the turn went, to the review submission that asked for it and to the reader.
    ///
    /// A failed remediation turn also releases its submission from the accepted set, so the
    /// reviewer can retry it — otherwise the failure would look permanent.
    async fn report_turn_outcome(
        &mut self,
        outcome: Result<Option<EntryId>, EngineError>,
        submission_id: Option<crate::guided_review::SubmissionId>,
        hook_cx: &HookCtx,
        event_tx: &mpsc::Sender<Event>,
    ) {
        match outcome {
            Ok(_) => {
                if let Some(submission_id) = submission_id {
                    self.review_desk
                        .send(super::review_desk::DeskCommand::Completed(submission_id.clone()));
                    let _ =
                        event_tx.send(Event::ReviewFeedbackTurnCompleted { submission_id }).await;
                }
            }
            Err(error) => {
                error!(%error, "turn failed");
                let message = error.to_string().chars().take(512).collect::<String>();
                if let Some(submission_id) = submission_id {
                    self.accepted_feedback_submissions.remove(&submission_id);
                    self.review_desk
                        .send(super::review_desk::DeskCommand::Failed(submission_id.clone()));
                    let _ = event_tx
                        .send(Event::ReviewFeedbackTurnFailed {
                            submission_id,
                            message: message.clone(),
                        })
                        .await;
                }
                let _ = event_tx.send(Event::Error(format!("turn error: {message}"))).await;
                // A turn that failed still stopped, and only `TurnEnded` says so. Without it a
                // front-end waits forever on a turn that is already over: the TUI keeps its
                // spinner up, `--format json` never closes the turn, and an RPC client hangs.
                let _ = event_tx
                    .send(Event::TurnEnded {
                        reason: crate::event::StopReason::Other("error".to_string()),
                    })
                    .await;
                // `HookPoint::Error` was declared and never fired, so nothing counted a failed turn:
                // the loop's `max_consecutive_errors` budget only ever saw *tool* errors, and its
                // rate-limit backoff had no trigger at all. A provider outage therefore span the
                // loop at full speed until an unrelated guard cut it off.
                self.config
                    .hook_registry
                    .invoke(
                        HookPoint::Error,
                        &HookEvent::Other(serde_json::json!({ "message": message })),
                        hook_cx,
                    )
                    .await;
            }
        }
    }

    /// Closes the turn's review baseline and announces that "last turn" is now reviewable.
    async fn retain_post_turn_baseline(
        &self,
        baseline: RetainedTurnBaseline,
        turn_id: &AgentTurnId,
        event_tx: &mpsc::Sender<Event>,
    ) {
        match self.capture_post_turn_baseline(&baseline).await {
            Ok(()) => {
                let _ = event_tx
                    .send(Event::LastTurnAvailable {
                        session_id: baseline.session_id.clone(),
                        turn_id: baseline.turn_id.as_str().to_string(),
                    })
                    .await;
                *self.last_turn_baseline.lock().await = Some(baseline);
            }
            Err(error) => {
                warn!(turn_id = %turn_id.as_str(), %error, "post-turn review snapshot unavailable");
            }
        }
    }

    /// Fires `AgentEnd` and acts on what it decided: stop, self-continue, or nudge a compaction.
    ///
    /// The turn's effective-context percent rides along so the termination hook can judge on real
    /// occupancy of the *usable* window rather than a formula over the raw one.
    async fn after_agent_end(&self, hook_cx: &HookCtx, event_tx: &mpsc::Sender<Event>) -> TurnFlow {
        let used = self.last_context_tokens.load(std::sync::atomic::Ordering::Relaxed);
        let active = self.effective_model().await;
        let usable = crate::compaction::usable_context_window(
            active.context_window as u64,
            active.max_tokens,
            self.config.compaction_settings.reserve_tokens,
        );
        let effective_percent = crate::compaction::context_fullness_percent(used as u64, usable);
        let outcome = self
            .config
            .hook_registry
            .invoke(
                HookPoint::AgentEnd,
                &HookEvent::Other(
                    serde_json::json!({ "effective_context_percent": effective_percent }),
                ),
                hook_cx,
            )
            .await;

        match outcome {
            HookOutcome::StopLoop { reason } => {
                tracing::info!(%reason, "agent-end hook requested stop");
                let _ = event_tx.send(Event::TurnEnded { reason: StopReason::Other(reason) }).await;
                return TurnFlow::Stop;
            }
            HookOutcome::ContinueLoop { message } => {
                tracing::debug!(%message, "agent-end hook requested self-continue");
                self.submit_to_self(message);
            }
            _ => {}
        }
        TurnFlow::Continue
    }

    /// Queues a message the engine wrote to itself, as an ordinary turn.
    ///
    /// Queued rather than run here, so one-turn-at-a-time holds: a self-continuation waits its
    /// place like any other action.
    pub(super) fn submit_to_self(&self, content: String) {
        self.enqueue_continuation(Action::SubmitUserMessage {
            content: bound_steer(content),
            images: Vec::new(),
            remediation: None,
        });
    }
}

/// Longest message the engine will submit to itself, in characters.
///
/// Generous by design: the loop's own steer carries a phase of the plan, and a refused gate carries
/// a reviewer's whole fix list. Neither comes near this. What it stops is the class of accident a
/// hook can have — the re-anchor hook once read three files whole, which in one working tree was
/// 372 KB of journal, and every byte of that would have become a permanent user message.
const MAX_STEER_CHARS: usize = 12_000;

/// A hook-authored message, cut to something a transcript can carry.
///
/// Bounded here rather than in each hook, because a hook that forgets is exactly the failure this
/// exists for. A steer is a *user message*: it is permanent history, it is re-sent with every later
/// request, and nothing downstream trims it. The cut says what happened, so a model reading a
/// truncated instruction knows it is truncated.
fn bound_steer(content: String) -> String {
    if content.chars().count() <= MAX_STEER_CHARS {
        return content;
    }
    let kept: String = content.chars().take(MAX_STEER_CHARS).collect();
    let at = kept.rsplit_once('\n').map(|(head, _)| head).unwrap_or(&kept);
    warn!(
        chars = content.chars().count(),
        cap = MAX_STEER_CHARS,
        "a hook's steer was too long for a transcript and was cut"
    );
    format!(
        "{at}\n\n[… the rest of this message was cut: it exceeded {MAX_STEER_CHARS} characters]"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ordinary steer is passed through byte for byte.
    #[test]
    fn a_steer_within_the_cap_is_untouched() {
        let steer =
            "Continue building. You are on Phase 1 — Ranking.\n\n## What phase 1 is\n\nRank.";
        assert_eq!(bound_steer(steer.to_string()), steer);
    }

    /// A hook that hands over a whole file gets a message, not a transcript full of it.
    ///
    /// The break this guards against, which shipped once: the re-anchor hook read the journal, the
    /// plan and the spec *whole* — 372 KB of journal in one measured tree — and submitted them as a
    /// turn. A user message is permanent and is re-sent with every request after it, so the cost of
    /// one hook's mistake is paid for the rest of the session.
    #[test]
    fn a_runaway_steer_is_cut_and_says_so() {
        let runaway = "a line of journal\n".repeat(MAX_STEER_CHARS);
        let bounded = bound_steer(runaway);

        assert!(bounded.chars().count() < MAX_STEER_CHARS + 200, "cut, plus the note about it");
        assert!(bounded.contains("was cut"), "and the model is told: {}", &bounded[..80]);
        assert!(bounded.starts_with("a line of journal"), "the beginning is what survives");
    }
}
