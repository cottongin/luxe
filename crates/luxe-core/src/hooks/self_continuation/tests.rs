use super::*;
use crate::loop_state::LoopState;
use tempfile::tempdir;

#[tokio::test]
async fn test_self_continuation_hook_not_running() {
    let dir = tempdir().unwrap();
    let controller =
        Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));

    let hook = SelfContinuationHook::new(controller);
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
async fn test_self_continuation_hook_running() {
    let dir = tempdir().unwrap();
    let controller =
        Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));

    // Start the loop.
    controller.start().await.unwrap();

    let hook = SelfContinuationHook::new(controller);
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

    // Loop running -> ContinueLoop.
    match outcome {
        HookOutcome::ContinueLoop { message } => {
            assert_eq!(message, default_continue_steer());
        }
        _ => panic!("expected ContinueLoop outcome"),
    }
}

/// A phased plan reaches the developer one phase at a time, and only once per phase.
///
/// The break this catches, measured on a real run: nothing told the developer which phase it was
/// on, so a 27B local model handed a three-phase plan built all three in its first turn and then
/// claimed three phases in three minutes with no work left in any of them. The section is sent
/// on the phase's first iteration only — a steer is permanent history, and re-sending the plan
/// every turn is paid for in every request after it.
#[tokio::test]
async fn the_steer_scopes_the_work_to_the_phase_the_loop_is_on() {
    let dir = tempdir().unwrap();
    tokio::fs::write(
        dir.path().join("PLAN.md"),
        "# Plan\n\n## Phase 0 — Counting\n\nCount the words.\n\n## Phase 1 — Ranking\n\nRank \
             them.\n",
    )
    .await
    .unwrap();
    let controller =
        Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));
    controller.start().await.unwrap();
    let hook = SelfContinuationHook::new(controller.clone()).in_project(dir.path());

    let opening = hook.steer_message().await;
    assert!(opening.contains("You are on Phase 0 — Counting"), "{opening}");
    assert!(!opening.contains("phase 0 — Phase 0"), "the heading names itself once: {opening}");
    assert!(opening.contains("Count the words."), "the phase's own words: {opening}");
    assert!(!opening.contains("Rank them."), "and not the next phase's: {opening}");

    let second = hook.steer_message().await;
    assert!(second.contains("You are on Phase 0"), "still says which phase: {second}");
    assert!(!second.contains("Count the words."), "sent once per phase, not per turn: {second}");

    controller.advance_phase().await.unwrap();
    let next = hook.steer_message().await;
    assert!(next.contains("Rank them."), "a new phase is briefed again: {next}");
}

/// A counter past the end of the plan is told so, rather than steered at a phase that is not.
///
/// The break this catches: the gate advances the counter on every accepted claim and nothing
/// bounds it by the plan, so a finished three-phase project kept being told to "work on the
/// current phase" at phase 4 — spending the rest of its turn budget inventing work.
#[tokio::test]
async fn a_phase_the_plan_does_not_have_ends_the_plan_rather_than_inventing_one() {
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("PLAN.md"), "## Phase 0 — Only\n\nDo it.\n").await.unwrap();
    let controller =
        Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));
    controller.start().await.unwrap();
    controller.set_phase(4).await.unwrap();

    let steer = SelfContinuationHook::new(controller).in_project(dir.path()).steer_message().await;

    assert!(steer.contains("no phase 4"), "{steer}");
    assert!(steer.contains("complete"), "a way out: {steer}");
    assert!(steer.contains("blocked"), "and the other one: {steer}");
    assert!(!steer.contains("PROJECT.md"), "a brief the project does not have: {steer}");

    tokio::fs::write(dir.path().join("PROJECT.md"), "# Brief\n").await.unwrap();
    let controller =
        Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));
    controller.start().await.unwrap();
    controller.set_phase(4).await.unwrap();
    let steer = SelfContinuationHook::new(controller).in_project(dir.path()).steer_message().await;
    assert!(steer.contains("Definition of Done in `PROJECT.md`"), "{steer}");
}

/// A plan written without phase headings keeps the generic steer.
#[tokio::test]
async fn a_plan_without_phases_is_steered_the_old_way() {
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("PLAN.md"), "# Plan\n\nJust do the thing.\n").await.unwrap();
    let controller =
        Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));
    controller.start().await.unwrap();

    let steer = SelfContinuationHook::new(controller).in_project(dir.path()).steer_message().await;

    assert_eq!(steer, default_continue_steer());
}

/// The steer names the journal the loop actually writes, and says how to claim a phase.
///
/// Two breaks this catches, both of which shipped. The path used to be
/// `.pi/agent-loop/PROGRESS.md` — another harness's directory, and nothing here writes it — so
/// every iteration told the agent to read a file that did not exist. And nothing told it to call
/// `loop_control` at all, so the phase gate never ran: measured at four turns of real work with
/// no claim and no gate.
#[test]
fn the_steer_points_at_the_journal_and_says_how_to_claim_a_phase() {
    let steer = default_continue_steer();
    assert!(steer.contains(".luxe/loop/PROGRESS.md"), "{steer}");
    assert!(!steer.contains(".pi/"), "the loop must not send anyone to another harness: {steer}");
    assert!(steer.contains("loop_control"), "a phase has to be claimable: {steer}");
    assert!(steer.contains("progress"), "{steer}");
}

#[tokio::test]
async fn test_self_continuation_hook_custom_steer() {
    let dir = tempdir().unwrap();
    let controller =
        Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));

    controller.start().await.unwrap();

    let hook = SelfContinuationHook::new(controller).with_steer("Custom steer message".to_string());

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
        HookOutcome::ContinueLoop { message } => {
            assert_eq!(message, "Custom steer message");
        }
        _ => panic!("expected ContinueLoop outcome"),
    }
}

#[tokio::test]
async fn test_self_continuation_hook_stopped_sentinel() {
    let dir = tempdir().unwrap();
    let controller =
        Arc::new(LoopController::new(dir.path().join("state.json"), dir.path().join("STOP")));

    // Start the loop, then create STOP sentinel.
    controller.start().await.unwrap();
    LoopState::create_stop_sentinel(dir.path().join("STOP").as_path()).unwrap();

    let hook = SelfContinuationHook::new(controller);
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

    // STOP sentinel exists -> Continue.
    assert!(matches!(outcome, HookOutcome::Continue));
}
