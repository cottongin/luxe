//! Completion challenger lifecycle integration test — drives the full verify→confirm→stop flow.
//!
//! Exercises: phase advancement via loop_control progress, completion challenge
//! (verify on first complete, stop on second), and verifies the controller stops.

#![allow(clippy::unwrap_used)]

use luxe_core::hook::LoopControl;
use luxe_core::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint};
use luxe_core::hooks::completion_challenger::CompletionChallengerHook;
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test]
async fn test_completion_challenger_full_lifecycle() {
    let dir = tempdir().unwrap();
    let controller = Arc::new(luxe_core::loop_controller::LoopController::new(
        dir.path().join("state.json"),
        dir.path().join("STOP"),
    ));

    // Start the loop.
    controller.start().await.unwrap();

    // Build the challenger hook.
    let hook = CompletionChallengerHook::new(controller.clone()).await;

    // A progress claim is not this hook's business: the review integrator owns phase advancement,
    // because only it runs the gate that decides whether the claim was true. This hook used to
    // advance the phase here as well, which moved the counter twice for one claim and moved it at
    // all for a claim the gate would have refused.
    for phase in [Some(0), Some(1), None] {
        let outcome = hook
            .on(
                HookPoint::LoopControl,
                &HookEvent::LoopControl(LoopControl::Progress { phase }),
                &HookCtx {
                    session_id: "test".to_string(),
                    event_tx: tokio::sync::mpsc::channel(1).0,
                    hook_registry: luxe_core::hook::HookRegistry::new(),
                },
            )
            .await;

        assert!(matches!(outcome, HookOutcome::Continue));
        assert_eq!(
            controller.get_state().await.current_phase,
            0,
            "the completion challenger must not touch the phase"
        );
    }

    // Complete: first call should trigger verification (not stop).
    let outcome = hook
        .on(
            HookPoint::LoopControl,
            &HookEvent::LoopControl(LoopControl::Complete),
            &HookCtx {
                session_id: "test".to_string(),
                event_tx: tokio::sync::mpsc::channel(1).0,
                hook_registry: luxe_core::hook::HookRegistry::new(),
            },
        )
        .await;

    match outcome {
        HookOutcome::ContinueLoop { message } => {
            assert!(
                message.contains("VERIFICATION REQUIRED"),
                "First complete should trigger verification, got: {}",
                message
            );
        }
        _ => panic!("expected ContinueLoop outcome on first complete, got {:?}", outcome),
    }

    // Loop should still be running.
    assert!(controller.is_running().await, "Loop should still be running after first complete");

    // Complete: second call should stop the loop (enough confirmations).
    let outcome = hook
        .on(
            HookPoint::LoopControl,
            &HookEvent::LoopControl(LoopControl::Complete),
            &HookCtx {
                session_id: "test".to_string(),
                event_tx: tokio::sync::mpsc::channel(1).0,
                hook_registry: luxe_core::hook::HookRegistry::new(),
            },
        )
        .await;

    match outcome {
        HookOutcome::ContinueLoop { message } => {
            assert!(
                message.contains("Ending loop"),
                "Second complete should stop the loop, got: {}",
                message
            );
        }
        _ => panic!("expected ContinueLoop outcome on second complete, got {:?}", outcome),
    }

    // Loop should be stopped.
    assert!(!controller.is_running().await, "Loop should be stopped after second complete");

    println!(
        "Completion challenger lifecycle test passed: phase advance + verify → confirm → stop"
    );
}
