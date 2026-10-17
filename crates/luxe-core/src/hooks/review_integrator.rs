//! Review integrator hook — the gate a claimed phase has to clear before the phase advances.
//!
//! Fires when the agent calls `loop_control action="progress"`, which is a *claim* that the phase is
//! done rather than the fact of it. [`PhaseGate`] decides, and a refusal reaches the model as the
//! tool's own result rather than a log entry.

use crate::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint, LoopControl};
use crate::loop_controller::LoopController;
use crate::phase_gate::{GateOutcome, PhaseGate};
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Runs the phase gate when a phase is claimed complete, and advances only if it passes.
///
/// The sole owner of phase advancement. Anything else moving that counter would be racing this,
/// which is what happened while this hook fired on every `AgentEnd`.
pub struct ReviewIntegratorHook {
    /// Loop controller for tracking phase and state.
    loop_controller: Arc<LoopController>,
    /// What a claim has to clear.
    gate: Arc<PhaseGate>,
    /// Project root, for re-anchoring the review baseline when a phase closes.
    repo_root: std::path::PathBuf,
}

impl std::fmt::Debug for ReviewIntegratorHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReviewIntegratorHook").finish()
    }
}

impl ReviewIntegratorHook {
    /// A hook whose gate accepts everything.
    pub fn new(loop_controller: Arc<LoopController>) -> Self {
        Self {
            loop_controller,
            gate: Arc::new(PhaseGate::new()),
            repo_root: std::path::PathBuf::from("."),
        }
    }

    /// The project the loop is working in, whose `HEAD` anchors each phase's review.
    pub fn in_project(mut self, repo_root: impl Into<std::path::PathBuf>) -> Self {
        self.repo_root = repo_root.into();
        self
    }

    /// Judge claims with `gate`, shared with whatever else has to consult it.
    pub fn with_gate(mut self, gate: Arc<PhaseGate>) -> Self {
        self.gate = gate;
        self
    }

    /// Move the phase on, once the gate has agreed it may.
    ///
    /// Always by one, whatever number the claim carried. A claim is the agent saying "this phase is
    /// finished", not choosing where the counter lands: honoring the number let a model with a
    /// one-phase plan in front of it walk the counter to three, because it kept naming a new phase
    /// each time it claimed. Setting an explicit phase is a human's call, and `luxe loop phase <n>`
    /// is how it is made.
    async fn advance_on_pass(&self, claimed: Option<u32>) -> Result<(), String> {
        let current = self.loop_controller.get_state().await.current_phase;
        if let Some(claimed) = claimed {
            if claimed != current {
                // `debug`, not `warn`: the number is for the record and the counter is ours, so a
                // claim that names the phase it thinks comes *next* is a disagreement about
                // spelling rather than an oddity the run coped with. It fired on every single
                // claim of a healthy four-phase run.
                debug!(claimed, current, "the claim named a phase other than the current one");
            }
        }
        info!(from = current, "gate passed, advancing to the next phase");
        self.loop_controller.advance_phase().await.map_err(|e| e.to_string())?;
        // The phase that just closed is the next one's starting point, so the review that follows
        // reads only what the next phase did.
        self.loop_controller.record_phase_baseline(&self.repo_root).await;
        Ok(())
    }
}

#[async_trait]
impl Hook for ReviewIntegratorHook {
    fn name(&self) -> &'static str {
        "review-integrator"
    }

    fn interest(&self) -> &[HookPoint] {
        &[HookPoint::LoopControl]
    }

    async fn on(&self, _point: HookPoint, ev: &HookEvent, cx: &HookCtx) -> HookOutcome {
        // Only a progress claim closes a phase. `complete` is the completion challenger's, which
        // runs the same gate before it accepts, and `blocked` belongs to whoever is being asked for
        // a decision.
        let HookEvent::LoopControl(LoopControl::Progress { phase }) = ev else {
            return HookOutcome::Continue;
        };
        debug!("phase claimed complete, running the gate");

        let current = self.loop_controller.get_state().await.current_phase;
        cx.notify(format!("phase {current} claimed complete — running the gate"));
        match self.gate.run(current, "this phase", &|line| cx.notify(line)).await {
            GateOutcome::Passed => {
                if let Err(error) = self.advance_on_pass(*phase).await {
                    error!(%error, "the gate passed but the phase could not be advanced");
                    let message = format!(
                        "The gate accepted phase {current}, but the loop could not save the new \
                         phase ({error}), so it is paused with phase {current} still open. Check \
                         that `.luxe/loop/` is writable, then run `luxe loop start` again."
                    );
                    return pause_for_a_person(&self.loop_controller, cx, message).await;
                }
                cx.notify(format!("phase {current} accepted — now on phase {}", current + 1));
                // The claim's own result says the gate agreed, as a refusal says why: echoing the
                // argument back would leave the agent unable to tell whether it was accepted.
                HookOutcome::ContinueLoop {
                    message: format!(
                        "The gate accepted phase {current}. The loop is now on phase {}: read what \
                         `PLAN.md` says about it before starting.",
                        current + 1
                    ),
                }
            }
            GateOutcome::Refused(message) => {
                info!("the gate refused the claim; the phase stays open");
                cx.notify(format!("phase {current} held: {}", crate::hooks::first_line(&message)));
                HookOutcome::ContinueLoop {
                    message: format!("{message}\n\nThe phase number has not moved."),
                }
            }
            GateOutcome::Held(message) => {
                pause_for_a_person(&self.loop_controller, cx, message).await
            }
        }
    }
}

