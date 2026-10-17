//! Loop hook integration tests — that the hooks fire, and that firing changes something.

mod common;

use luxe_core::Hook;
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test]
async fn test_journal_bounding_archives_past_cap() {
    let dir = tempdir().unwrap();
    let journal_path = dir.path().join("PROGRESS.md");
    let archive_path = dir.path().join("PROGRESS.archive.md");

    // Create a large PROGRESS.md (simulate >24KB).
    let large_content = "x".repeat(25_000);
    tokio::fs::write(&journal_path, &large_content).await.unwrap();

    // Build and run the journal bounding hook.
    let hook = luxe_core::hooks::journal_bounding::JournalBoundingHook::new(
        journal_path.clone(),
        archive_path.clone(),
        24, // 24 KB cap
    );

    let hook_cx = luxe_core::hook::HookCtx {
        session_id: "test-session".to_string(),
        event_tx: tokio::sync::mpsc::channel(1).0,
        hook_registry: luxe_core::hook::HookRegistry::new(),
    };

    // Invoke the hook at BeforeCompact.
    let _outcome = hook
        .on(
            luxe_core::hook::HookPoint::BeforeCompact,
            &luxe_core::hook::HookEvent::Other(serde_json::Value::Null),
            &hook_cx,
        )
        .await;

    // Should archive and truncate.
    assert!(archive_path.exists(), "archive file should exist");
    let archived = tokio::fs::read_to_string(&archive_path).await.unwrap();
    assert_eq!(archived, large_content, "archive should contain original content");

    let remaining = tokio::fs::read_to_string(&journal_path).await.unwrap();
    assert!(remaining.len() < 24_000, "journal should be truncated to ~24KB");
}

/// An ordinary turn does not advance the phase.
///
/// The break this catches, which shipped: the review integrator fired on `AgentEnd` — every turn —
/// ran a gate whose default command is `true`, and advanced on the pass. Measured before the fix,
/// five plain turns moved the counter 0 → 5, so `luxe loop phase` reported a number that meant
/// "turns taken" and any per-phase behavior was keyed on noise.
#[tokio::test]
async fn an_ordinary_turn_does_not_advance_the_phase() {
    let dir = tempdir().unwrap();
    tokio::fs::create_dir_all(dir.path().join(".luxe")).await.unwrap();
    let controller = Arc::new(luxe_core::loop_controller::LoopController::new(
        dir.path().join("loop_state.json"),
        dir.path().join("STOP"),
    ));
    controller.start().await.unwrap();

    let integrator =
        luxe_core::hooks::review_integrator::ReviewIntegratorHook::new(controller.clone())
            .with_gate(Arc::new(luxe_core::phase_gate::PhaseGate::new().with_commands(Arc::new(
                luxe_core::hooks::review_gate::ReviewGateHook::with_commands(vec![
                    common::always_succeeds(),
                ]),
            ))));

    for _ in 0..5 {
        integrator
            .on(
                luxe_core::hook::HookPoint::AgentEnd,
                &luxe_core::hook::HookEvent::Other(serde_json::json!({})),
                &hook_ctx(),
            )
            .await;
    }

    assert_eq!(
        controller.get_state().await.current_phase,
        0,
        "a turn is not a phase; only a claim of progress that clears the gate is"
    );
    controller.stop().await.unwrap();
}

/// A progress claim that clears the gate advances the phase exactly once.
#[tokio::test]
async fn a_claim_that_clears_the_gate_advances_one_phase() {
    let dir = tempdir().unwrap();
    tokio::fs::create_dir_all(dir.path().join(".luxe")).await.unwrap();
    let controller = Arc::new(luxe_core::loop_controller::LoopController::new(
        dir.path().join("loop_state.json"),
        dir.path().join("STOP"),
    ));
    controller.start().await.unwrap();

    let integrator =
        luxe_core::hooks::review_integrator::ReviewIntegratorHook::new(controller.clone())
            .with_gate(Arc::new(luxe_core::phase_gate::PhaseGate::new().with_commands(Arc::new(
                luxe_core::hooks::review_gate::ReviewGateHook::with_commands(vec![
                    common::always_succeeds(),
                ]),
            ))));

    let outcome = integrator
        .on(
            luxe_core::hook::HookPoint::LoopControl,
            &luxe_core::hook::HookEvent::LoopControl(luxe_core::hook::LoopControl::Progress {
                phase: None,
            }),
            &hook_ctx(),
        )
        .await;

    let luxe_core::hook::HookOutcome::ContinueLoop { message } = outcome else {
        panic!("an accepted claim answers the agent")
    };
    assert!(message.contains("accepted phase 0"), "{message}");
    assert_eq!(controller.get_state().await.current_phase, 1);
    controller.stop().await.unwrap();
}

