use super::*;
use tempfile::tempdir;

#[tokio::test]
async fn test_completion_challenger_hook_not_running() {
    let dir = tempdir().unwrap();
    let controller =
        Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));

    let hook = CompletionChallengerHook::new(controller).await;
    let outcome = hook
        .on(
            HookPoint::LoopControl,
            &HookEvent::LoopControl(crate::hook::LoopControl::Complete),
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

/// A completion claim the reviewer cannot approve pauses the run instead of ending it.
///
/// The break this catches: the gate's undecided outcome falling through to the confirmation
/// count, so an outage could let a run declare itself complete with nobody having reviewed it.
#[tokio::test]
async fn an_undecided_completion_pauses_rather_than_ends() {
    use crate::phase_review::{PhaseReviewOutcome, PhaseReviewRequest, PhaseReviewer};

    #[derive(Debug)]
    struct Down;
    #[async_trait]
    impl PhaseReviewer for Down {
        async fn review(&self, _request: PhaseReviewRequest) -> PhaseReviewOutcome {
            PhaseReviewOutcome::Unavailable("provider down".into())
        }
    }
    #[derive(Debug)]
    struct Change;
    #[async_trait]
    impl crate::phase_gate::PhaseReviewContext for Change {
        async fn diff(&self) -> Result<String, String> {
            Ok("+ change".into())
        }
        async fn planning(&self, _phase: u32) -> String {
            String::new()
        }
        async fn files(
            &self,
            _diff: &str,
            _planning: &str,
        ) -> Vec<crate::phase_review::ReviewedFile> {
            Vec::new()
        }
    }

    let dir = tempdir().unwrap();
    let controller =
        Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));
    controller.start().await.unwrap();
    let gate = crate::phase_gate::PhaseGate::new().with_reviewer(Arc::new(Down), Arc::new(Change));
    let hook = CompletionChallengerHook::new(controller.clone()).await.with_gate(Arc::new(gate));
    hook.set_confirmations_required(1).await;

    let outcome = hook
        .on(
            HookPoint::LoopControl,
            &HookEvent::LoopControl(crate::hook::LoopControl::Complete),
            &HookCtx {
                session_id: "test".to_string(),
                event_tx: tokio::sync::mpsc::channel(1).0,
                hook_registry: crate::hook::HookRegistry::new(),
            },
        )
        .await;

    assert!(matches!(outcome, HookOutcome::StopLoop { .. }), "got {outcome:?}");
    assert!(!controller.is_running().await, "paused");
    assert!(!dir.path().join("STOP").exists(), "paused, not stopped: the run resumes");
}

#[tokio::test]
async fn test_completion_challenger_hook_verification() {
    let dir = tempdir().unwrap();
    let controller =
        Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));

    controller.start().await.unwrap();

    let hook = CompletionChallengerHook::new(controller).await;
    hook.set_confirmations_required(2).await;

    // First call should request verification (counter increments to 1).
    let outcome = hook
        .on(
            HookPoint::LoopControl,
            &HookEvent::LoopControl(crate::hook::LoopControl::Complete),
            &HookCtx {
                session_id: "test".to_string(),
                event_tx: tokio::sync::mpsc::channel(1).0,
                hook_registry: crate::hook::HookRegistry::new(),
            },
        )
        .await;

    match outcome {
        HookOutcome::ContinueLoop { message } => {
            assert!(message.contains("VERIFICATION REQUIRED"), "message was: {}", message);
        }
        _ => panic!("expected ContinueLoop outcome, got {:?}", outcome),
    }

    // Second call should stop the loop (counter increments to 2, >= 2).
    let outcome = hook
        .on(
            HookPoint::LoopControl,
            &HookEvent::LoopControl(crate::hook::LoopControl::Complete),
            &HookCtx {
                session_id: "test".to_string(),
                event_tx: tokio::sync::mpsc::channel(1).0,
                hook_registry: crate::hook::HookRegistry::new(),
            },
        )
        .await;

    match outcome {
        HookOutcome::ContinueLoop { message } => {
            assert!(message.contains("Ending loop"), "message was: {}", message);
        }
        _ => panic!("expected ContinueLoop outcome, got {:?}", outcome),
    }

    // Loop should be stopped.
    assert!(!hook.loop_controller.is_running().await);
}

#[tokio::test]
async fn test_completion_challenger_hook_reset() {
    let dir = tempdir().unwrap();
    let controller =
        Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));

    controller.start().await.unwrap();

    let hook = CompletionChallengerHook::new(controller).await;
    hook.set_confirmations_required(2).await;

    // Increment confirmation.
    hook.increment_confirmation().await;

    // Reset confirmations.
    hook.reset_confirmations().await;

    // Verify by checking state.
    let state = hook.state.lock().await;
    assert_eq!(state.confirmations_received, 0);
}

#[tokio::test]
async fn test_completion_challenger_reads_loop_state_config() {
    // Regression test: verifies the challenger reads confirmations_required from
    // LoopState at construction time (not just the hook's own setter). This pins
    // the threading introduced in 6b2c179 so it can't silently regress.
    let dir = tempdir().unwrap();
    let controller =
        Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));

    // Set LoopState.confirmations_required to 3 (distinctive value).
    controller
        .update_state(|s| {
            s.confirmations_required = 3;
        })
        .await;

    controller.start().await.unwrap();

    let hook = CompletionChallengerHook::new(controller).await;

    // First Complete -> verify (counter 0->1, 1<3).
    let outcome = hook
        .on(
            HookPoint::LoopControl,
            &HookEvent::LoopControl(crate::hook::LoopControl::Complete),
            &HookCtx {
                session_id: "test".to_string(),
                event_tx: tokio::sync::mpsc::channel(1).0,
                hook_registry: crate::hook::HookRegistry::new(),
            },
        )
        .await;

    match outcome {
        HookOutcome::ContinueLoop { message } => {
            assert!(message.contains("VERIFICATION REQUIRED"), "message was: {}", message);
        }
        _ => panic!("expected ContinueLoop outcome, got {:?}", outcome),
    }

    // Second Complete -> verify (counter 1->2, 2<3).
    let outcome = hook
        .on(
            HookPoint::LoopControl,
            &HookEvent::LoopControl(crate::hook::LoopControl::Complete),
            &HookCtx {
                session_id: "test".to_string(),
                event_tx: tokio::sync::mpsc::channel(1).0,
                hook_registry: crate::hook::HookRegistry::new(),
            },
        )
        .await;

    match outcome {
        HookOutcome::ContinueLoop { message } => {
            assert!(message.contains("VERIFICATION REQUIRED"), "message was: {}", message);
        }
        _ => panic!("expected ContinueLoop outcome, got {:?}", outcome),
    }

    // Third Complete -> stop (counter 2->3, 3>=3).
    let outcome = hook
        .on(
            HookPoint::LoopControl,
            &HookEvent::LoopControl(crate::hook::LoopControl::Complete),
            &HookCtx {
                session_id: "test".to_string(),
                event_tx: tokio::sync::mpsc::channel(1).0,
                hook_registry: crate::hook::HookRegistry::new(),
            },
        )
        .await;

    match outcome {
        HookOutcome::ContinueLoop { message } => {
            assert!(message.contains("Ending loop"), "message was: {}", message);
        }
        _ => panic!("expected ContinueLoop outcome, got {:?}", outcome),
    }

    // Loop should be stopped.
    assert!(!hook.loop_controller.is_running().await);
}
