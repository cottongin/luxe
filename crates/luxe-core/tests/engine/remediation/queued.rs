//! Queued — feedback that arrives behind an active turn is cancelled or dispatched, never lost.

use std::sync::Arc;

use futures::stream::{self, BoxStream};
use luxe_core::engine::{Action, Engine};
use luxe_core::event::Event;
use luxe_core::provider::{CompletionRequest, Provider, ProviderError, ProviderEvent, StopReason};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use tokio::sync::{Mutex, Notify};

use crate::fixtures::*;

struct BlockingFirstProvider {
    requests: Arc<Mutex<Vec<CompletionRequest>>>,
    started: Arc<Notify>,
    release: Arc<Notify>,
    calls: Mutex<usize>,
}

#[async_trait::async_trait]
impl Provider for BlockingFirstProvider {
    fn name(&self) -> &str {
        "blocking-first"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        self.requests.lock().await.push(request);
        let first_call = {
            let mut calls = self.calls.lock().await;
            *calls += 1;
            *calls == 1
        };
        if first_call {
            self.started.notify_one();
            let release = self.release.clone();
            return Ok(Box::pin(stream::once(async move {
                release.notified().await;
                ProviderEvent::Done { reason: StopReason::EndTurn }
            })));
        }
        Ok(Box::pin(stream::iter(vec![ProviderEvent::Done { reason: StopReason::EndTurn }])))
    }
}

#[tokio::test]
async fn p32_cancels_feedback_queued_behind_an_active_turn() {
    use luxe_core::guided_review::{
        FindingAnchor, FindingId, FindingSeverity, ReviewFeedbackMode, ReviewId, SubmissionId,
        SubmittedFinding,
    };

    let tmp = tempfile::tempdir().unwrap();
    let sessions_root = tmp.path().join("sessions");
    let mut store = JsonlSessionStore::new(&sessions_root);
    let session_id = store.create("/tmp/test").unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let config = base_config(
        store,
        session_id.clone(),
        Arc::new(BlockingFirstProvider {
            requests: requests.clone(),
            started: started.clone(),
            release: release.clone(),
            calls: Mutex::new(0),
        }),
    );
    let (action_tx, mut event_rx) = Engine::spawn(config);
    assert!(matches!(event_rx.recv().await, Some(Event::SessionLoaded { .. })));

    action_tx
        .send(Action::SubmitUserMessage {
            content: "Keep the agent busy.".into(),
            images: Vec::new(),
            remediation: None,
        })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), started.notified())
        .await
        .expect("initial provider turn must start");

    let submission_id = SubmissionId::new();
    let action = Action::SubmitReviewFeedback {
        submission_id: submission_id.clone(),
        review_id: ReviewId::new(),
        source: review_source(),
        mode: ReviewFeedbackMode::RequestFixes,
        findings: vec![SubmittedFinding {
            finding_id: FindingId::new(),
            label: "R1".into(),
            evidence: Default::default(),
            anchor: FindingAnchor::Review,
            severity: FindingSeverity::Issue,
            title: "Cancel me".into(),
            body: "This feedback must not run.".into(),
            proposed_fix: None,
            thread: Vec::new(),
            github_author: None,
        }],
    };
    action_tx.send(action).await.unwrap();
    action_tx
        .send(Action::CancelQueuedReviewFeedback { submission_id: submission_id.clone() })
        .await
        .unwrap();
    release.notify_one();

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match tokio::time::timeout(std::time::Duration::from_millis(100), event_rx.recv()).await {
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) | Err(_) => {}
            Ok(None) => panic!("engine stopped before the active turn ended"),
        }
        assert!(tokio::time::Instant::now() < deadline, "active turn did not finish in time");
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_eq!(requests.lock().await.len(), 1, "cancelled feedback must not reach the provider");
    let persisted = JsonlSessionStore::new(sessions_root);
    let marker = format!("Submission: {}", submission_id.as_str());
    let feedback_entries = persisted
        .load(&session_id)
        .unwrap()
        .iter()
        .filter(|entry| {
            matches!(&entry.kind, luxe_core::session::EntryKind::Message(message)
            if message.role == luxe_core::message::Role::User
                && message.content.iter().any(|block| matches!(block,
                    luxe_core::message::ContentBlock::Text(text)
                    if text.lines().any(|line| line == marker))))
        })
        .count();
    assert_eq!(feedback_entries, 0, "cancelled feedback must not create a transcript entry");
}