/// A progress claim the gate refuses leaves the phase where it was, and says why.
///
/// The break this catches: the old `feed_fix_list` logged the reasons and returned `Ok`, so the
/// agent was told nothing and carried on believing the phase had closed. Both halves matter — the
/// counter must not move, *and* the refusal must reach the model.
#[tokio::test]
async fn a_claim_the_gate_refuses_holds_the_phase_and_says_why() {
    common::outside_any_review_gate();
    let dir = tempdir().unwrap();
    tokio::fs::create_dir_all(dir.path().join(".luxe")).await.unwrap();
    let controller = Arc::new(luxe_core::loop_controller::LoopController::new(
        dir.path().join("loop_state.json"),
        dir.path().join("STOP"),
    ));
    controller.start().await.unwrap();

    let integrator =
        luxe_core::hooks::review_integrator::ReviewIntegratorHook::new(controller.clone())
            .with_gate(Arc::new(luxe_core::phase_gate::PhaseGate::new().with_commands(Arc::new(
                luxe_core::hooks::review_gate::ReviewGateHook::with_commands(vec![
                    common::always_fails(),
                ]),
            ))));

    let outcome = integrator
        .on(
            luxe_core::hook::HookPoint::LoopControl,
            &luxe_core::hook::HookEvent::LoopControl(luxe_core::hook::LoopControl::Progress {
                phase: None,
            }),
            &hook_ctx(),
        )
        .await;

    assert_eq!(
        controller.get_state().await.current_phase,
        0,
        "a refused claim must not move the phase"
    );
    match outcome {
        luxe_core::hook::HookOutcome::ContinueLoop { message } => {
            assert!(message.contains("did not pass"), "the refusal must be legible: {message}");
            assert!(
                message.contains("has not moved"),
                "the agent has to learn the phase stayed put: {message}"
            );
        }
        other => panic!("a refused claim must reach the model, got {other:?}"),
    }
    controller.stop().await.unwrap();
}

/// `complete` and `blocked` are other hooks' business; the gate must not run for them.
#[tokio::test]
async fn only_a_progress_claim_runs_the_gate() {
    let dir = tempdir().unwrap();
    tokio::fs::create_dir_all(dir.path().join(".luxe")).await.unwrap();
    let controller = Arc::new(luxe_core::loop_controller::LoopController::new(
        dir.path().join("loop_state.json"),
        dir.path().join("STOP"),
    ));
    controller.start().await.unwrap();

    // A gate that would fail loudly if it ran at all.
    let integrator =
        luxe_core::hooks::review_integrator::ReviewIntegratorHook::new(controller.clone())
            .with_gate(Arc::new(luxe_core::phase_gate::PhaseGate::new().with_commands(Arc::new(
                luxe_core::hooks::review_gate::ReviewGateHook::with_commands(vec![
                    common::always_fails(),
                ]),
            ))));

    for event in [
        luxe_core::hook::HookEvent::LoopControl(luxe_core::hook::LoopControl::Complete),
        luxe_core::hook::HookEvent::LoopControl(luxe_core::hook::LoopControl::Blocked {
            reason: "needs a decision".into(),
        }),
    ] {
        let outcome =
            integrator.on(luxe_core::hook::HookPoint::LoopControl, &event, &hook_ctx()).await;
        assert!(
            matches!(outcome, luxe_core::hook::HookOutcome::Continue),
            "only `progress` closes a phase"
        );
    }
    assert_eq!(controller.get_state().await.current_phase, 0);
    controller.stop().await.unwrap();
}

/// A reviewer that answers however the test needs, and records whether it was consulted.
#[derive(Debug)]
struct ScriptedReviewer {
    outcome: luxe_core::phase_review::PhaseReviewOutcome,
    consulted: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait::async_trait]
