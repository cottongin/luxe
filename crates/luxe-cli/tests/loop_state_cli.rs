//! `luxe loop` subcommands act on the run that exists, not on a fresh one.

use std::process::Command;

/// Write a loop state that looks like a run in progress.
fn a_run_in_progress(dir: &std::path::Path) -> std::path::PathBuf {
    let luxe = dir.join(".luxe");
    std::fs::create_dir_all(&luxe).unwrap();
    let path = luxe.join("loop_state.json");
    std::fs::write(
        &path,
        serde_json::json!({
            "running": false,
            "iteration": 40,
            "current_phase": 3,
            "phase_baseline": "5cee240408c0deadbeef",
            "max_turns_per_run": 8,
            "compact_percent": 75,
            "pause_percent": 90,
            "journal_max_kb": 24,
            "confirmations_required": 2,
            "context_grace_turns": 2,
            "max_no_progress_turns": 12,
            "max_stalls": 5,
            "rate_limit_backoff_ms": 30000,
            "compact_timeout_ms": 600000,
            "max_consecutive_errors": 5,
            "spend": { "total": { "calls": 22, "prompt_tokens": 210983 }, "by_model": {} }
        })
        .to_string(),
    )
    .unwrap();
    path
}

fn state(path: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn luxe(dir: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_luxe"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("the binary runs");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Setting the phase by hand keeps everything else about the run.
///
/// The break this catches, and it shipped: no `luxe loop` subcommand read the state file, so each
/// acted on a default `LoopState` and the ones that save wrote those defaults back. `luxe loop phase
/// --set` is the documented lever for correcting a drifted phase number, and using it reset the
/// iteration count, the run's spend, every tuning field, and `phase_baseline` — which is what a phase
/// review diffs against, so the next review would have compared the tree to `HEAD` and seen nothing.
#[test]
fn setting_the_phase_does_not_reset_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let path = a_run_in_progress(dir.path());

    luxe(dir.path(), &["loop", "phase", "--set", "5"]);

    let after = state(&path);
    assert_eq!(after["current_phase"], 5, "the phase is what was asked for");
    assert_eq!(after["iteration"], 40, "the iteration count survives");
    assert_eq!(after["phase_baseline"], "5cee240408c0deadbeef", "the review's baseline survives");
    assert_eq!(after["spend"]["total"]["calls"], 22, "the run's spend survives");
    assert_eq!(after["max_turns_per_run"], 8, "a tuning field is not reset to its default");
}

/// `status` reports the run on disk.
///
/// The break this catches: printing a default `LoopState`, so a paused run at phase 3 iteration 40
/// read as phase 0 iteration 0 — a status command that cannot report status.
#[test]
fn status_reports_the_run_that_exists() {
    let dir = tempfile::tempdir().unwrap();
    a_run_in_progress(dir.path());

    let out = luxe(dir.path(), &["loop", "status"]);

    assert!(out.contains("Iteration: 40"), "{out}");
    assert!(out.contains("Phase: 3"), "{out}");
    assert!(out.contains("Max turns: 8"), "{out}");
}

/// A project that has never run a loop still answers, rather than failing on a missing file.
#[test]
fn a_project_with_no_loop_yet_reports_a_fresh_one() {
    let dir = tempfile::tempdir().unwrap();

    let out = luxe(dir.path(), &["loop", "status"]);

    assert!(out.contains("Iteration: 0"), "{out}");
    assert!(!out.contains("Spend this run"), "nothing spent, nothing shown: {out}");
}

/// Write a loop state that claims to be running, owned by `pid` on this machine.
fn a_run_claiming_to_be_alive(dir: &std::path::Path, pid: u32) -> std::path::PathBuf {
    let path = a_run_in_progress(dir);
    let mut value = state(&path);
    value["running"] = serde_json::json!(true);
    value["owner"] = serde_json::json!({
        "pid": pid,
        "host": luxe_core::program::hostname(),
        "started_at": 1_700_000_000i64,
    });
    std::fs::write(&path, value.to_string()).unwrap();
    path
}

/// A run whose process is gone is reported as not running, and says why.
///
/// The break this catches: `running` is written by the run itself, so a loop that was killed — or
/// crashed, or lost the terminal it was started in — leaves `true` on disk forever. `luxe loop
/// status` repeated it, which is the one thing a status command must not do.
#[test]
fn status_does_not_report_a_dead_run_as_running() {
    let dir = tempfile::tempdir().unwrap();
    // Well past any system's pid_max, so nothing can have been given this id since.
    a_run_claiming_to_be_alive(dir.path(), u32::MAX - 1);

    let out = luxe(dir.path(), &["loop", "status"]);

    assert!(out.contains("Running: false"), "{out}");
    assert!(out.contains("is gone"), "and it says what happened: {out}");
    assert!(out.contains("luxe loop start"), "and what to do about it: {out}");
    assert!(out.contains("Phase: 3"), "the run it describes is still the one on disk: {out}");
}

/// A second loop in the same project is refused while the first one is alive.
///
/// They share a phase counter, a journal and a `STOP` sentinel: whichever starts second corrupts
/// the first one's run, silently, and the two then fight over the same files.
///
/// Only a *live* owner refuses. The other half — that a crashed run does not lock the project out
/// of starting again — is `loop_state`'s `the_current_process_is_alive_and_is_us`, because the
/// command that would prove it here is one that runs until it is stopped.
#[test]
fn a_second_loop_will_not_start_over_a_live_one() {
    let dir = tempfile::tempdir().unwrap();
    // This test process is alive by construction, and is not the `luxe` about to be spawned.
    a_run_claiming_to_be_alive(dir.path(), std::process::id());

    let out = Command::new(env!("CARGO_BIN_EXE_luxe"))
        .args(["loop", "start"])
        .current_dir(dir.path())
        .output()
        .expect("the binary runs");

    assert!(!out.status.success(), "starting over a live loop must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("already running"), "{stderr}");
    assert!(stderr.contains("luxe loop stop"), "and names the way out: {stderr}");
}
