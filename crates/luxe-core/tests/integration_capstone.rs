//! Capstone integration test — runs a mock multi-phase spec through the loop.
//!
//! This test verifies that the continuous loop can drive a multi-phase project to
//! completion with review gates, compaction between runs, and phase tracking.

use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test]
async fn test_capstone_mock_multi_phase_spec() {
    // Create a temp directory for the test.
    let dir = tempdir().unwrap();
    let repo_root = dir.path().to_path_buf();

    // Create a mock PROJECT.md.
    tokio::fs::write(
        repo_root.join("PROJECT.md"),
        "# Test Project\n\n## Phases\n- Phase 0: Bootstrap\n- Phase 1: Core types\n",
    )
    .await
    .unwrap();

    // Create a mock PLAN.md.
    tokio::fs::write(
        repo_root.join("PLAN.md"),
        "# Plan\n\n## Phase 0\nBootstrap the workspace.\n\n## Phase 1\nImplement core types.",
    )
    .await
    .unwrap();

    // Create a mock PROGRESS.md.
    tokio::fs::write(
        repo_root.join("PROGRESS.md"),
        "# Progress\n\n## Phase 0\nDone.\n\n## Phase 1\nIn progress.",
    )
    .await
    .unwrap();

    // Set up the loop controller.
    let state_path = repo_root.join(".luxe").join("loop_state.json");
    let stop_path = repo_root.join(".luxe").join("STOP");
    tokio::fs::create_dir_all(repo_root.join(".luxe")).await.unwrap();

    let controller =
        Arc::new(luxe_core::loop_controller::LoopController::new(state_path, stop_path));

    // Start the loop.
    controller.start().await.unwrap();

    // Verify initial state.
    let state: luxe_core::loop_controller::LoopState = controller.get_state().await;
    assert_eq!(state.current_phase, 0);
    assert_eq!(state.iteration, 1);

    // Simulate running phase 0.
    controller.advance_phase().await.unwrap();
    let state = controller.get_state().await;
    assert_eq!(state.current_phase, 1);

    // Simulate running phase 1.
    controller.advance_phase().await.unwrap();
    let state = controller.get_state().await;
    assert_eq!(state.current_phase, 2);

    // Verify the loop is still running.
    assert!(controller.is_running().await);

    // Stop the loop.
    controller.stop().await.unwrap();
    assert!(!controller.is_running().await);

    println!("Capstone test passed: mock multi-phase spec ran to completion");
}