impl luxe_core::phase_review::PhaseReviewer for ScriptedReviewer {
    async fn review(
        &self,
        _request: luxe_core::phase_review::PhaseReviewRequest,
    ) -> luxe_core::phase_review::PhaseReviewOutcome {
        self.consulted.store(true, std::sync::atomic::Ordering::SeqCst);
        self.outcome.clone()
    }
}

/// Evidence with a change in it, so the reviewer has something to be asked about.
#[derive(Debug)]
struct StubContext;

#[async_trait::async_trait]
impl luxe_core::phase_gate::PhaseReviewContext for StubContext {
    async fn diff(&self) -> Result<String, String> {
        Ok("--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n".to_string())
    }

    async fn planning(&self, _phase: u32) -> String {
        "## Phase 0\nDo the thing.".to_string()
    }

    async fn files(
        &self,
        _diff: &str,
        _planning: &str,
    ) -> Vec<luxe_core::phase_review::ReviewedFile> {
        Vec::new()
    }
}

/// Build an integrator whose commands behave as `gate_passes` says, with a scripted reviewer.
fn integrator_with_reviewer(
    controller: Arc<luxe_core::loop_controller::LoopController>,
    gate_passes: bool,
    outcome: luxe_core::phase_review::PhaseReviewOutcome,
) -> (luxe_core::hooks::review_integrator::ReviewIntegratorHook, Arc<std::sync::atomic::AtomicBool>)
{
    let consulted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let gate = Arc::new(gate_with_reviewer(gate_passes, outcome, consulted.clone()));
    let hook =
        luxe_core::hooks::review_integrator::ReviewIntegratorHook::new(controller).with_gate(gate);
    (hook, consulted)
}

/// A gate whose commands behave as `gate_passes` says, with a scripted reviewer behind them.
fn gate_with_reviewer(
    gate_passes: bool,
    outcome: luxe_core::phase_review::PhaseReviewOutcome,
    consulted: Arc<std::sync::atomic::AtomicBool>,
) -> luxe_core::phase_gate::PhaseGate {
    let commands = if gate_passes { common::always_succeeds() } else { common::always_fails() };
    luxe_core::phase_gate::PhaseGate::new()
        .with_commands(Arc::new(luxe_core::hooks::review_gate::ReviewGateHook::with_commands(
            vec![commands],
        )))
        .with_reviewer(Arc::new(ScriptedReviewer { outcome, consulted }), Arc::new(StubContext))
}

async fn claim_progress(
    hook: &luxe_core::hooks::review_integrator::ReviewIntegratorHook,
) -> luxe_core::hook::HookOutcome {
    hook.on(
        luxe_core::hook::HookPoint::LoopControl,
        &luxe_core::hook::HookEvent::LoopControl(luxe_core::hook::LoopControl::Progress {
            phase: None,
        }),
        &hook_ctx(),
    )
    .await
}

async fn running(dir: &tempfile::TempDir) -> Arc<luxe_core::loop_controller::LoopController> {
    tokio::fs::create_dir_all(dir.path().join(".luxe")).await.unwrap();
    let controller = Arc::new(luxe_core::loop_controller::LoopController::new(
        dir.path().join("loop_state.json"),
        dir.path().join("STOP"),
    ));
    controller.start().await.unwrap();
    controller
}

/// A red suite is never sent to the reviewer.
///
/// The ordering is the whole reason the gate has two stages: a review that contradicts a failing
/// test suite is worth nothing, and the reviewer is the expensive half. The break this catches is
/// running both unconditionally, which bills a strong model on every failed claim.
#[tokio::test]
async fn a_failing_gate_does_not_spend_the_reviewer() {
    common::outside_any_review_gate();
    let dir = tempdir().unwrap();
    let controller = running(&dir).await;
    let (hook, consulted) = integrator_with_reviewer(
        controller.clone(),
        false,
        luxe_core::phase_review::PhaseReviewOutcome::Continue,
    );

    let outcome = claim_progress(&hook).await;

    assert!(!consulted.load(std::sync::atomic::Ordering::SeqCst), "the reviewer must not be asked");
    assert_eq!(controller.get_state().await.current_phase, 0);
    assert!(matches!(outcome, luxe_core::hook::HookOutcome::ContinueLoop { .. }));
    controller.stop().await.unwrap();
}

