use super::*;
use tempfile::tempdir;

#[tokio::test]
async fn test_runaway_guards_hook_not_running() {
    let dir = tempdir().unwrap();
    let controller =
        Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));

    let hook = RunawayGuardsHook::new(controller);
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
async fn test_runaway_guards_hook_max_turns() {
    let dir = tempdir().unwrap();
    let controller =
        Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));

    controller.start().await.unwrap();

    // Set max turns to 2 (iteration starts at 1, so first turn passes, second triggers).
    controller
        .update_state(|state| {
            state.max_turns_per_run = 2;
        })
        .await;

    let hook = RunawayGuardsHook::new(controller.clone());

    // First turn should pass.
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

    assert!(matches!(outcome, HookOutcome::Continue));

    // Simulate a new run (increment iteration).
    controller.resume().await.unwrap();
    controller.start().await.unwrap();

    // Second turn should trigger max turns.
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

    match outcome {
        HookOutcome::StopLoop { reason } => {
            assert!(reason.contains("Max turns reached"));
        }
        _ => panic!("expected StopLoop outcome"),
    }
}

#[tokio::test]
async fn test_runaway_guards_hook_no_progress() {
    let dir = tempdir().unwrap();
    let controller =
        Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));

    controller.start().await.unwrap();

    // Set max_no_progress_turns to 5 (lower than default 12).
    controller
        .update_state(|state| {
            state.max_no_progress_turns = 5;
        })
        .await;

    let hook = RunawayGuardsHook::new(controller);

    // Simulate 5 turns without progress.
    for _ in 0..5 {
        hook.record_stall().await;
    }

    // Should trigger no-progress.
    let msg = hook.check_no_progress().await;
    assert!(msg.is_some());
    assert!(msg.unwrap().contains("No progress detected"));
}

#[tokio::test]
async fn test_runaway_guards_hook_record_progress() {
    let dir = tempdir().unwrap();
    let controller =
        Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));

    controller.start().await.unwrap();

    let hook = RunawayGuardsHook::new(controller);

    // Simulate 5 turns without progress.
    for _ in 0..5 {
        hook.record_stall().await;
    }

    // Record progress.
    hook.record_progress("edited file").await;

    // Should not trigger no-progress.
    let msg = hook.check_no_progress().await;
    assert!(msg.is_none());
}

#[tokio::test]
async fn test_runaway_guards_hook_rate_limit_backoff() {
    let dir = tempdir().unwrap();
    let controller =
        Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));

    controller.start().await.unwrap();

    let hook = RunawayGuardsHook::new(controller);

    // 0 errors -> 1s backoff.
    assert_eq!(hook.get_backoff_ms().await, 1000);

    // 1 error -> 2s backoff.
    hook.record_error().await;
    assert_eq!(hook.get_backoff_ms().await, 2000);

    // 5 errors -> 30s backoff (max).
    for _ in 0..4 {
        hook.record_error().await;
    }
    assert_eq!(hook.get_backoff_ms().await, 30000);

    // Reset errors.
    hook.reset_errors().await;
    assert_eq!(hook.get_backoff_ms().await, 1000);
}

#[tokio::test]
async fn test_runaway_guards_hook_after_tool_use_progress() {
    let dir = tempdir().unwrap();
    let controller =
        Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));

    controller.start().await.unwrap();

    // Set max_stalls to 3 (lower than default 5).
    controller
        .update_state(|state| {
            state.max_stalls = 3;
        })
        .await;

    let hook = RunawayGuardsHook::new(controller);

    // Simulate 3 stalls (would trigger max_stalls).
    for _ in 0..3 {
        hook.record_stall().await;
    }

    // Should trigger max_stalls.
    let msg = hook.check_max_stalls().await;
    assert!(msg.is_some());

    // Now simulate a successful tool use.
    let outcome = hook
        .on(
            HookPoint::AfterToolUse,
            &HookEvent::ToolResult {
                tool_name: "edit".to_string(),
                result: "success".to_string(),
                is_error: false,
            },
            &HookCtx {
                session_id: "test".to_string(),
                event_tx: tokio::sync::mpsc::channel(1).0,
                hook_registry: crate::hook::HookRegistry::new(),
            },
        )
        .await;

    assert!(matches!(outcome, HookOutcome::Continue));

    // Should NOT trigger max_stalls anymore (progress recorded).
    let msg = hook.check_max_stalls().await;
    assert!(msg.is_none());
}

#[tokio::test]
async fn a_failing_tool_call_is_not_counted_against_the_error_budget() {
    let dir = tempdir().unwrap();
    let controller =
        Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));
    controller.start().await.unwrap();
    let hook = RunawayGuardsHook::new(controller);

    // Five failing tool calls, which is what `max_consecutive_errors` defaults to.
    for _ in 0..5 {
        let outcome = hook
            .on(
                HookPoint::AfterToolUse,
                &HookEvent::ToolResult {
                    tool_name: "edit".to_string(),
                    result: "oldText not found".to_string(),
                    is_error: true,
                },
                &HookCtx {
                    session_id: "test".to_string(),
                    event_tx: tokio::sync::mpsc::channel(1).0,
                    hook_registry: crate::hook::HookRegistry::new(),
                },
            )
            .await;
        assert!(matches!(outcome, HookOutcome::Continue));
    }

    // The break this catches, measured on a real run: an `edit` whose `oldText` did not match,
    // a `bash` that exited non-zero and a `read` of a missing file are the agent's ordinary
    // feedback loop. Counting them here ended a run that was working, with "Max consecutive
    // errors reached" and five scattered tool failures behind it. A turn failure is what the
    // budget is for, and that arrives at `HookPoint::Error`.
    assert!(hook.check_max_errors().await.is_none(), "ordinary tool failures must not end the run");
    assert_eq!(
        hook.get_backoff_ms().await,
        1000,
        "and must not earn a backoff, which is for a provider that is failing"
    );
}

#[tokio::test]
async fn test_runaway_guards_hook_read_only_tools_dont_reset_stalls() {
    let dir = tempdir().unwrap();
    let controller =
        Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));

    controller.start().await.unwrap();

    // Set max_stalls to 3.
    controller
        .update_state(|state| {
            state.max_stalls = 3;
        })
        .await;

    let hook = RunawayGuardsHook::new(controller);

    // Simulate 3 stalls.
    for _ in 0..3 {
        hook.record_stall().await;
    }

    // Should trigger max_stalls.
    let msg = hook.check_max_stalls().await;
    assert!(msg.is_some());

    // Simulate read-only tools (read, grep, glob).
    for tool in ["read", "grep", "glob"] {
        hook.on(
            HookPoint::AfterToolUse,
            &HookEvent::ToolResult {
                tool_name: tool.to_string(),
                result: "found stuff".to_string(),
                is_error: false,
            },
            &HookCtx {
                session_id: "test".to_string(),
                event_tx: tokio::sync::mpsc::channel(1).0,
                hook_registry: crate::hook::HookRegistry::new(),
            },
        )
        .await;
    }

    // Should STILL trigger max_stalls (read-only tools don't reset stalls).
    let msg = hook.check_max_stalls().await;
    assert!(msg.is_some());
}
