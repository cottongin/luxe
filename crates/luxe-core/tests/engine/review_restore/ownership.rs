//! Ownership — the engine holds a review from comment to report, through close and a restart.

use std::process::Command;
use std::sync::Arc;

use futures::stream::{self, BoxStream};
use luxe_core::engine::{Action, Engine};
use luxe_core::event::Event;
use luxe_core::guided_review::{FindingAnchor, FindingSeverity, ReviewLoadRequestId, ReviewTarget};
use luxe_core::provider::{CompletionRequest, Provider, ProviderError, ProviderEvent, StopReason};
use luxe_core::session::{JsonlSessionStore, SessionStore};

use crate::fixtures::*;

/// The engine owns a review from the first comment to the agent's report, and through a restart.
///
/// The whole loop the TUI used to run by hand, with nothing but intents going in: a comment sent
/// with `send`, the feedback turn it starts, the report that turn writes, each stored as it
/// happens, and all of it back after a restart. The restart half is the defect this replaces: the
/// restored session reached the front end under a request it never sent, and was dropped.
#[tokio::test]
async fn the_engine_owns_a_review_from_comment_to_report_and_through_a_restart() {
    use luxe_core::guided_review::{FindingState, ReviewSessionSaveRequestId, SubmissionDispatch};
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
    let session_id = JsonlSessionStore::new(project_root.to_path_buf())
        .create(&project_root.display().to_string())
        .unwrap();

    let (actions, mut events) =
        Engine::spawn(review_engine_config(project_root, session_id.clone()));
    let request_id = ReviewLoadRequestId::new();
    actions
        .send(Action::LoadGuidedReview {
            request_id: request_id.clone(),
            target: ReviewTarget::WorkingTree,
        })
        .await
        .unwrap();
    let review = first(&mut events, "the review", |event| match event {
        Event::GuidedReviewLoaded { request_id: loaded, review } if loaded == request_id => {
            Some(review)
        }
        _ => None,
    })
    .await;
    // The next review event is its session: always, so a front end never has a review without one
    let bound = first(&mut events, "the review's session", |event| match event {
        Event::ReviewSessionRestored { session } => Some(session),
        Event::GuidedReviewLoaded { .. } => panic!("a second review arrived before the session"),
        _ => None,
    })
    .await;
    assert_eq!(bound.id, review.id);
    assert!(bound.findings.is_empty());

    let comment = ReviewSessionSaveRequestId::new();
    actions
        .send(Action::EditReview {
            request_id: comment.clone(),
            review_id: review.id.clone(),
            edit: luxe_core::engine::ReviewEdit::Comment {
                draft: luxe_core::engine::CommentDraft {
                    anchor: FindingAnchor::Review,
                    severity: FindingSeverity::Issue,
                    title: "Keep the tracked function focused".into(),
                    body: "The reviewed source needs the bounded follow-up change.".into(),
                    proposed_fix: None,
                },
                send: true,
            },
        })
        .await
        .unwrap();
    let saved = first(&mut events, "the comment's save", |event| match event {
        Event::ReviewSessionSaved { request_id, session } if request_id == comment => Some(session),
        Event::ReviewSessionSaveFailed { message, .. } => {
            panic!("the comment was refused: {message}")
        }
        _ => None,
    })
    .await;
    assert_eq!(saved.findings[0].label, "R1");
    assert_eq!(saved.submissions[0].dispatch, SubmissionDispatch::Queued);

    // The turn it started runs, reports, and finishes, and the desk stores each step
    let finished = first(&mut events, "the stored completion", |event| match event {
        Event::ReviewSessionSaved { session, .. }
            if session.submissions.first().map(|s| s.dispatch)
                == Some(SubmissionDispatch::Completed) =>
        {
            Some(session)
        }
        _ => None,
    })
    .await;
    assert_eq!(finished.findings[0].state, FindingState::NeedsVerification, "the agent says fixed");
    assert!(finished.findings[0]
        .thread
        .iter()
        .any(|m| m.body.contains("Updated the tracked source.")));
    // Let the first engine finish writing before another one opens the project
    actions.send(Action::Close).await.unwrap();
    tokio::time::timeout(EVENT_WAIT, async { while events.recv().await.is_some() {} })
        .await
        .expect("the engine and its review desk shut down on close");

    let (_resumed, mut resumed_events) =
        Engine::spawn(review_engine_config(project_root, session_id));
    let restored_review = first(&mut resumed_events, "the restored review", |event| match event {
        Event::GuidedReviewLoaded { review, .. } => Some(review),
        _ => None,
    })
    .await;
    assert_eq!(restored_review.id, review.id);
    let restored = first(&mut resumed_events, "the restored session", |event| match event {
        Event::ReviewSessionRestored { session } => Some(session),
        _ => None,
    })
    .await;
    assert_eq!(*restored, *finished, "the comment, its turn and its report all came back");
}