/// A green suite plus a reviewer that approves advances the phase.
#[tokio::test]
async fn a_green_gate_and_an_approving_reviewer_advance_the_phase() {
    let dir = tempdir().unwrap();
    let controller = running(&dir).await;
    let (hook, consulted) = integrator_with_reviewer(
        controller.clone(),
        true,
        luxe_core::phase_review::PhaseReviewOutcome::Continue,
    );

    let outcome = claim_progress(&hook).await;

    assert!(consulted.load(std::sync::atomic::Ordering::SeqCst), "the reviewer must be asked");
    assert_eq!(controller.get_state().await.current_phase, 1);
    assert!(
        matches!(outcome, luxe_core::hook::HookOutcome::ContinueLoop { .. }),
        "and the claim is answered rather than shrugged at"
    );
    controller.stop().await.unwrap();
}

/// A reviewer that finds work holds the phase and hands the findings back.
///
/// This is the case the whole slice exists for: the commands pass, so nothing mechanical objects,
/// and the phase still does not close because the change does not do what was asked.
#[tokio::test]
async fn a_reviewer_that_finds_work_holds_the_phase() {
    let dir = tempdir().unwrap();
    let controller = running(&dir).await;
    let (hook, _) = integrator_with_reviewer(
        controller.clone(),
        true,
        luxe_core::phase_review::PhaseReviewOutcome::Fix(vec![
            "src/a.rs:12 the retry is gated on an error message".into(),
        ]),
    );

    let outcome = claim_progress(&hook).await;

    assert_eq!(controller.get_state().await.current_phase, 0, "a held phase must not advance");
    match outcome {
        luxe_core::hook::HookOutcome::ContinueLoop { message } => {
            assert!(message.contains("gate passed"), "say the commands were fine: {message}");
            assert!(message.contains("reviewer found work"), "and who objected: {message}");
            assert!(message.contains("src/a.rs:12"), "and what to do: {message}");
        }
        other => panic!("the findings must reach the model, got {other:?}"),
    }
    controller.stop().await.unwrap();
}

/// A reviewer that cannot answer pauses the loop: never an approval, and never a refusal the agent
/// would retry forever.
///
/// The break this catches: treating `Unavailable` as approval, so an outage advanced every phase
/// unreviewed, or as a refusal, which wedges the loop on something the agent cannot fix.
#[tokio::test]
async fn an_unusable_review_pauses_the_loop_for_a_person() {
    let dir = tempdir().unwrap();
    let controller = running(&dir).await;
    let (hook, _) = integrator_with_reviewer(
        controller.clone(),
        true,
        luxe_core::phase_review::PhaseReviewOutcome::Unavailable("provider down".into()),
    );

    let outcome = claim_progress(&hook).await;

    assert_eq!(controller.get_state().await.current_phase, 0, "the phase does not advance");
    match outcome {
        luxe_core::hook::HookOutcome::StopLoop { reason } => {
            assert!(reason.contains("provider down"), "says why: {reason}");
        }
        other => panic!("an undecided gate stops the loop, got {other:?}"),
    }
    assert!(!controller.is_running().await, "and leaves it paused, resumable with its phase");
}

/// A failed turn counts against the loop's error budget, and paces the next iteration.
///
/// Two breaks this catches, both of which shipped. `HookPoint::Error` was declared and never fired,
/// so `max_consecutive_errors` only ever saw *tool* errors — a provider outage was invisible to the
/// loop's error budget. And `get_backoff_ms` had no caller at all, so a rate-limited provider was
/// retried at full speed until an unrelated guard cut the run off.
#[tokio::test]
async fn a_failed_turn_is_counted_and_backed_off() {
    use luxe_core::hooks::runaway_guards::IterationPacing;

    let dir = tempdir().unwrap();
    let controller = running(&dir).await;
    let guards =
        Arc::new(luxe_core::hooks::runaway_guards::RunawayGuardsHook::new(controller.clone()));

    assert_eq!(
        guards.pause_before_next_iteration().await,
        std::time::Duration::ZERO,
        "a healthy loop waits for nothing"
    );

    guards
        .on(
            luxe_core::hook::HookPoint::Error,
            &luxe_core::hook::HookEvent::Other(serde_json::json!({"message": "network error"})),
            &hook_ctx(),
        )
        .await;

    let paused = guards.pause_before_next_iteration().await;
    assert!(!paused.is_zero(), "a failed turn earns a pause");
    assert!(paused <= std::time::Duration::from_secs(30), "and a bounded one: {paused:?}");
    controller.stop().await.unwrap();
}

