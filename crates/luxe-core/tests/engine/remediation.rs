//! Remediation — a request for fixes reaches the agent with its evidence, and a retry reuses it.

use crate::fixtures::*;
use std::sync::Arc;

use futures::stream::{self, BoxStream};
use luxe_core::engine::{Action, Engine};
use luxe_core::event::Event;
use luxe_core::provider::{CompletionRequest, Provider, ProviderError, ProviderEvent, StopReason};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use tokio::sync::Mutex;

struct FeedbackRemediationProvider {
    requests: Arc<Mutex<Vec<CompletionRequest>>>,
    reply: String,
    failures_remaining: Mutex<usize>,
}

#[async_trait::async_trait]
impl Provider for FeedbackRemediationProvider {
    fn name(&self) -> &str {
        "feedback-remediation"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        self.requests.lock().await.push(request);
        let mut failures_remaining = self.failures_remaining.lock().await;
        if *failures_remaining > 0 {
            *failures_remaining -= 1;
            return Err(ProviderError::Other("transient feedback failure".into()));
        }
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::TextDelta { index: 0, delta: self.reply.clone() },
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ])))
    }
}

#[tokio::test]
async fn p32_request_fixes_reaches_agent_with_immutable_evidence_and_remediation() {
    use luxe_core::guided_review::{
        FindingAnchor, FindingId, FindingSeverity, ReviewFeedbackMode, ReviewId, SubmissionId,
        SubmittedFinding,
    };

    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();
    let review_id = ReviewId::new();
    let source = review_source();
    let finding = SubmittedFinding {
        finding_id: FindingId::new(),
        label: "R1".into(),
        evidence: Default::default(),
        anchor: FindingAnchor::Review,
        severity: FindingSeverity::Issue,
        title: "Bound retries".into(),
        body: "Avoid an unbounded client wait.".into(),
        proposed_fix: Some("Return a timeout error.".into()),
        thread: Vec::new(),
        github_author: None,
    };
    let submission_id = SubmissionId::new();
    let reply = format!(
        "Fixed the timeout.\n```luxe-remediation\n{{\"review_id\":\"{}\",\"revision_id\":\"{}\",\"findings\":[{{\"finding_id\":\"{}\",\"status\":\"fixed\",\"note\":\"Added a bound.\"}}]}}\n```",
        review_id.as_str(),
        source.content_digest.as_str(),
        finding.finding_id.as_str(),
    );
    let requests = Arc::new(Mutex::new(Vec::new()));
    let config = base_config(
        store,
        session_id,
        Arc::new(FeedbackRemediationProvider {
            requests: requests.clone(),
            reply,
            failures_remaining: Mutex::new(0),
        }),
    );
    let (action_tx, mut event_rx) = Engine::spawn(config);
    assert!(matches!(event_rx.recv().await, Some(Event::SessionLoaded { .. })));

    let feedback_action = Action::SubmitReviewFeedback {
        submission_id: submission_id.clone(),
        review_id: review_id.clone(),
        source: source.clone(),
        mode: ReviewFeedbackMode::RequestFixes,
        findings: vec![finding.clone()],
    };
    action_tx.send(feedback_action.clone()).await.unwrap();
    action_tx.send(feedback_action).await.unwrap();

    let mut assistant_committed = false;
    let mut remediation_reported = false;
    let mut feedback_completed = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(200), event_rx.recv()).await {
            Ok(Some(Event::MessageCommitted { message, .. }))
                if message.role == luxe_core::message::Role::Assistant =>
            {
                assistant_committed = true;
            }
            Ok(Some(Event::RemediationReported { report, .. })) => {
                assert!(assistant_committed, "evidence follows the committed assistant message");
                assert_eq!(report.review_id, review_id);
                assert_eq!(report.revision_id, source.content_digest);
                assert_eq!(report.findings[0].finding_id, finding.finding_id);
                remediation_reported = true;
            }
            Ok(Some(Event::ReviewFeedbackTurnCompleted { submission_id: completed })) => {
                assert_eq!(completed, submission_id);
                feedback_completed = true;
                break;
            }
            Ok(Some(_)) | Err(_) => {}
            Ok(None) => break,
        }
    }

    assert!(remediation_reported, "agent must report bounded remediation evidence");
    assert!(feedback_completed, "feedback turn must complete with its submission ID");

    let requests = requests.lock().await;
    assert_eq!(
        requests.len(),
        1,
        "duplicate immutable submissions must still start only one agent turn"
    );
    let prompt = requests[0]
        .messages
        .iter()
        .find_map(|message| match (&message.role, message.content.first()) {
            (
                luxe_core::message::Role::User,
                Some(luxe_core::message::ContentBlock::Text(text)),
            ) => Some(text),
            _ => None,
        })
        .expect("structured feedback user message");
    assert!(prompt.contains(review_id.as_str()));
    assert!(prompt.contains(source.content_digest.as_str()));
    assert!(prompt.contains(finding.finding_id.as_str()));
    assert!(prompt.contains("Do not rewrite unrelated code."));
}

