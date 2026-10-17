//! Review restore — the engine owns a guided review through load, refresh, close and restart.

use std::process::Command;
use std::sync::Arc;

use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::guided_review::{
    ActiveReviewRecordStore, AgentTurnId, FindingAnchor, FindingId, FindingSeverity,
    LastTurnRecord, LastTurnRecordStore, LocalGit, ReviewFeedbackMode, ReviewLoadRequestId,
    ReviewTarget, SubmissionId, SubmittedFinding, UntrackedSourcePolicy,
};
use luxe_core::session::{JsonlSessionStore, SessionStore};

use crate::fixtures::*;

/// A review of the last turn loads from the baseline the turn advertised, then refreshes in place
/// and refuses a turn it never retained.
///
/// Two turns run first so the review has a real baseline to name; what those turns do is
/// `turns`'s business.
#[tokio::test]
async fn a_last_turn_review_loads_the_turn_that_ran_and_refreshes_in_place() {
    let temp_dir = tempfile::tempdir().unwrap();
    let tmp_dir = temp_dir.path();
    assert!(Command::new("git").arg("init").current_dir(tmp_dir).status().unwrap().success());

    // Create a test file for the read tool.
    std::fs::write(tmp_dir.join("test.txt"), "Hello, World!").unwrap();
    for arguments in [
        ["config", "user.email", "tests@example.invalid"].as_slice(),
        ["config", "user.name", "Luxe Tests"].as_slice(),
        ["add", "test.txt"].as_slice(),
        ["commit", "-m", "initial fixture"].as_slice(),
    ] {
        assert!(Command::new("git")
            .args(arguments)
            .current_dir(tmp_dir)
            .status()
            .unwrap()
            .success());
    }

    // Luxe's own planning artifact, not the harness it happens to be developed with.
    std::fs::write(tmp_dir.join("PLAN.md"), "review plan evidence").unwrap();

    // Setup session store.
    let mut store = JsonlSessionStore::new(tmp_dir.to_path_buf());
    let session_id = store.create(&tmp_dir.display().to_string()).unwrap();

    let config = EngineConfig {
        guided_review_project_root: Some(tmp_dir.to_path_buf()),
        ..base_config(store, session_id.clone(), Arc::new(MockProvider::new()))
    };

    // Spawn the engine.
    let (action_tx, mut event_rx) = Engine::spawn(config);

    // Submit a user message.
    action_tx
        .send(luxe_core::engine::Action::SubmitUserMessage {
            content: "Read test.txt".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    events_until_turn_end(&mut event_rx).await;

    // Submit another message for second turn.
    action_tx
        .send(luxe_core::engine::Action::SubmitUserMessage {
            content: "Edit test.txt".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();
    let events_turn2 = events_until_turn_end(&mut event_rx).await;

    let (baseline_session_id, baseline_turn_id) = events_turn2
        .iter()
        .find_map(|event| match event {
            Event::LastTurnAvailable { session_id, turn_id } => {
                Some((session_id.clone(), turn_id.clone()))
            }
            _ => None,
        })
        .expect("successful pre-turn capture should advertise its exact LastTurn identity");
    let request_id = luxe_core::guided_review::ReviewLoadRequestId::new();
    action_tx
        .send(luxe_core::engine::Action::LoadGuidedReview {
            request_id: request_id.clone(),
            target: luxe_core::guided_review::ReviewTarget::LastTurn {
                session_id: baseline_session_id,
                turn_id: baseline_turn_id,
            },
        })
        .await
        .unwrap();
    let mut saw_loading = false;
    let loaded_review = loop {
        match tokio::time::timeout(EVENT_WAIT, event_rx.recv())
            .await
            .expect("matching LastTurn request should resolve")
        {
            Some(Event::GuidedReviewLoading { request_id: loading, .. })
                if loading == request_id =>
            {
                saw_loading = true
            }
            Some(Event::GuidedReviewLoaded { request_id: loaded, review })
                if loaded == request_id =>
            {
                assert!(saw_loading, "LastTurn review should report its loading state first");
                break review;
            }
            Some(Event::GuidedReviewFailed { error, .. }) => {
                panic!("matching retained LastTurn request failed: {}", error.message)
            }
            Some(_) => {}
            None => panic!("Event channel closed unexpectedly"),
        }
    };

    assert_eq!(loaded_review.context.goal.as_deref(), Some("Edit test.txt"));
    assert_eq!(loaded_review.context.planning_excerpt.as_deref(), Some("review plan evidence"));
    let expected_revision = loaded_review.revision_id.clone();
    let expected_review_id = loaded_review.id.clone();
    let refresh_request = luxe_core::guided_review::ReviewLoadRequestId::new();
    action_tx
        .send(luxe_core::engine::Action::RefreshGuidedReview {
            request_id: refresh_request.clone(),
            review_id: loaded_review.id.clone(),
            expected_revision,
        })
        .await
        .unwrap();
    let mut saw_refresh_loading = false;
    loop {
        match tokio::time::timeout(EVENT_WAIT, event_rx.recv())
            .await
            .expect("refresh request should resolve")
        {
            Some(Event::GuidedReviewLoading { request_id, .. })
                if request_id == refresh_request =>
            {
                saw_refresh_loading = true;
            }
            Some(Event::GuidedReviewLoaded { request_id, review })
                if request_id == refresh_request =>
            {
                assert!(saw_refresh_loading, "refresh should report its loading state first");
                assert_eq!(review.id, expected_review_id);
                break;
            }
            Some(Event::GuidedReviewFailed { error, .. }) => {
                panic!("refreshing the pinned review failed: {}", error.message)
            }
            Some(_) => {}
            None => panic!("Event channel closed unexpectedly"),
        }
    }

    let request_id = luxe_core::guided_review::ReviewLoadRequestId::new();
    let target = luxe_core::guided_review::ReviewTarget::LastTurn {
        session_id: session_id.to_string(),
        turn_id: "unretained-turn".into(),
    };
    action_tx
        .send(luxe_core::engine::Action::LoadGuidedReview {
            request_id: request_id.clone(),
            target: target.clone(),
        })
        .await
        .unwrap();
    // A background Guide pass for the earlier review may still report in, so wait for the
    // failure rather than assuming it is the very next event.
    let (failed_request, failed_target, error) = tokio::time::timeout(EVENT_WAIT, async {
        loop {
            match event_rx.recv().await {
                Some(Event::GuidedReviewFailed { request_id, target, error }) => {
                    break (request_id, target, error)
                }
                Some(_) => {}
                None => panic!("expected typed unavailable failure for unretained LastTurn"),
            }
        }
    })
    .await
    .expect("unretained LastTurn request should resolve");
    assert_eq!(failed_request, request_id);
    assert_eq!(failed_target, target);
    assert_eq!(error.kind, luxe_core::event::GuidedReviewFailureKind::Unavailable);

    // Cancel and close.
    action_tx.send(luxe_core::engine::Action::Cancel).await.unwrap();
    action_tx.send(luxe_core::engine::Action::Close).await.unwrap();
}

#[tokio::test]
async fn p32_guided_review_refresh_persists_a_fresh_artifact_after_worktree_mutation() {
    let temp_dir = tempfile::tempdir().unwrap();
    let project_root = temp_dir.path();
    assert!(Command::new("git").arg("init").current_dir(project_root).status().unwrap().success());
    std::fs::write(project_root.join("tracked.rs"), "fn original() {}\n").unwrap();
    for arguments in [
        ["config", "user.email", "tests@example.invalid"].as_slice(),
        ["config", "user.name", "Luxe Tests"].as_slice(),
        ["add", "tracked.rs"].as_slice(),
        ["commit", "-m", "initial fixture"].as_slice(),
    ] {
        assert!(Command::new("git")
            .args(arguments)
            .current_dir(project_root)
            .status()
            .unwrap()
            .success());
    }
    std::fs::write(project_root.join("tracked.rs"), "fn changed() {}\n").unwrap();

    let mut store = JsonlSessionStore::new(project_root.to_path_buf());
    let session_id = store.create(&project_root.display().to_string()).unwrap();
    let provider = Arc::new(GuideManifestProvider::default());
    let config = EngineConfig {
        guided_review_project_root: Some(project_root.to_path_buf()),
        ..base_config(store, session_id.clone(), provider.clone())
    };
    let (action_tx, mut event_rx) = Engine::spawn(config);
    let request_id = ReviewLoadRequestId::new();
    action_tx
        .send(luxe_core::engine::Action::LoadGuidedReview {
            request_id: request_id.clone(),
            target: ReviewTarget::WorkingTree,
        })
        .await
        .unwrap();

    let review = tokio::time::timeout(EVENT_WAIT, async {
        loop {
            match event_rx.recv().await {
                Some(Event::GuidedReviewLoaded { request_id: loaded_request, review })
                    if loaded_request == request_id =>
                {
                    break review
                }
                Some(_) => {}
                None => panic!("engine event channel closed before review loaded"),
            }
        }
    })
    .await
    .expect("guided review load timed out");

    // The review opens on deterministic chapters so the diff never waits on a model...
    assert_eq!(review.chapters[0].title, "Core implementation");
    let record = ActiveReviewRecordStore::new(project_root).load().unwrap().unwrap();
    assert_eq!(record.review_id, review.id);
    assert_eq!(record.revision_id, review.revision_id);

    // ...and the model-authored Guide lands afterwards as an in-place upgrade for that revision.
    let (chapters, provenance) = tokio::time::timeout(EVENT_WAIT, async {
        loop {
            match event_rx.recv().await {
                Some(Event::GuidedReviewGuideUpdated {
                    review_id,
                    revision_id,
                    chapters,
                    provenance,
                    ..
                }) if review_id == review.id && revision_id == review.revision_id => {
                    break (chapters, provenance)
                }
                Some(_) => {}
                None => panic!("engine event channel closed before the Guide arrived"),
            }
        }
    })
    .await
    .expect("guide generation timed out");
    assert_eq!(chapters[0].title, "Model Guide");
    assert!(matches!(provenance, luxe_core::guided_review::GuideProvenance::Model { .. }));
    // A restart reopens the chaptered Guide, not the structural placeholder it started from.
    let record = ActiveReviewRecordStore::new(project_root).load().unwrap().unwrap();
    assert_eq!(record.chapters[0].title, "Model Guide");

    // The same change again gets the guide it already has, without asking the model twice
    let asked = provider.guides.load(std::sync::atomic::Ordering::SeqCst);
    assert!(asked > 0);
    let again = ReviewLoadRequestId::new();
    action_tx
        .send(luxe_core::engine::Action::LoadGuidedReview {
            request_id: again.clone(),
            target: ReviewTarget::WorkingTree,
        })
        .await
        .unwrap();
    let (chapters, provenance) = tokio::time::timeout(EVENT_WAIT, async {
        loop {
            match event_rx.recv().await {
                Some(Event::GuidedReviewGuideUpdated {
                    request_id, chapters, provenance, ..
                }) if request_id == again => break (chapters, provenance),
                Some(_) => {}
                None => panic!("engine event channel closed before the reused Guide arrived"),
            }
        }
    })
    .await
    .expect("the reused guide arrives");
    assert_eq!(chapters[0].title, "Model Guide");
    assert!(matches!(provenance, luxe_core::guided_review::GuideProvenance::Model { .. }));
    assert_eq!(provider.guides.load(std::sync::atomic::Ordering::SeqCst), asked, "no second pass");
    // The feedback below goes to the review the engine now holds, which is this second one
    let review = Box::new(
        ActiveReviewRecordStore::new(project_root)
            .load()
            .unwrap()
            .unwrap()
            .restore(project_root, &luxe_core::guided_review::ReviewExclusions::default())
            .unwrap(),
    );

    let finding = SubmittedFinding {
        finding_id: FindingId::new(),
        label: "R1".into(),
        evidence: Default::default(),
        anchor: FindingAnchor::Review,
        severity: FindingSeverity::Issue,
        title: "Keep the tracked function focused".into(),
        body: "The reviewed source needs the bounded follow-up change.".into(),
        proposed_fix: None,
        thread: Vec::new(),
        github_author: None,
    };
    let submission_id = SubmissionId::new();
    action_tx
        .send(Action::SubmitReviewFeedback {
            submission_id: submission_id.clone(),
            review_id: review.id.clone(),
            source: review.source.clone(),
            mode: ReviewFeedbackMode::RequestFixes,
            findings: vec![finding.clone()],
        })
        .await
        .unwrap();
    let mut remediation_seen = false;
    let feedback_deadline = tokio::time::Instant::now() + EVENT_WAIT;
    while tokio::time::Instant::now() < feedback_deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(200), event_rx.recv()).await {
            Ok(Some(Event::RemediationReported { report, .. })) => {
                assert_eq!(report.review_id, review.id);
                assert_eq!(report.revision_id, review.source.content_digest);
                assert_eq!(report.findings[0].finding_id, finding.finding_id);
                remediation_seen = true;
            }
            Ok(Some(Event::ReviewFeedbackTurnCompleted { submission_id: completed })) => {
                assert_eq!(completed, submission_id);
                break;
            }
            Ok(Some(_)) | Err(_) => {}
            Ok(None) => panic!("engine event channel closed before feedback completion"),
        }
    }
    assert!(remediation_seen, "the pinned feedback emits remediation evidence");

    std::fs::write(project_root.join("tracked.rs"), "fn later_mutation() {}\n").unwrap();
    let refresh_request = ReviewLoadRequestId::new();
    action_tx
        .send(luxe_core::engine::Action::RefreshGuidedReview {
            request_id: refresh_request.clone(),
            expected_revision: review.revision_id.clone(),
            review_id: review.id.clone(),
        })
        .await
        .unwrap();
    let refreshed = tokio::time::timeout(EVENT_WAIT, async {
        loop {
            match event_rx.recv().await {
                Some(Event::GuidedReviewLoaded { request_id: loaded_request, review })
                    if loaded_request == refresh_request =>
                {
                    break review
                }
                Some(Event::GuidedReviewFailed { error, .. }) => {
                    panic!("fresh review failed: {}", error.message)
                }
                Some(_) => {}
                None => panic!("engine event channel closed before refreshed review loaded"),
            }
        }
    })
    .await
    .expect("guided review refresh timed out");
    assert_eq!(refreshed.id, review.id);
    assert_ne!(refreshed.revision_id, review.revision_id);
    assert_ne!(refreshed.source.content_digest, review.source.content_digest);
    let refreshed_lines = refreshed
        .files
        .iter()
        .flat_map(|file| &file.hunks)
        .flat_map(|hunk| &hunk.lines)
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>();
    assert!(refreshed_lines.iter().any(|line| line.contains("fn later_mutation()")));

    std::fs::write(project_root.join("tracked.rs"), "fn final_mutation() {}\n").unwrap();
    drop(action_tx);

    let resumed_config = EngineConfig {
        guided_review_project_root: Some(project_root.to_path_buf()),
        ..base_config(
            JsonlSessionStore::new(project_root.to_path_buf()),
            session_id,
            Arc::new(MockProvider::new()),
        )
    };
    let (_resumed_actions, mut resumed_events) = Engine::spawn(resumed_config);
    let restored = tokio::time::timeout(EVENT_WAIT, async {
        loop {
            match resumed_events.recv().await {
                Some(Event::GuidedReviewLoaded { review, .. }) => break review,
                Some(_) => {}
                None => panic!("resumed engine event channel closed before review loaded"),
            }
        }
    })
    .await
    .expect("persisted guided review restore timed out");

    assert_eq!(restored.id, refreshed.id);
    assert_eq!(restored.revision_id, refreshed.revision_id);
    let restored_lines = restored
        .files
        .iter()
        .flat_map(|file| &file.hunks)
        .flat_map(|hunk| &hunk.lines)
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>();
    assert!(restored_lines.iter().any(|line| line.contains("fn later_mutation()")));
    assert!(!restored_lines.iter().any(|line| line.contains("fn final_mutation()")));
}

#[tokio::test]
async fn guided_review_last_turn_loads_a_retained_baseline_after_restart() {
    let temp_dir = tempfile::tempdir().unwrap();
    let project_root = temp_dir.path();
    assert!(Command::new("git").arg("init").current_dir(project_root).status().unwrap().success());
    std::fs::write(project_root.join("tracked.rs"), "fn original() {}\n").unwrap();
    for arguments in [
        ["config", "user.email", "tests@example.invalid"].as_slice(),
        ["config", "user.name", "Luxe Tests"].as_slice(),
        ["add", "tracked.rs"].as_slice(),
        ["commit", "-m", "initial fixture"].as_slice(),
    ] {
        assert!(Command::new("git")
            .args(arguments)
            .current_dir(project_root)
            .status()
            .unwrap()
            .success());
    }

    let mut store = JsonlSessionStore::new(project_root.to_path_buf());
    let session_id = store.create(&project_root.display().to_string()).unwrap();
    let turn_id = AgentTurnId::new();
    let git = LocalGit::new(project_root);
    git.capture_and_persist_turn_baseline(
        &session_id.to_string(),
        turn_id.as_str(),
        UntrackedSourcePolicy::default(),
    )
    .await
    .unwrap();
    std::fs::write(project_root.join("tracked.rs"), "fn changed_after_turn() {}\n").unwrap();
    git.capture_and_persist_post_turn_snapshot(
        &session_id.to_string(),
        turn_id.as_str(),
        UntrackedSourcePolicy::default(),
    )
    .await
    .unwrap();
    LastTurnRecordStore::new(project_root)
        .save(&LastTurnRecord::new(session_id.to_string(), turn_id.as_str()).unwrap())
        .unwrap();
    std::fs::write(project_root.join("tracked.rs"), "fn mutated_after_turn() {}\n").unwrap();

    let config = EngineConfig {
        guided_review_project_root: Some(project_root.to_path_buf()),
        ..base_config(store, session_id.clone(), Arc::new(MockProvider::new()))
    };
    let (action_tx, mut event_rx) = Engine::spawn(config);
    let restored_turn = tokio::time::timeout(EVENT_WAIT, async {
        loop {
            match event_rx.recv().await {
                Some(Event::LastTurnAvailable {
                    session_id: restored_session,
                    turn_id: restored_turn,
                }) => {
                    break (restored_session, restored_turn);
                }
                Some(_) => {}
                None => {
                    panic!("engine event channel closed before Last Turn availability restored")
                }
            }
        }
    })
    .await
    .expect("Last Turn availability restore timed out");
    assert_eq!(restored_turn.0, session_id.to_string());
    assert_eq!(restored_turn.1, turn_id.as_str());

    let request_id = ReviewLoadRequestId::new();
    action_tx
        .send(luxe_core::engine::Action::LoadGuidedReview {
            request_id: request_id.clone(),
            target: ReviewTarget::LastTurn {
                session_id: session_id.to_string(),
                turn_id: turn_id.as_str().to_owned(),
            },
        })
        .await
        .unwrap();

    let review = tokio::time::timeout(EVENT_WAIT, async {
        loop {
            match event_rx.recv().await {
                Some(Event::GuidedReviewLoaded { request_id: loaded_request, review })
                    if loaded_request == request_id =>
                {
                    break review
                }
                Some(_) => {}
                None => panic!("engine event channel closed before Last Turn review loaded"),
            }
        }
    })
    .await
    .expect("Last Turn review load timed out");

    assert!(matches!(review.target, ReviewTarget::LastTurn { .. }));
    let lines = review
        .files
        .iter()
        .flat_map(|file| &file.hunks)
        .flat_map(|hunk| &hunk.lines)
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>();
    assert!(lines.iter().any(|line| line.contains("fn changed_after_turn()")));
    assert!(!lines.iter().any(|line| line.contains("fn mutated_after_turn()")));
}

mod ownership;