/// A turn that ends without failing clears the error run.
///
/// The break this catches: counting errors that were never consecutive. Five failures spread across
/// a long run would then stop the loop as though they had happened back to back — and since nothing
/// reset the counter, that was the behavior the moment turn errors started being counted at all.
#[tokio::test]
async fn a_successful_turn_clears_the_error_run() {
    use luxe_core::hooks::runaway_guards::IterationPacing;

    let dir = tempdir().unwrap();
    let controller = running(&dir).await;
    let guards =
        Arc::new(luxe_core::hooks::runaway_guards::RunawayGuardsHook::new(controller.clone()));

    // A failed turn: Error then AgentEnd, in the order the engine fires them.
    guards
        .on(
            luxe_core::hook::HookPoint::Error,
            &luxe_core::hook::HookEvent::Other(serde_json::json!({"message": "boom"})),
            &hook_ctx(),
        )
        .await;
    guards
        .on(
            luxe_core::hook::HookPoint::AgentEnd,
            &luxe_core::hook::HookEvent::Other(serde_json::json!({})),
            &hook_ctx(),
        )
        .await;
    assert!(
        !guards.pause_before_next_iteration().await.is_zero(),
        "the failure is still counted after its own turn ends"
    );

    // A turn that ends with no Error before it.
    guards
        .on(
            luxe_core::hook::HookPoint::AgentEnd,
            &luxe_core::hook::HookEvent::Other(serde_json::json!({})),
            &hook_ctx(),
        )
        .await;
    assert_eq!(
        guards.pause_before_next_iteration().await,
        std::time::Duration::ZERO,
        "a clean turn means the provider is fine again"
    );
    controller.stop().await.unwrap();
}

/// `complete` has to clear the same gate a phase does.
///
/// The break this catches, observed on a live run: the agent called `complete` on its first
/// iteration instead of claiming the phase, the challenger asked for verification, accepted the
/// second confirmation, and stopped the loop — with the reviewer never consulted and the phase
/// counter still at zero. Two doors out of the loop, a gate on only one of them.
#[tokio::test]
async fn a_completion_claim_the_gate_refuses_does_not_end_the_loop() {
    let dir = tempdir().unwrap();
    let controller = running(&dir).await;
    let consulted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hook =
        luxe_core::hooks::completion_challenger::CompletionChallengerHook::new(controller.clone())
            .await
            .with_gate(Arc::new(gate_with_reviewer(
                true,
                luxe_core::phase_review::PhaseReviewOutcome::Fix(vec![
                    "src/a.rs:1 the phase is not actually done".into(),
                ]),
                consulted.clone(),
            )));

    let outcome = hook
        .on(
            luxe_core::hook::HookPoint::LoopControl,
            &luxe_core::hook::HookEvent::LoopControl(luxe_core::hook::LoopControl::Complete),
            &hook_ctx(),
        )
        .await;

    assert!(consulted.load(std::sync::atomic::Ordering::SeqCst), "the reviewer must be asked");
    assert!(controller.is_running().await, "a refused completion leaves the loop running");
    match outcome {
        luxe_core::hook::HookOutcome::ContinueLoop { message } => {
            assert!(message.contains("src/a.rs:1"), "the findings must reach the agent: {message}");
            assert!(message.contains("still running"), "{message}");
            assert!(
                !message.contains("VERIFICATION"),
                "a refused claim is not a verification request: {message}"
            );
        }
        other => panic!("a refused completion must come back as work, got {other:?}"),
    }
    controller.stop().await.unwrap();
}

