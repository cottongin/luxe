//! Context-aware termination hook — ends a loop run when the context window is nearly full.
//!
//! Reads the turn's effective-context percent — occupancy of the usable window (see
//! [`crate::compaction::usable_context_window`]) — from the `AgentEnd` event the engine attaches.
//! At the `pause_percent` high-water mark it ends the run, so the next one starts lean.
//!
//! Compacting is the engine's job, at the threshold `luxe loop start` gives it. This hook does not
//! answer `compact_percent`: a "wrap up and summarize" note compacts nothing, and a session grows
//! without bound while carrying one on every turn.

use crate::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint};
use crate::loop_controller::LoopController;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info};

struct HookState {
    grace_turns_remaining: u32,
}

/// Pull the effective-context percent the engine attached to the `AgentEnd` event (0.0 when
/// absent — e.g. no usage observed yet).
fn effective_percent(ev: &HookEvent) -> f64 {
    match ev {
        HookEvent::Other(v) => {
            v.get("effective_context_percent").and_then(serde_json::Value::as_f64).unwrap_or(0.0)
        }
        _ => 0.0,
    }
}

/// A hook that ends a loop run once effective context passes `pause_percent`.
///
/// Fires on `HookPoint::AgentEnd`.
pub struct ContextAwareTerminationHook {
    /// Loop controller for checking state and context thresholds.
    loop_controller: Arc<LoopController>,
    /// Mutable hook state (effective context, grace period).
    state: Arc<Mutex<HookState>>,
}

impl std::fmt::Debug for ContextAwareTerminationHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContextAwareTerminationHook").finish()
    }
}

impl ContextAwareTerminationHook {
    /// Create a new context-aware termination hook.
    pub fn new(loop_controller: Arc<LoopController>) -> Self {
        Self {
            loop_controller,
            state: Arc::new(Mutex::new(HookState { grace_turns_remaining: 0 })),
        }
    }

    /// Set grace period turns (after compaction).
    pub async fn set_grace_period(&self, turns: u32) {
        let mut state = self.state.lock().await;
        state.grace_turns_remaining = turns;
    }
}

#[async_trait]
impl Hook for ContextAwareTerminationHook {
    fn name(&self) -> &'static str {
        "context-aware-termination"
    }

    fn interest(&self) -> &[HookPoint] {
        &[HookPoint::AgentEnd]
    }

    async fn on(&self, _point: HookPoint, ev: &HookEvent, cx: &HookCtx) -> HookOutcome {
        let percent = effective_percent(ev);
        let mut state = self.state.lock().await;

        // Decrement grace period.
        if state.grace_turns_remaining > 0 {
            state.grace_turns_remaining -= 1;
        }

        if !self.loop_controller.is_running().await {
            debug!("loop not running, skipping");
            return HookOutcome::Continue;
        }

        let controller_state = self.loop_controller.get_state().await;

        if state.grace_turns_remaining > 0 {
            debug!(
                grace_turns_remaining = state.grace_turns_remaining,
                "in grace period, skipping"
            );
            return HookOutcome::Continue;
        }

        if controller_state.should_pause_for_compaction(percent) {
            info!(
                effective_context = percent,
                "ending the run for compaction at the pause threshold"
            );
            cx.notify(format!(
                "the context window is {percent:.0}% full — ending the run so it can compact"
            ));
            let _ = self.loop_controller.stop().await;
            return HookOutcome::ContinueLoop {
                message: "Context limit reached. Ending run for compaction.".to_string(),
            };
        }

        HookOutcome::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_context_aware_termination_not_running() {
        let dir = tempdir().unwrap();
        let controller =
            Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));

        let hook = ContextAwareTerminationHook::new(controller);
        let outcome = hook
            .on(
                HookPoint::AgentEnd,
                &HookEvent::Other(serde_json::json!({})),
                &HookCtx {
                    session_id: "test".to_string(),
                    event_tx: tokio::sync::mpsc::channel(1).0,
                    hook_registry: crate::hook::HookRegistry::new(),
                },
            )
            .await;

        // Loop not running -> Continue.
        assert!(matches!(outcome, HookOutcome::Continue));
    }

    #[tokio::test]
    async fn test_context_aware_termination_grace_period() {
        let dir = tempdir().unwrap();
        let controller =
            Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));

        controller.start().await.unwrap();

        let hook = ContextAwareTerminationHook::new(controller);
        hook.set_grace_period(3).await;

        // Even a pause-level percent is suppressed while in the grace window.
        let ev = HookEvent::Other(serde_json::json!({ "effective_context_percent": 95.0 }));
        let outcome = hook
            .on(
                HookPoint::AgentEnd,
                &ev,
                &HookCtx {
                    session_id: "test".to_string(),
                    event_tx: tokio::sync::mpsc::channel(1).0,
                    hook_registry: crate::hook::HookRegistry::new(),
                },
            )
            .await;
        assert!(matches!(outcome, HookOutcome::Continue));
        assert!(hook.loop_controller.is_running().await, "grace must not end the run");
    }

    /// Past `compact_percent` and short of the pause, the run goes on with no note of its own.
    ///
    /// The break this catches: the hook answered the threshold with a "wrap up" steer every turn,
    /// which compacted nothing and asked the agent to stop working.
    #[tokio::test]
    async fn past_the_compaction_mark_the_hook_sends_no_nudge() {
        let dir = tempdir().unwrap();
        let controller =
            Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));

        controller.start().await.unwrap();

        let hook = ContextAwareTerminationHook::new(controller);
        // 80%: past the default compact_percent (75), short of pause_percent (90)
        let ev = HookEvent::Other(serde_json::json!({ "effective_context_percent": 80.0 }));
        let outcome = hook
            .on(
                HookPoint::AgentEnd,
                &ev,
                &HookCtx {
                    session_id: "test".to_string(),
                    event_tx: tokio::sync::mpsc::channel(1).0,
                    hook_registry: crate::hook::HookRegistry::new(),
                },
            )
            .await;

        assert!(matches!(outcome, HookOutcome::Continue), "{outcome:?}");
        assert!(hook.loop_controller.is_running().await);
    }

    #[tokio::test]
    async fn test_context_aware_termination_pause() {
        let dir = tempdir().unwrap();
        let controller =
            Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));

        controller.start().await.unwrap();

        let hook = ContextAwareTerminationHook::new(controller);
        // 95%: above default pause_percent (90) -> end the run.
        let ev = HookEvent::Other(serde_json::json!({ "effective_context_percent": 95.0 }));
        let outcome = hook
            .on(
                HookPoint::AgentEnd,
                &ev,
                &HookCtx {
                    session_id: "test".to_string(),
                    event_tx: tokio::sync::mpsc::channel(1).0,
                    hook_registry: crate::hook::HookRegistry::new(),
                },
            )
            .await;

        match outcome {
            HookOutcome::ContinueLoop { message } => {
                assert!(message.contains("Ending run"));
            }
            _ => panic!("expected ContinueLoop outcome"),
        }

        // Loop should be stopped.
        assert!(!hook.loop_controller.is_running().await);
    }
}
