//! Baseline retention — a finished turn clears out the turn baselines nothing reads any more.

use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::guided_review::TURN_BASELINE_CACHE_RELATIVE_DIR;
use luxe_core::session::SessionStore;

use crate::fixtures::*;

fn committed_project(root: &std::path::Path) {
    std::fs::write(root.join("test.txt"), "Hello, World!").unwrap();
    for arguments in [
        ["init"].as_slice(),
        ["config", "user.email", "tests@example.invalid"].as_slice(),
        ["config", "user.name", "luxe Tests"].as_slice(),
        ["add", "test.txt"].as_slice(),
        ["commit", "-m", "initial fixture"].as_slice(),
    ] {
        let status = Command::new("git").args(arguments).current_dir(root).status().unwrap();
        assert!(status.success(), "git {arguments:?} failed");
    }
}

/// An hours-old baseline no record names, as a long session leaves behind.
fn stale_baseline(root: &std::path::Path) -> std::path::PathBuf {
    let stale = root.join(TURN_BASELINE_CACHE_RELATIVE_DIR).join("f".repeat(64));
    std::fs::create_dir_all(stale.join("files")).unwrap();
    let hours_ago = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
    std::fs::File::open(&stale).unwrap().set_modified(hours_ago).unwrap();
    stale
}

/// Waits out the post-turn capture, which runs once the turn has ended and is done when it says
/// the turn can be reviewed.
async fn last_turn_available(events: &mut tokio::sync::mpsc::Receiver<Event>) {
    let wait = async {
        while let Some(event) = events.recv().await {
            if matches!(event, Event::LastTurnAvailable { .. }) {
                return;
            }
        }
        panic!("the engine stopped before the turn could be reviewed");
    };
    tokio::time::timeout(EVENT_WAIT, wait).await.expect("the post-turn capture finishes");
}

/// The break this catches: a turn that saves its record without pruning, so every turn leaves two
/// full copies of the workspace behind for good.
#[tokio::test]
async fn a_finished_turn_prunes_old_baselines_and_keeps_its_own() {
    let project = tempfile::tempdir().unwrap();
    committed_project(project.path());
    let stale = stale_baseline(project.path());
    let mut store = project_store(project.path());
    let session_id = store.create(&project.path().display().to_string()).unwrap();
    let config = EngineConfig {
        guided_review_project_root: Some(project.path().to_path_buf()),
        ..base_config(store, session_id, Arc::new(MockProvider::new()))
    };
    let (actions, mut events) = Engine::spawn(config);
    let message = Action::SubmitUserMessage {
        content: "Read test.txt".to_string(),
        images: vec![],
        remediation: None,
    };
    actions.send(message).await.unwrap();
    last_turn_available(&mut events).await;

    assert!(!stale.exists(), "the hours-old baseline survived the turn");
    let cache = project.path().join(TURN_BASELINE_CACHE_RELATIVE_DIR);
    assert_eq!(std::fs::read_dir(cache).unwrap().count(), 2, "the turn's own pair should stay");
}