/// A refused completion does not spend a confirmation.
///
/// Confirmations exist so a model cannot end the loop on a whim. If a refused claim counted, two
/// refusals would still stop it — the challenge would be satisfied by being told no twice.
#[tokio::test]
async fn a_refused_completion_does_not_count_as_a_confirmation() {
    let dir = tempdir().unwrap();
    let controller = running(&dir).await;
    let refusing = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hook =
        luxe_core::hooks::completion_challenger::CompletionChallengerHook::new(controller.clone())
            .await
            .with_gate(Arc::new(gate_with_reviewer(
                true,
                luxe_core::phase_review::PhaseReviewOutcome::Fix(vec!["not done".into()]),
                refusing,
            )));

    for _ in 0..3 {
        hook.on(
            luxe_core::hook::HookPoint::LoopControl,
            &luxe_core::hook::HookEvent::LoopControl(luxe_core::hook::LoopControl::Complete),
            &hook_ctx(),
        )
        .await;
    }

    assert!(controller.is_running().await, "three refused claims must not add up to a completion");
    controller.stop().await.unwrap();
}

/// A completion the gate accepts still has to be confirmed.
#[tokio::test]
async fn an_accepted_completion_still_needs_its_confirmation() {
    let dir = tempdir().unwrap();
    let controller = running(&dir).await;
    let consulted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hook =
        luxe_core::hooks::completion_challenger::CompletionChallengerHook::new(controller.clone())
            .await
            .with_gate(Arc::new(gate_with_reviewer(
                true,
                luxe_core::phase_review::PhaseReviewOutcome::Continue,
                consulted,
            )));
    hook.set_confirmations_required(2).await;

    let first = hook
        .on(
            luxe_core::hook::HookPoint::LoopControl,
            &luxe_core::hook::HookEvent::LoopControl(luxe_core::hook::LoopControl::Complete),
            &hook_ctx(),
        )
        .await;
    match first {
        luxe_core::hook::HookOutcome::ContinueLoop { message } => assert!(
            message.contains("VERIFICATION"),
            "a gate-passing claim still gets challenged: {message}"
        ),
        other => panic!("expected a verification request, got {other:?}"),
    }
    assert!(controller.is_running().await, "one confirmation is not two");

    hook.on(
        luxe_core::hook::HookPoint::LoopControl,
        &luxe_core::hook::HookEvent::LoopControl(luxe_core::hook::LoopControl::Complete),
        &hook_ctx(),
    )
    .await;
    assert!(!controller.is_running().await, "the second confirmation ends the run");
}

/// A hook context with no listeners, for driving a hook directly.
fn hook_ctx() -> luxe_core::hook::HookCtx {
    luxe_core::hook::HookCtx {
        session_id: "test-session".to_string(),
        event_tx: tokio::sync::mpsc::channel(1).0,
        hook_registry: luxe_core::hook::HookRegistry::new(),
    }
}

#[tokio::test]
async fn test_completion_challenger_requires_confirmation() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join("loop_state.json");
    let stop_path = dir.path().join("STOP");
    tokio::fs::create_dir_all(dir.path().join(".luxe")).await.unwrap();

    let controller =
        Arc::new(luxe_core::loop_controller::LoopController::new(state_path, stop_path));
    controller.start().await.unwrap();

    let hook =
        luxe_core::hooks::completion_challenger::CompletionChallengerHook::new(controller.clone())
            .await;

    let hook_cx = luxe_core::hook::HookCtx {
        session_id: "test-session".to_string(),
        event_tx: tokio::sync::mpsc::channel(1).0,
        hook_registry: luxe_core::hook::HookRegistry::new(),
    };

    // Set confirmations required on the hook (not the controller).
    hook.set_confirmations_required(2).await;

    let outcome = hook
        .on(
            luxe_core::hook::HookPoint::LoopControl,
            &luxe_core::hook::HookEvent::LoopControl(luxe_core::hook::LoopControl::Complete),
            &hook_cx,
        )
        .await;

    // Should return ContinueLoop with verification message (not StopLoop).
    match outcome {
        luxe_core::hook::HookOutcome::ContinueLoop { message } => {
            assert!(
                message.contains("VERIFICATION") || message.contains("confirm"),
                "message should mention verification, got: {}",
                message
            );
        }
        _ => panic!("expected ContinueLoop, got {:?}", outcome),
    }

    controller.stop().await.unwrap();
}