/// Pause the loop on a claim the gate could not decide, telling the reader and the model why.
pub(crate) async fn pause_for_a_person(
    controller: &LoopController,
    cx: &HookCtx,
    message: String,
) -> HookOutcome {
    warn!(reason = %message, "the gate could not decide; pausing the loop");
    cx.notify(format!("paused for a person: {}", crate::hooks::first_line(&message)));
    if let Err(error) = controller.pause().await {
        error!(%error, "the loop could not be paused");
    }
    HookOutcome::StopLoop { reason: message }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn ctx() -> HookCtx {
        HookCtx {
            session_id: "test".to_string(),
            event_tx: tokio::sync::mpsc::channel(1).0,
            hook_registry: crate::hook::HookRegistry::new(),
        }
    }

    async fn running_controller(dir: &tempfile::TempDir) -> Arc<LoopController> {
        let controller =
            Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));
        controller.start().await.unwrap();
        controller
    }

    /// With no gate configured, a progress claim is taken at its word.
    ///
    /// A loop that has not been given acceptance commands must still be able to move through its
    /// plan; the gate is what makes a claim *checkable*, not what makes it possible.
    #[tokio::test]
    async fn a_claim_advances_when_no_gate_is_configured() {
        let dir = tempdir().unwrap();
        let controller = running_controller(&dir).await;
        let hook = ReviewIntegratorHook::new(controller.clone());

        let outcome = hook
            .on(
                HookPoint::LoopControl,
                &HookEvent::LoopControl(LoopControl::Progress { phase: None }),
                &ctx(),
            )
            .await;

        // The claim's own result says what happened to it; echoing the argument back would leave an
        // agent that just claimed a phase unable to tell an acceptance from a refusal.
        let HookOutcome::ContinueLoop { message } = outcome else {
            panic!("an accepted claim answers the agent")
        };
        assert!(message.contains("accepted phase 0"), "{message}");
        assert!(message.contains("phase 1"), "and says where the loop is now: {message}");
        assert_eq!(controller.get_state().await.current_phase, 1);
    }

    /// An acceptance the loop cannot save is not announced as one.
    ///
    /// The break this catches: the phase moved in memory, the save failed, and the agent was told
    /// to start the next phase, which a restart would not remember.
    #[tokio::test]
    async fn an_acceptance_that_cannot_be_saved_pauses_with_the_phase_unmoved() {
        let dir = tempdir().unwrap();
        let controller = running_controller(&dir).await;
        // a directory where the state file belongs makes every later save fail
        std::fs::remove_file(dir.path().join("state.json")).unwrap();
        std::fs::create_dir(dir.path().join("state.json")).unwrap();
        let hook = ReviewIntegratorHook::new(controller.clone());

        let outcome = hook
            .on(
                HookPoint::LoopControl,
                &HookEvent::LoopControl(LoopControl::Progress { phase: None }),
                &ctx(),
            )
            .await;

        let HookOutcome::StopLoop { reason } = outcome else {
            panic!("a failed save must not read as an acceptance, got {outcome:?}")
        };
        assert!(reason.contains("could not save"), "{reason}");
        assert_eq!(controller.get_state().await.current_phase, 0, "the live phase did not move");
        assert!(!controller.is_running().await, "and the loop waits for a person");
    }

    /// A claim that names a phase still advances by exactly one.
    ///
    /// The break this catches, measured on a real run: honoring the number let a local model with a
    /// one-phase plan walk the counter to three, because it named a new phase every time it claimed
    /// one. The counter is the gate's; `luxe loop phase <n>` is how a human sets it.
    #[tokio::test]
    async fn a_claim_cannot_choose_its_own_phase_number() {
        let dir = tempdir().unwrap();
        let controller = running_controller(&dir).await;
        let hook = ReviewIntegratorHook::new(controller.clone());

        hook.on(
            HookPoint::LoopControl,
            &HookEvent::LoopControl(LoopControl::Progress { phase: Some(7) }),
            &ctx(),
        )
        .await;

        assert_eq!(controller.get_state().await.current_phase, 1, "one step, not seven");
    }

    /// Anything that is not a loop-control directive leaves the phase alone.
    #[tokio::test]
    async fn an_unrelated_event_is_not_a_phase_claim() {
        let dir = tempdir().unwrap();
        let controller = running_controller(&dir).await;
        let hook = ReviewIntegratorHook::new(controller.clone());

        let outcome =
            hook.on(HookPoint::AgentEnd, &HookEvent::Other(serde_json::json!({})), &ctx()).await;

        assert!(matches!(outcome, HookOutcome::Continue));
        assert_eq!(controller.get_state().await.current_phase, 0);
    }
}
