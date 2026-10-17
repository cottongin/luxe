//! Completion challenger — forces verification + confirmations before ending the loop.
//!
//! Prevents premature completion by requiring a forced verification turn and multiple
//! confirmations before the loop can end.

use crate::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint};
use crate::loop_controller::LoopController;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info};

struct HookState {
    confirmations_required: u8,
    confirmations_received: u8,
}

/// A hook that challenges premature completion requests.
///
/// When `loop_control action="complete"` is received, forces a verification turn
/// listing every phase + Definition-of-Done item with status. Only stops after
/// configurable number of confirmations.
pub struct CompletionChallengerHook {
    /// Loop controller for checking state.
    loop_controller: Arc<LoopController>,
    /// What a completion claim has to clear, shared with the phase gate the loop already runs.
    gate: Arc<crate::phase_gate::PhaseGate>,
    /// Mutable hook state (confirmations).
    state: Arc<Mutex<HookState>>,
}

impl std::fmt::Debug for CompletionChallengerHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompletionChallengerHook").finish()
    }
}

impl CompletionChallengerHook {
    /// Create a new completion challenger hook.
    pub async fn new(loop_controller: Arc<LoopController>) -> Self {
        // Read the documented config knob from LoopState; fall back to 2 if unset (0).
        let state = loop_controller.get_state().await;
        let confirmations_required = if state.confirmations_required == 0 {
            2 // >=2 so the first `complete` surfaces the verification turn; a second call stops.
        } else {
            state.confirmations_required
        };

        Self {
            loop_controller,
            gate: Arc::new(crate::phase_gate::PhaseGate::new()),
            state: Arc::new(Mutex::new(HookState {
                confirmations_required,
                confirmations_received: 0,
            })),
        }
    }

    /// Judge completion claims with `gate`, the same one the phase gate runs.
    pub fn with_gate(mut self, gate: Arc<crate::phase_gate::PhaseGate>) -> Self {
        self.gate = gate;
        self
    }

    /// Set the number of confirmations required.
    pub async fn set_confirmations_required(&self, n: u8) {
        let mut state = self.state.lock().await;
        state.confirmations_required = n;
    }

    /// Increment the confirmation counter.
    pub async fn increment_confirmation(&self) {
        let mut state = self.state.lock().await;
        if state.confirmations_received < state.confirmations_required {
            state.confirmations_received += 1;
        }
    }

    /// Reset the confirmation counter.
    pub async fn reset_confirmations(&self) {
        let mut state = self.state.lock().await;
        state.confirmations_received = 0;
    }

    /// Check if we have enough confirmations to stop.
    async fn has_enough_confirmations(&self) -> bool {
        let state = self.state.lock().await;
        state.confirmations_received >= state.confirmations_required
    }

    /// Get the verification message (asks the model to list all phases + Definition-of-Done items).
    fn verification_message(&self) -> String {
        "VERIFICATION REQUIRED: List every phase and Definition-of-Done item with status. Only call loop_control complete after re-confirmation.".to_string()
    }

    /// Get the final stop message.
    fn final_stop_message(&self) -> String {
        "All phases verified. Ending loop.".to_string()
    }
}

#[async_trait]
impl Hook for CompletionChallengerHook {
    fn name(&self) -> &'static str {
        "completion-challenger"
    }

    fn interest(&self) -> &[HookPoint] {
        &[HookPoint::LoopControl]
    }

    async fn on(&self, _point: HookPoint, ev: &HookEvent, cx: &HookCtx) -> HookOutcome {
        let loop_control = match ev {
            HookEvent::LoopControl(lc) => lc,
            _ => return HookOutcome::Continue,
        };

        if !self.loop_controller.is_running().await {
            debug!("loop not running, ignoring");
            return HookOutcome::Continue;
        }

        match loop_control {
            crate::hook::LoopControl::Complete => {
                // The gate first: `complete` is a claim like `progress`, and a door with no gate on
                // it lets an agent leave without the reviewer ever being consulted. A refused claim
                // does not count as a confirmation either: the agent has to fix the work and ask
                // again.
                let phase = self.loop_controller.get_state().await.current_phase;
                cx.notify("the work is claimed complete — running the gate");
                match self.gate.run(phase, "the work", &|line| cx.notify(line)).await {
                    crate::phase_gate::GateOutcome::Passed => {}
                    crate::phase_gate::GateOutcome::Refused(message) => {
                        info!("the gate refused the completion claim");
                        cx.notify(format!(
                            "completion held: {}",
                            crate::hooks::first_line(&message)
                        ));
                        return HookOutcome::ContinueLoop {
                            message: format!("{message}\n\nThe loop is still running."),
                        };
                    }
                    crate::phase_gate::GateOutcome::Held(message) => {
                        return crate::hooks::review_integrator::pause_for_a_person(
                            &self.loop_controller,
                            cx,
                            message,
                        )
                        .await;
                    }
                }

                // Require verification + confirmations.
                // Increment confirmation count on each complete request.
                self.increment_confirmation().await;
                if self.has_enough_confirmations().await {
                    info!("enough confirmations, stopping loop");
                    cx.notify("every phase verified — the run is complete");
                    let _ = self.loop_controller.stop().await;
                    return HookOutcome::ContinueLoop { message: self.final_stop_message() };
                }
                cx.notify("completion claimed — asking for a verification pass first");
                HookOutcome::ContinueLoop { message: self.verification_message() }
            }
            // Progress is the review integrator's business: a phase advances when the acceptance
            // gate agrees it may, and this hook does not run that gate. Advancing here as well is
            // how the counter moved twice for one claim, and moved at all for a claim the gate
            // would have refused.
            crate::hook::LoopControl::Progress { .. } => HookOutcome::Continue,
            crate::hook::LoopControl::Blocked { reason } => {
                info!(reason = %reason, "loop blocked");
                cx.notify(format!(
                    "blocked, waiting for a person: {}",
                    crate::hooks::first_line(reason)
                ));
                let _ = self.loop_controller.pause().await;
                HookOutcome::StopLoop { reason: format!("Blocked: {reason}") }
            }
        }
    }
}

#[cfg(test)]
mod tests;