#[tokio::test]
async fn test_self_continuation_enqueues_next_iteration() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join("loop_state.json");
    let stop_path = dir.path().join("STOP");
    tokio::fs::create_dir_all(dir.path().join(".luxe")).await.unwrap();

    let controller =
        Arc::new(luxe_core::loop_controller::LoopController::new(state_path, stop_path));
    controller.start().await.unwrap();

    let hook = luxe_core::hooks::self_continuation::SelfContinuationHook::new(controller.clone());

    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(10);
    let hook_cx = luxe_core::hook::HookCtx {
        session_id: "test-session".to_string(),
        event_tx: event_tx.clone(),
        hook_registry: luxe_core::hook::HookRegistry::new(),
    };

    // Simulate agent_end (loop is running).
    let outcome = hook
        .on(
            luxe_core::hook::HookPoint::AgentEnd,
            &luxe_core::hook::HookEvent::Other(serde_json::json!({})),
            &hook_cx,
        )
        .await;

    // Should return ContinueLoop with the default steer message.
    match outcome {
        luxe_core::hook::HookOutcome::ContinueLoop { message } => {
            assert!(
                message.contains("Continue building") || message.contains("PROGRESS"),
                "should have steer message"
            );
        }
        _ => panic!("expected ContinueLoop, got {:?}", outcome),
    }

    controller.stop().await.unwrap();
}

/// Past `compact_percent`, a loop's steer is the steer alone: compacting is the engine's job.
///
/// The break this catches: the termination hook joined "wrap up and summarize" to every steer past
/// the mark and never compacted anything, so a long run carried that note on every turn while the
/// transcript kept growing (one overnight session reached 780k tokens).
#[tokio::test]
async fn past_the_compaction_mark_the_steer_carries_no_nudge() {
    let dir = tempdir().unwrap();
    tokio::fs::write(
        dir.path().join("PLAN.md"),
        "## Phase 0 — Counting\n\nCount the words.\n\n## Phase 1 — Ranking\n\nRank them.\n",
    )
    .await
    .unwrap();
    let controller = Arc::new(luxe_core::loop_controller::LoopController::new(
        dir.path().join("loop_state.json"),
        dir.path().join("STOP"),
    ));
    controller.start().await.unwrap();

    let mut registry = luxe_core::hook::HookRegistry::new();
    registry.register(Arc::new(
        luxe_core::hooks::self_continuation::SelfContinuationHook::new(controller.clone())
            .in_project(dir.path()),
    ));
    registry.register(Arc::new(
        luxe_core::hooks::context_aware_termination::ContextAwareTerminationHook::new(
            controller.clone(),
        ),
    ));

    // Past the default 75% compaction mark, and well under the 90% that ends a run
    let outcome = registry
        .invoke(
            luxe_core::hook::HookPoint::AgentEnd,
            &luxe_core::hook::HookEvent::Other(
                serde_json::json!({ "effective_context_percent": 80.0 }),
            ),
            &hook_ctx(),
        )
        .await;

    let luxe_core::hook::HookOutcome::ContinueLoop { message } = outcome else {
        panic!("a running loop steers itself")
    };
    assert!(message.contains("Phase 0 — Counting"), "the steer is the phase's: {message}");
    assert!(!message.contains("wrap up"), "and nothing asks the agent to stop: {message}");
    assert!(controller.is_running().await);
}

/// Every hook the loop registers can say what it is.
///
/// The line `luxe loop start` prints about what it registered is this list, so a hook added without
/// a name would show up as an empty column rather than as nothing at all.
#[tokio::test]
async fn the_loop_s_hooks_all_have_names() {
    let dir = tempdir().unwrap();
    let controller = Arc::new(luxe_core::loop_controller::LoopController::new(
        dir.path().join("loop_state.json"),
        dir.path().join("STOP"),
    ));

    let registry =
        luxe_core::loop_hooks::build_default_hooks(controller, dir.path().to_path_buf()).await;
    let names = registry.hook_names();

    assert!(names.contains(&"self-continuation"), "{names:?}");
    assert!(names.contains(&"review-integrator"), "{names:?}");
    assert!(names.contains(&"journal-bounding"), "{names:?}");
    for name in &names {
        assert!(!name.is_empty(), "a hook with no name is a hook nothing can attribute: {names:?}");
        assert_eq!(*name, name.trim(), "names land in a comma-joined line: {names:?}");
    }
}
