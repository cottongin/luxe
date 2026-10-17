use crate::usage::UsageTotals;

fn call(prompt: u64, completion: u64, cost: f64) -> UsageTotals {
    UsageTotals {
        calls: 1,
        prompt_tokens: prompt,
        completion_tokens: completion,
        cost_usd: cost,
        ..UsageTotals::default()
    }
}

/// A run's spend covers every model it used, not just the one driving it.
///
/// The break this catches: the phase reviewer runs in an engine and a session of its own, so its
/// spend landed in a ledger nobody read. The reviewer is the strong, expensive model, which made
/// the missing number the one worth having.
#[test]
fn a_run_adds_up_every_model_it_used() {
    let mut spend = LoopSpend::default();
    spend.record("qwen3.6-27b", &call(1_000, 200, 0.0));
    spend.record("claude-sonnet-5", &call(8_000, 900, 0.42));
    spend.record("qwen3.6-27b", &call(1_500, 300, 0.0));

    assert_eq!(spend.total.calls, 3);
    assert_eq!(spend.total.prompt_tokens, 10_500);
    assert!((spend.total.cost_usd - 0.42).abs() < f64::EPSILON, "{}", spend.total.cost_usd);

    let developer = &spend.by_model["qwen3.6-27b"];
    assert_eq!((developer.calls, developer.prompt_tokens), (2, 2_500));
    let reviewer = &spend.by_model["claude-sonnet-5"];
    assert_eq!((reviewer.calls, reviewer.prompt_tokens), (1, 8_000));
}

/// The spend survives a resume, because a run does.
///
/// The break this catches: keeping the total in memory would reset it at every pause, and a loop
/// pauses for compaction as a matter of course.
#[test]
fn the_spend_is_persisted_with_the_rest_of_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loop_state.json");

    let mut state = LoopState::default();
    state.spend.record("claude-sonnet-5", &call(8_000, 900, 0.42));
    state.save(&path).unwrap();

    let reloaded = LoopState::load(&path).unwrap();

    assert_eq!(reloaded.spend, state.spend, "a resumed run keeps what it already spent");
}

/// A state file written before spend existed still loads.
///
/// The break this catches: `.luxe/loop_state.json` outlives the version that wrote it, and a
/// missing field that fails the parse would strand a resumable run.
#[test]
fn a_state_file_without_spend_still_loads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loop_state.json");
    std::fs::write(
        &path,
        r#"{"running":true,"iteration":7,"current_phase":2,"max_turns_per_run":100,
                "compact_percent":75,"pause_percent":90,"journal_max_kb":24,
                "confirmations_required":2,"context_grace_turns":2,"max_no_progress_turns":12,
                "max_stalls":5,"rate_limit_backoff_ms":30000,"compact_timeout_ms":600000,
                "max_consecutive_errors":5}"#,
    )
    .unwrap();

    let state = LoopState::load(&path).expect("an older state file still loads");

    assert_eq!(state.iteration, 7);
    assert_eq!(state.spend, LoopSpend::default(), "no spend recorded yet");
}

/// A subscription turn is never reported as money the user spent.
///
/// The break this catches: summing a plan's equivalent rate into `cost_usd` would invent a bill.
#[test]
fn a_plan_turn_is_reported_as_plan_rather_than_spend() {
    let mut spend = LoopSpend::default();
    spend.record(
        "claude-sonnet-5",
        &UsageTotals {
            calls: 1,
            prompt_tokens: 8_000,
            completion_tokens: 900,
            plan_usd: 0.42,
            plan_calls: 1,
            ..UsageTotals::default()
        },
    );

    let line = spend.summary_lines().join("\n");

    assert!(line.contains("on plan"), "{line}");
    assert_eq!(spend.total.cost_usd, 0.0, "a plan turn costs no money");
}

/// With one model there is no "all models" line to add up.
#[test]
fn a_single_model_run_does_not_total_itself() {
    let mut spend = LoopSpend::default();
    spend.record("qwen3.6-27b", &call(1_000, 200, 0.0));

    let lines = spend.summary_lines();

    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].starts_with("qwen3.6-27b: 1 calls, 1200 tokens"), "{lines:?}");
}

/// A run that has spent nothing prints no section at all.
#[test]
fn nothing_spent_is_nothing_shown() {
    assert!(LoopSpend::default().summary_lines().is_empty());
}

use super::*;
use tempfile::tempdir;

#[test]
fn test_default_state() {
    let state = LoopState::default();
    assert!(!state.running);
    assert_eq!(state.iteration, 0);
    assert_eq!(state.current_phase, 0);
    assert_eq!(state.max_turns_per_run, 100);
    assert_eq!(state.compact_percent, 75);
    assert_eq!(state.pause_percent, 90);
}

#[test]
fn test_save_and_load() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join("state.json");

    let mut state = LoopState::default();
    state.running = true;
    state.iteration = 42;
    state.current_phase = 3;
    state.save(&state_path).unwrap();

    let loaded = LoopState::load(&state_path).unwrap();
    assert!(loaded.running);
    assert_eq!(loaded.iteration, 42);
    assert_eq!(loaded.current_phase, 3);
}

#[test]
fn test_load_nonexistent_returns_default() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join("nonexistent.json");
    let state = LoopState::load(&state_path).unwrap();
    assert!(!state.running);
}

#[test]
fn test_should_pause_for_compaction() {
    let mut state = LoopState::default();
    state.pause_percent = 80;

    // Below threshold (62.5%)
    assert!(!state.should_pause_for_compaction(62.5));

    // Above threshold (83.3%)
    assert!(state.should_pause_for_compaction(83.3));
}

#[test]
fn test_is_grace_period() {
    let state = LoopState::default();
    // Grace period is 2 turns
    assert!(state.is_grace_period(0));
    assert!(state.is_grace_period(1));
    assert!(!state.is_grace_period(2));
}

#[test]
fn test_stop_sentinel() {
    let dir = tempdir().unwrap();
    let sentinel_path = dir.path().join("STOP");

    assert!(!LoopState::is_stopped(&sentinel_path));
    LoopState::create_stop_sentinel(&sentinel_path).unwrap();
    assert!(LoopState::is_stopped(&sentinel_path));
    LoopState::remove_stop_sentinel(&sentinel_path).unwrap();
    assert!(!LoopState::is_stopped(&sentinel_path));
}

#[test]
fn test_should_continue() {
    let dir = tempdir().unwrap();
    let sentinel_path = dir.path().join("STOP");

    let mut state = LoopState::default();
    state.running = true;

    // Running and no STOP sentinel -> continue
    assert!(state.should_continue(&sentinel_path));

    // Create STOP sentinel -> don't continue
    LoopState::create_stop_sentinel(&sentinel_path).unwrap();
    assert!(!state.should_continue(&sentinel_path));
}

#[test]
fn test_serialization_roundtrip() {
    let state = LoopState::default();
    let json = serde_json::to_string(&state).unwrap();
    let decoded: LoopState = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.running, state.running);
    assert_eq!(decoded.iteration, state.iteration);
    assert_eq!(decoded.current_phase, state.current_phase);
}