/// A comment saved just before the front end closes is on disk by the time the engine has stopped.
///
/// The desk takes edits as they arrive, so `Close` can reach the engine while the write is still
/// queued; a front end awaits the engine's task and then exits, which drops anything unfinished.
/// Shutdown waits for the desk, so the last thing the reader wrote is not the thing they lose.
#[tokio::test]
async fn a_comment_saved_just_before_close_is_on_disk_when_the_engine_stops() {
    use luxe_core::guided_review::{ReviewSessionSaveRequestId, ReviewSessionStore};
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
    let session_id = JsonlSessionStore::new(project_root.to_path_buf())
        .create(&project_root.display().to_string())
        .unwrap();

    let (actions, mut events, task) =
        Engine::spawn_task(review_engine_config(project_root, session_id));
    let request_id = ReviewLoadRequestId::new();
    actions
        .send(Action::LoadGuidedReview {
            request_id: request_id.clone(),
            target: ReviewTarget::WorkingTree,
        })
        .await
        .unwrap();
    let review = first(&mut events, "the review", |event| match event {
        Event::GuidedReviewLoaded { request_id: loaded, review } if loaded == request_id => {
            Some(review)
        }
        _ => None,
    })
    .await;
    actions
        .send(Action::EditReview {
            request_id: ReviewSessionSaveRequestId::new(),
            review_id: review.id.clone(),
            edit: luxe_core::engine::ReviewEdit::Comment {
                draft: luxe_core::engine::CommentDraft {
                    anchor: FindingAnchor::Review,
                    severity: FindingSeverity::Issue,
                    title: "Written last".into(),
                    body: "Saved a moment before quitting.".into(),
                    proposed_fix: None,
                },
                send: false,
            },
        })
        .await
        .unwrap();
    actions.send(Action::Close).await.unwrap();
    // Drain so the desk is never stuck behind a full event channel, then wait for the engine
    let drained = tokio::spawn(async move { while events.recv().await.is_some() {} });
    tokio::time::timeout(EVENT_WAIT, task).await.expect("the engine stops").unwrap();

    let stored = ReviewSessionStore::new(project_root).load(&review.id).unwrap();
    let titles: Vec<String> =
        stored.iter().flat_map(|session| &session.findings).map(|f| f.title.clone()).collect();
    assert_eq!(titles, ["Written last"], "the last comment survived the close");
    drained.abort();
}

/// A provider whose every answer waits until it is cancelled: a Guide pass that would otherwise
/// run for minutes on a large review.
struct UntilCancelled;

#[async_trait::async_trait]
impl Provider for UntilCancelled {
    fn name(&self) -> &str {
        "until-cancelled"
    }

    async fn stream(
        &self,
        _request: CompletionRequest,
        mut cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        Ok(Box::pin(stream::once(async move {
            while !*cancel.borrow() {
                if cancel.changed().await.is_err() {
                    break;
                }
            }
            ProviderEvent::Done { reason: StopReason::EndTurn }
        })))
    }
}

/// Closing the engine ends the event stream promptly, even with a Guide pass still being written.
///
/// A front end waits for the stream to end before it exits, and every Guide pass holds a sender:
/// `luxe --rpc` given `close` mid-guide on a large review sat there for minutes. Shutdown cancels
/// the pass instead of waiting it out.
#[tokio::test]
async fn closing_does_not_wait_for_a_guide_being_written() {
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
    let session_id = JsonlSessionStore::new(project_root.to_path_buf())
        .create(&project_root.display().to_string())
        .unwrap();
    let mut config = review_engine_config(project_root, session_id);
    config.provider = Arc::new(UntilCancelled);
    config.model = luxe_core::provider::ModelInfo {
        r#ref: luxe_core::provider::ModelRef::new("until-cancelled", "slow"),
        ..Default::default()
    };
    let (actions, mut events) = Engine::spawn(config);
    let request_id = ReviewLoadRequestId::new();
    actions
        .send(Action::LoadGuidedReview {
            request_id: request_id.clone(),
            target: ReviewTarget::WorkingTree,
        })
        .await
        .unwrap();
    first(&mut events, "the review", |event| match event {
        Event::GuidedReviewLoaded { request_id: loaded, .. } if loaded == request_id => Some(()),
        _ => None,
    })
    .await;

    actions.send(Action::Close).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while events.recv().await.is_some() {}
    })
    .await
    .expect("the stream ends on close, without waiting for the guide");
}
