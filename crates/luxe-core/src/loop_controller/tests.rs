use super::*;
use std::time::Duration;
use tempfile::tempdir;

#[tokio::test]
async fn test_loop_controller_default() {
    let dir = tempdir().unwrap();
    let controller = LoopController::new(dir.path().join("state.json"), dir.path().join("STOP"));
    let state = controller.get_state().await;
    assert!(!state.running);
    assert_eq!(state.iteration, 0);
    assert_eq!(state.current_phase, 0);
}

#[tokio::test]
async fn test_loop_controller_start_stop() {
    let dir = tempdir().unwrap();
    let controller = LoopController::new(dir.path().join("state.json"), dir.path().join("STOP"));

    // Start the loop.
    controller.start().await.unwrap();
    assert!(controller.is_running().await);

    // Stop the loop.
    controller.stop().await.unwrap();
    assert!(!controller.is_running().await);
}

#[tokio::test]
async fn test_loop_controller_pause_resume() {
    let dir = tempdir().unwrap();
    let controller = LoopController::new(dir.path().join("state.json"), dir.path().join("STOP"));

    controller.start().await.unwrap();
    assert!(controller.is_running().await);

    controller.pause().await.unwrap();
    assert!(!controller.is_running().await);

    controller.resume().await.unwrap();
    assert!(controller.is_running().await);
}

#[tokio::test]
async fn test_loop_controller_advance_phase() {
    let dir = tempdir().unwrap();
    let controller = LoopController::new(dir.path().join("state.json"), dir.path().join("STOP"));

    controller.set_phase(0).await.unwrap();
    assert_eq!(controller.get_state().await.current_phase, 0);

    controller.advance_phase().await.unwrap();
    assert_eq!(controller.get_state().await.current_phase, 1);

    controller.advance_phase().await.unwrap();
    assert_eq!(controller.get_state().await.current_phase, 2);
}

#[tokio::test]
async fn test_loop_controller_set_phase() {
    let dir = tempdir().unwrap();
    let controller = LoopController::new(dir.path().join("state.json"), dir.path().join("STOP"));

    controller.set_phase(5).await.unwrap();
    assert_eq!(controller.get_state().await.current_phase, 5);
}

#[tokio::test]
async fn test_loop_controller_persistence() {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join("state.json");
    let stop_path = dir.path().join("STOP");

    // Create controller, start it, and save.
    let controller1 = LoopController::new(state_path.clone(), stop_path.clone());
    controller1.start().await.unwrap();
    controller1.set_phase(3).await.unwrap();

    // Create a new controller pointing to the same files.
    let controller2 = LoopController::new(state_path, stop_path);
    controller2.load().await.unwrap();

    let state = controller2.get_state().await;
    assert!(state.running);
    assert_eq!(state.current_phase, 3);
}

#[tokio::test]
async fn test_loop_termination_reasons() {
    let dir = tempdir().unwrap();
    let controller = LoopController::new(dir.path().join("state.json"), dir.path().join("STOP"));

    // Not running -> Stopped.
    assert_eq!(controller.check_runaway_guards().await, LoopTerminationReason::Stopped);

    // Start the loop.
    controller.start().await.unwrap();
    assert_eq!(controller.check_runaway_guards().await, LoopTerminationReason::Continue);
}

#[tokio::test]
async fn test_loop_status_summary() {
    let dir = tempdir().unwrap();
    let controller = LoopController::new(dir.path().join("state.json"), dir.path().join("STOP"));

    controller.start().await.unwrap();
    let status = controller.status_summary().await;
    assert!(status.running);
    assert_eq!(status.iteration, 1);
    assert_eq!(status.current_phase, 0);
}

fn status(running: bool, liveness: crate::loop_state::LoopLiveness) -> LoopStatus {
    LoopStatus {
        running,
        liveness,
        iteration: 5,
        current_phase: 2,
        max_turns_per_run: 100,
        compact_percent: 70,
        pause_percent: 85,
    }
}

#[test]
fn test_loop_status_display() {
    use crate::loop_state::LoopLiveness;

    let display = format!("{}", status(true, LoopLiveness::Unrecorded));
    assert!(display.contains("loop: iter 5 / phase 2"));
    assert!(display.contains("70% / 85% context"));

    assert_eq!(format!("{}", status(false, LoopLiveness::Unrecorded)), "loop: paused");
}