#[tokio::test]
async fn p32_retry_reuses_one_feedback_transcript_entry() {
    use luxe_core::guided_review::{
        FindingAnchor, FindingId, FindingSeverity, ReviewFeedbackMode, ReviewId, SubmissionId,
        SubmittedFinding,
    };

    let tmp = tempfile::tempdir().unwrap();
    let sessions_root = tmp.path().join("sessions");
    let mut store = JsonlSessionStore::new(&sessions_root);
    let session_id = store.create("/tmp/test").unwrap();
    let review_id = ReviewId::new();
    let source = review_source();
    let finding = SubmittedFinding {
        finding_id: FindingId::new(),
        label: "R1".into(),
        evidence: Default::default(),
        anchor: FindingAnchor::Review,
        severity: FindingSeverity::Issue,
        title: "Bound retries".into(),
        body: "Avoid an unbounded client wait.".into(),
        proposed_fix: None,
        thread: Vec::new(),
        github_author: None,
    };
    let submission_id = SubmissionId::new();
    let requests = Arc::new(Mutex::new(Vec::new()));
    // a compliant agent, so the only extra call is the retry itself
    let reply = format!(
        "Retry completed.\n```luxe-remediation\n{{\"review_id\":\"{}\",\"revision_id\":\"{}\",\"findings\":[{{\"finding_id\":\"{}\",\"status\":\"fixed\"}}]}}\n```",
        review_id.as_str(),
        source.content_digest.as_str(),
        finding.finding_id.as_str(),
    );
    let config = base_config(
        store,
        session_id.clone(),
        Arc::new(FeedbackRemediationProvider {
            requests: requests.clone(),
            reply,
            failures_remaining: Mutex::new(1),
        }),
    );
    let (action_tx, mut event_rx) = Engine::spawn(config);
    assert!(matches!(event_rx.recv().await, Some(Event::SessionLoaded { .. })));

    let action = Action::SubmitReviewFeedback {
        submission_id: submission_id.clone(),
        review_id: review_id.clone(),
        source: source.clone(),
        mode: ReviewFeedbackMode::RequestFixes,
        findings: vec![finding],
    };
    action_tx.send(action.clone()).await.unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match tokio::time::timeout(std::time::Duration::from_millis(200), event_rx.recv()).await {
            Ok(Some(Event::ReviewFeedbackTurnFailed { submission_id: failed, .. })) => {
                assert_eq!(failed, submission_id);
                break;
            }
            Ok(Some(_)) | Err(_) => {}
            Ok(None) => panic!("engine stopped before the transient failure"),
        }
        assert!(tokio::time::Instant::now() < deadline, "feedback did not fail in time");
    }

    action_tx.send(action).await.unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match tokio::time::timeout(std::time::Duration::from_millis(200), event_rx.recv()).await {
            Ok(Some(Event::ReviewFeedbackTurnCompleted { submission_id: completed })) => {
                assert_eq!(completed, submission_id);
                break;
            }
            Ok(Some(_)) | Err(_) => {}
            Ok(None) => panic!("engine stopped before completing retry"),
        }
        assert!(tokio::time::Instant::now() < deadline, "feedback retry did not finish in time");
    }

    let persisted = JsonlSessionStore::new(sessions_root);
    let marker = format!("Submission: {}", submission_id.as_str());
    let entries = persisted.load(&session_id).unwrap();
    let feedback_entries = entries
        .iter()
        .filter(|entry| {
            matches!(&entry.kind, luxe_core::session::EntryKind::Message(message)
            if message.role == luxe_core::message::Role::User
                && message.content.iter().any(|block| matches!(block,
                    luxe_core::message::ContentBlock::Text(text)
                    if text.lines().any(|line| line == marker))))
        })
        .count();
    assert_eq!(feedback_entries, 1, "same submission must reuse one transcript entry");
    assert_eq!(requests.lock().await.len(), 2, "retry must reach the provider once more");
}

mod queued;
mod reports;
