//! The review gate hook, end to end.
//!
//! What remains of this file's original subject: the phase/workflow types it also covered were a
//! superseded duplicate of the loop's own phase tracking and have been deleted.

mod common;

use luxe_core::hook::{HookCtx, HookEvent, HookOutcome, HookPoint, HookRegistry};
use std::sync::Arc;
use tokio::sync::mpsc;

#[tokio::test]
async fn test_review_gate_hook_fires() {
    use luxe_core::hooks::review_gate::ReviewGateHook;
    common::outside_any_review_gate();

    // NOTE: never register a default ReviewGateHook here — its default acceptance
    // commands are `cargo test --workspace` / `cargo clippy --workspace`, which would
    // recursively spawn this very test suite (a fork bomb that locks the machine).
    // Use a harmless command that always succeeds instead.
    let mut registry = HookRegistry::new();
    registry.register(Arc::new(ReviewGateHook::with_commands(vec![common::always_succeeds()])));

    let (tx, _rx) = mpsc::channel(1);
    let hook_cx = HookCtx {
        session_id: "test-session".to_string(),
        event_tx: tx,
        hook_registry: HookRegistry::new(),
    };

    let outcome = registry
        .invoke(HookPoint::ReviewGate, &HookEvent::Other(serde_json::Value::Null), &hook_cx)
        .await;

    match outcome {
        HookOutcome::InjectContext(ctx) => {
            // A passing acceptance command yields the CONTINUE verdict.
            assert!(
                ctx.contains("CONTINUE"),
                "Review gate should produce a CONTINUE verdict, got: {}",
                ctx
            );
        }
        _ => panic!("Expected InjectContext outcome from review gate hook"),
    }
}