/// A run whose process is gone reads as paused, whatever the flag on disk says.
///
/// The break this catches: `luxe loop status` reported `Running: true` for a loop that had been
/// killed hours earlier, because the flag is written by the run and a run that dies never gets
/// to correct it. Every other state stays trusting — an unrecorded owner is an older state file
/// or a `luxe loop resume`, and neither is evidence of a dead process.
#[test]
fn a_run_whose_process_is_gone_is_not_running() {
    use crate::loop_state::LoopLiveness;

    assert!(!status(true, LoopLiveness::Gone { pid: 4242 }).is_live());
    assert_eq!(format!("{}", status(true, LoopLiveness::Gone { pid: 4242 })), "loop: paused");
    assert!(status(true, LoopLiveness::Alive { pid: 4242 }).is_live());
    assert!(status(true, LoopLiveness::Unrecorded).is_live());
    assert!(
        status(true, LoopLiveness::Elsewhere { host: "other-box".into() }).is_live(),
        "another machine's run is not this machine's to declare dead"
    );

    let gone = status(true, LoopLiveness::Gone { pid: 4242 }).liveness_line().unwrap();
    assert!(gone.contains("4242"), "it names the process: {gone}");
    assert!(gone.contains("luxe loop start"), "and the way forward: {gone}");
    assert_eq!(status(true, LoopLiveness::Unrecorded).liveness_line(), None);
}

#[tokio::test]
async fn test_running_rx_after_start() {
    let tmp = std::env::temp_dir().join(format!("luxe-test-running-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let controller = LoopController::new(tmp.join("state.json"), tmp.join("STOP"));
    controller.start().await.unwrap();
    let rx = controller.running_rx();
    assert!(*rx.borrow(), "running_rx should reflect running=true after start()");
}

#[tokio::test]
async fn test_running_rx_after_stop() {
    let tmp = std::env::temp_dir().join(format!("luxe-test-running-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let controller = LoopController::new(tmp.join("state.json"), tmp.join("STOP"));
    controller.start().await.unwrap();
    let mut rx = controller.running_rx();
    controller.stop().await.unwrap();
    // Should resolve immediately after stop()
    tokio::time::timeout(Duration::from_millis(100), rx.changed())
        .await
        .expect("changed() should resolve after stop()")
        .expect("changed() should not error");
    assert!(!*rx.borrow(), "running_rx should reflect running=false after stop()");
}

/// Starting after a stop really runs, rather than starting over a stop sentinel.
///
/// The break this catches, which shipped: `stop` writes `.luxe/STOP` and `is_running` reads
/// that file as well as the flag, but `start` only set the flag. So `luxe loop stop` followed
/// by `luxe loop start` printed its banner over a loop every hook still considered stopped —
/// and stayed that way until someone deleted the file by hand. `resume` cleared it all along,
/// which is why this went unnoticed.
#[tokio::test]
async fn a_started_loop_is_running_even_after_a_previous_stop() {
    let tmp = tempfile::tempdir().unwrap();
    let controller = LoopController::new(tmp.path().join("state.json"), tmp.path().join("STOP"));

    controller.start().await.unwrap();
    assert!(controller.is_running().await);

    controller.stop().await.unwrap();
    assert!(!controller.is_running().await, "a stopped loop is stopped");

    controller.start().await.unwrap();
    assert!(
        controller.is_running().await,
        "a restarted loop must actually run, not merely say it does"
    );
    assert!(
        !LoopState::is_stopped(&tmp.path().join("STOP")),
        "the stale sentinel must be gone, not just overridden"
    );
}

#[tokio::test]
async fn test_running_rx_across_clone() {
    let tmp = std::env::temp_dir().join(format!("luxe-test-running-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let controller = LoopController::new(tmp.join("state.json"), tmp.join("STOP"));
    let mut rx = controller.running_rx();
    // Stop on the clone — rx (from original) should see it
    controller.clone().stop().await.unwrap();
    tokio::time::timeout(Duration::from_millis(100), rx.changed())
        .await
        .expect("changed() should resolve after clone's stop()")
        .expect("changed() should not error");
    assert!(!*rx.borrow(), "running_rx from original controller should see clone's stop()");
}