#[tokio::test]
async fn p32_dispatches_feedback_queued_behind_an_active_turn() {
    use luxe_core::guided_review::{
        FindingAnchor, FindingId, FindingSeverity, ReviewFeedbackMode, ReviewId, SubmissionId,
        SubmittedFinding,
    };

    let tmp = tempfile::tempdir().unwrap();
    let sessions_root = tmp.path().join("sessions");
    let mut store = JsonlSessionStore::new(&sessions_root);
    let session_id = store.create("/tmp/test").unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let config = base_config(
        store,
        session_id.clone(),
        Arc::new(BlockingFirstProvider {
            requests: requests.clone(),
            started: started.clone(),
            release: release.clone(),
            calls: Mutex::new(0),
        }),
    );
    let (action_tx, mut event_rx) = Engine::spawn(config);
    assert!(matches!(event_rx.recv().await, Some(Event::SessionLoaded { .. })));

    action_tx
        .send(Action::SubmitUserMessage {
            content: "Keep the agent busy.".into(),
            images: Vec::new(),
            remediation: None,
        })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), started.notified())
        .await
        .expect("initial provider turn must start");

    let submission_id = SubmissionId::new();
    action_tx
        .send(Action::SubmitReviewFeedback {
            submission_id: submission_id.clone(),
            review_id: ReviewId::new(),
            source: review_source(),
            mode: ReviewFeedbackMode::RequestFixes,
            findings: vec![SubmittedFinding {
                finding_id: FindingId::new(),
                label: "R1".into(),
                evidence: Default::default(),
                anchor: FindingAnchor::Review,
                severity: FindingSeverity::Issue,
                title: "Dispatch me".into(),
                body: "This feedback must run at the next boundary.".into(),
                proposed_fix: None,
                thread: Vec::new(),
                github_author: None,
            }],
        })
        .await
        .unwrap();
    release.notify_one();

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut dispatched = false;
    loop {
        match tokio::time::timeout(std::time::Duration::from_millis(100), event_rx.recv()).await {
            Ok(Some(Event::ReviewFeedbackTurnDispatched { submission_id: boundary })) => {
                assert_eq!(boundary, submission_id);
                dispatched = true;
            }
            Ok(Some(Event::ReviewFeedbackTurnCompleted { submission_id: completed })) => {
                assert_eq!(completed, submission_id);
                break;
            }
            Ok(Some(_)) | Err(_) => {}
            Ok(None) => panic!("engine stopped before dispatching queued feedback"),
        }
        assert!(tokio::time::Instant::now() < deadline, "queued feedback did not dispatch in time");
    }

    assert!(dispatched, "feedback completion must follow its dispatch-boundary event");
    assert_eq!(requests.lock().await.len(), 2, "feedback must dispatch after the active turn");
    let persisted = JsonlSessionStore::new(sessions_root);
    let marker = format!("Submission: {}", submission_id.as_str());
    let feedback_entries = persisted
        .load(&session_id)
        .unwrap()
        .iter()
        .filter(|entry| {
            matches!(&entry.kind, luxe_core::session::EntryKind::Message(message)
            if message.role == luxe_core::message::Role::User
                && message.content.iter().any(|block| matches!(block,
                    luxe_core::message::ContentBlock::Text(text)
                    if text.lines().any(|line| line == marker))))
        })
        .count();
    assert_eq!(feedback_entries, 1, "queued feedback must create one transcript entry");
}
