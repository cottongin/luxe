//! Reports — what the agent says about each comment, and what is said when it says nothing.

use std::process::Command;
use std::sync::Arc;

use futures::stream::{self, BoxStream};
use luxe_core::engine::{Action, Engine};
use luxe_core::event::Event;
use luxe_core::guided_review::{FindingAnchor, FindingSeverity, ReviewLoadRequestId, ReviewTarget};
use luxe_core::provider::{CompletionRequest, Provider, ProviderError, ProviderEvent, StopReason};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use tokio::sync::Mutex;

use crate::fixtures::*;

struct ToolIntermediateRemediationProvider {
    intermediate: String,
    calls: Mutex<usize>,
}

#[async_trait::async_trait]
impl Provider for ToolIntermediateRemediationProvider {
    fn name(&self) -> &str {
        "tool-intermediate-remediation"
    }

    async fn stream(
        &self,
        _request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let first_call = {
            let mut calls = self.calls.lock().await;
            *calls += 1;
            *calls == 1
        };
        let events = if first_call {
            vec![
                ProviderEvent::TextDelta { index: 0, delta: self.intermediate.clone() },
                ProviderEvent::ToolCallStart {
                    index: 0,
                    id: "call_1".into(),
                    name: "unknown".into(),
                },
                ProviderEvent::ToolCallDelta { index: 0, json_delta: "{}".into() },
                ProviderEvent::ToolCallEnd { index: 0 },
                ProviderEvent::Done { reason: StopReason::ToolCall },
            ]
        } else {
            vec![
                ProviderEvent::TextDelta {
                    index: 0,
                    delta: "Finished after the tool result.".into(),
                },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }
}

#[tokio::test]
async fn remediation_report_emits_after_its_assistant_message_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();
    let review_id = luxe_core::guided_review::ReviewId::new();
    let revision_id = luxe_core::guided_review::ReviewRevisionId::from_canonical_bytes(b"review");
    let finding_id = luxe_core::guided_review::FindingId::new();
    let reply = format!(
        "Addressed the retry path.\n```luxe-remediation\n{{\"review_id\":\"{}\",\"revision_id\":\"{}\",\"findings\":[{{\"finding_id\":\"{}\",\"status\":\"fixed\",\"note\":\"Added the missing bound.\"}}]}}\n```",
        review_id.as_str(),
        revision_id.as_str(),
        finding_id.as_str(),
    );
    let config = base_config(store, session_id, Arc::new(RemediationProvider { reply }));
    let (action_tx, mut event_rx) = Engine::spawn(config);
    assert!(matches!(event_rx.recv().await, Some(Event::SessionLoaded { .. })));

    action_tx
        .send(Action::SubmitUserMessage {
            content: "Please address R1.".into(),
            images: Vec::new(),
            remediation: Some(luxe_core::engine::RemediationTurnContext {
                submission_id: None,
                review_id: review_id.clone(),
                revision_id: revision_id.clone(),
                finding_ids: vec![finding_id.clone()],
                answers_only: false,
                reports: true,
            }),
        })
        .await
        .unwrap();

    let mut committed_entry = None;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(200), event_rx.recv()).await {
            Ok(Some(Event::MessageCommitted { entry_id, message, .. }))
                if message.role == luxe_core::message::Role::Assistant =>
            {
                committed_entry = Some(entry_id);
            }
            Ok(Some(Event::RemediationReported { entry_id, report })) => {
                assert_eq!(committed_entry.as_ref(), Some(&entry_id));
                assert_eq!(report.review_id, review_id);
                assert_eq!(report.revision_id, revision_id);
                assert_eq!(report.findings[0].finding_id, finding_id);
                return;
            }
            Ok(Some(_)) | Err(_) => {}
            Ok(None) => break,
        }
    }
    panic!("engine did not emit the committed remediation report");
}

/// A report written beside a tool call counts, and is read once, when the turn ends.
///
/// A model that reports and then runs one last check before a one-line sign-off used to lose its
/// report: only the final message was read.
#[tokio::test]
async fn a_report_beside_a_tool_call_is_read_when_the_turn_ends() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();
    let review_id = luxe_core::guided_review::ReviewId::new();
    let revision_id = luxe_core::guided_review::ReviewRevisionId::from_canonical_bytes(b"review");
    let finding_id = luxe_core::guided_review::FindingId::new();
    let intermediate = format!(
        "```luxe-remediation\n{{\"review_id\":\"{}\",\"revision_id\":\"{}\",\"findings\":[{{\"finding_id\":\"{}\",\"status\":\"fixed\"}}]}}\n```",
        review_id.as_str(),
        revision_id.as_str(),
        finding_id.as_str(),
    );
    let config = base_config(
        store,
        session_id,
        Arc::new(ToolIntermediateRemediationProvider { intermediate, calls: Mutex::new(0) }),
    );
    let (action_tx, mut event_rx) = Engine::spawn(config);
    assert!(matches!(event_rx.recv().await, Some(Event::SessionLoaded { .. })));
    action_tx
        .send(Action::SubmitUserMessage {
            content: "Please address R1.".into(),
            images: Vec::new(),
            remediation: Some(luxe_core::engine::RemediationTurnContext {
                submission_id: None,
                review_id,
                revision_id,
                finding_ids: vec![finding_id.clone()],
                answers_only: false,
                reports: true,
            }),
        })
        .await
        .unwrap();

    let mut last_reply = None;
    let mut reported = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(200), event_rx.recv()).await {
            Ok(Some(Event::MessageCommitted { entry_id, message, .. }))
                if message.role == luxe_core::message::Role::Assistant =>
            {
                last_reply = Some(entry_id);
            }
            Ok(Some(Event::RemediationReported { entry_id, report })) => {
                reported.push((entry_id, report));
            }
            Ok(Some(Event::TurnEnded { .. })) => {
                assert_eq!(reported.len(), 1, "read once, at the end");
                let (entry_id, report) = &reported[0];
                assert_eq!(Some(entry_id), last_reply.as_ref(), "against the reply that ended it");
                assert_eq!(report.findings[0].finding_id, finding_id);
                return;
            }
            Ok(Some(_)) | Err(_) => {}
            Ok(None) => break,
        }
    }
    panic!("engine did not complete the scripted tool turn");
}

/// A Guide like [`GuideManifestProvider`]'s, and fix turns answered from a script.
///
/// Every call a fix turn makes — the feedback itself, or Luxe asking for the report — takes the
/// next reply, with `{review}`, `{revision}`, `{first}` and `{second}` filled in from the feedback.
/// [`UNTIL_CANCELLED`] answers nothing until the turn is cancelled, and [`CUT_OFF`] stops partway
/// through a tool call's arguments, as a reply that hit its token limit does.
#[derive(Default)]
struct ScriptedFixes {
    guide: GuideManifestProvider,
    replies: std::sync::Mutex<std::collections::VecDeque<&'static str>>,
    /// The review, the revision, then each finding, as the feedback named them
    ids: std::sync::Mutex<Vec<String>>,
    /// What each fix-turn call was sent last
    asked: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl Provider for ScriptedFixes {
    fn name(&self) -> &str {
        "scripted-fixes"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let last = request.messages.last().and_then(|message| {
            message.content.iter().find_map(|block| match block {
                luxe_core::message::ContentBlock::Text(text) => Some(text.clone()),
                _ => None,
            })
        });
        let Some(text) = last.filter(|text| text.starts_with("Guided review ")) else {
            return self.guide.stream(request, cancel).await;
        };
        if text.starts_with("Guided review feedback") {
            let field = |prefix: &str| -> Vec<String> {
                text.lines()
                    .filter_map(|line| line.trim_start().strip_prefix(prefix).map(str::to_owned))
                    .collect()
            };
            let mut ids = field("Review: ");
            ids.extend(field("Source revision: "));
            ids.extend(field("Finding ID: "));
            *self.ids.lock().unwrap() = ids;
        }
        self.asked.lock().unwrap().push(text);
        let ids = self.ids.lock().unwrap().clone();
        let next = self.replies.lock().unwrap().pop_front();
        let next = next.expect("a scripted reply for every fix-turn call");
        if next == CUT_OFF {
            return Ok(Box::pin(stream::iter(vec![
                ProviderEvent::ToolCallStart {
                    index: 0,
                    id: "call_1".into(),
                    name: "write".into(),
                },
                ProviderEvent::ToolCallDelta { index: 0, json_delta: "{\"path\": \"stat".into() },
                ProviderEvent::Done { reason: StopReason::Length },
            ])));
        }
        if next == UNTIL_CANCELLED {
            let mut cancel = cancel;
            return Ok(Box::pin(stream::once(async move {
                while !*cancel.borrow() {
                    if cancel.changed().await.is_err() {
                        break;
                    }
                }
                ProviderEvent::Done { reason: StopReason::EndTurn }
            })));
        }
        let reply = next
            .replace("{review}", &ids[0])
            .replace("{revision}", &ids[1])
            .replace("{first}", &ids[2])
            .replace("{second}", &ids[3]);
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::TextDelta { index: 0, delta: reply },
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ])))
    }
}

/// A scripted reply that waits for the turn to be cancelled.
const UNTIL_CANCELLED: &str = "<until cancelled>";

/// A scripted reply cut off in the middle of a tool call.
const CUT_OFF: &str = "<cut off>";

/// Sends two comments to be fixed by an agent that answers with `replies`, and returns the
/// session once the turn is done, with what each of the turn's calls was sent. A reply of
/// [`UNTIL_CANCELLED`] is cancelled, the way a reader stops a turn, once the turn starts.
async fn fix_two_comments(
    replies: &[&'static str],
) -> (luxe_core::guided_review::ReviewSession, Vec<String>) {
    use luxe_core::guided_review::{
        ReviewFeedbackMode, ReviewSessionSaveRequestId, SubmissionDispatch,
    };
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
    let provider = Arc::new(ScriptedFixes {
        replies: std::sync::Mutex::new(replies.iter().copied().collect()),
        ..Default::default()
    });
    let mut config = review_engine_config(project_root, session_id);
    config.provider = provider.clone();
    let (actions, mut events) = Engine::spawn(config);

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
    let mut finding_ids = Vec::new();
    for title in ["Keep the function focused", "Name it for what it does"] {
        let request_id = ReviewSessionSaveRequestId::new();
        actions
            .send(Action::EditReview {
                request_id: request_id.clone(),
                review_id: review.id.clone(),
                edit: luxe_core::engine::ReviewEdit::Comment {
                    draft: luxe_core::engine::CommentDraft {
                        anchor: FindingAnchor::Review,
                        severity: FindingSeverity::Issue,
                        title: title.into(),
                        body: format!("{title}, please."),
                        proposed_fix: None,
                    },
                    send: false,
                },
            })
            .await
            .unwrap();
        let saved = first(&mut events, "the comment", |event| match event {
            Event::ReviewSessionSaved { request_id: saved, session } if saved == request_id => {
                Some(session)
            }
            _ => None,
        })
        .await;
        finding_ids.push(saved.findings.last().unwrap().id.clone());
    }
    actions
        .send(Action::EditReview {
            request_id: ReviewSessionSaveRequestId::new(),
            review_id: review.id.clone(),
            edit: luxe_core::engine::ReviewEdit::Send {
                finding_ids,
                mode: Some(ReviewFeedbackMode::RequestFixes),
                outcome: None,
            },
        })
        .await
        .unwrap();
    let stops = replies.contains(&UNTIL_CANCELLED);
    let finished = first(&mut events, "the finished fix turn", |event| match event {
        Event::ReviewFeedbackTurnDispatched { .. } if stops => {
            let actions = actions.clone();
            tokio::spawn(async move { actions.send(Action::Cancel).await });
            None
        }
        Event::ReviewSessionSaved { session, .. }
            if session.submissions.first().map(|s| s.dispatch)
                == Some(SubmissionDispatch::Completed) =>
        {
            Some(session)
        }
        _ => None,
    })
    .await;
    actions.send(Action::Close).await.unwrap();
    tokio::time::timeout(EVENT_WAIT, async { while events.recv().await.is_some() {} })
        .await
        .expect("the engine shuts down on close");
    let asked = provider.asked.lock().unwrap().clone();
    (*finished, asked)
}

/// What Luxe said on a comment's thread, in its own name.
fn luxe_notes(finding: &luxe_core::guided_review::ReviewFinding) -> Vec<&str> {
    use luxe_core::guided_review::FindingMessageAuthor;
    let notes = finding.thread.iter().filter(|m| m.author == FindingMessageAuthor::System);
    notes.map(|m| m.body.as_str()).collect()
}

/// A fix turn that ends without a report is asked for it once, and the answer counts — even in a
/// `json` fence, which is how a correct report used to be thrown away.
#[tokio::test]
async fn a_missing_report_is_asked_for_once_and_the_answer_counts() {
    use luxe_core::guided_review::FindingState;
    let (session, asked) = fix_two_comments(&[
        "Fixed both comments.",
        "```json\n{\"review_id\":\"{review}\",\"revision_id\":\"{revision}\",\"findings\":[\
         {\"finding_id\":\"{first}\",\"status\":\"fixed\",\"note\":\"Split it.\"},\
         {\"finding_id\":\"{second}\",\"status\":\"Fixed\",\"note\":\"Renamed it.\"}]}\n```",
    ])
    .await;
    assert_eq!(asked.len(), 2, "the feedback, then one ask");
    assert!(asked[1].starts_with("Guided review report\n"), "{}", asked[1]);
    assert!(asked[1].contains("it wrote no report"), "the ask says what was wrong: {}", asked[1]);
    assert!(asked[1].contains("```luxe-remediation\n"), "and carries the template: {}", asked[1]);
    assert!(!asked[1].contains("Submission: "), "a retry must not mistake it for the feedback");
    assert_eq!(luxe_core::briefs::label(&asked[1]), Some("Asked the agent for its report"));
    for (finding, note) in session.findings.iter().zip(["Split it.", "Renamed it."]) {
        assert_eq!(finding.state, FindingState::NeedsVerification, "{}", finding.title);
        assert!(finding.thread.iter().any(|m| m.body.contains(note)), "{:?}", finding.thread);
        assert!(luxe_notes(finding).is_empty(), "nothing went unreported: {:?}", finding.thread);
    }
}

/// An agent that cannot produce a usable report is asked once, not again and again, and then
/// each comment says so, and why, in Luxe's name; the comments stay where the reviewer left them.
#[tokio::test]
async fn an_agent_that_never_reports_leaves_word_on_each_comment() {
    use luxe_core::guided_review::FindingState;
    let (session, asked) = fix_two_comments(&[
        "Fixed both comments.",
        "```luxe-remediation\n{\"review_id\":\"{review}\",\"revision_id\":\"revision:stale\",\
         \"findings\":[{\"finding_id\":\"{first}\",\"status\":\"fixed\"}]}\n```",
    ])
    .await;
    assert_eq!(asked.len(), 2, "asked once, then the turn ends");
    for finding in &session.findings {
        assert_eq!(finding.state, FindingState::Open, "only the reviewer moves a comment");
        assert_eq!(
            luxe_notes(finding),
            ["No word from the agent on this comment: its report was for a different version of \
              the code. Refresh the review to see the code as it is now."],
            "{}",
            finding.title
        );
    }
}

/// A report that speaks for some of the comments counts for those, without an ask, and the one it
/// left out says so.
#[tokio::test]
async fn a_report_that_leaves_a_comment_out_says_so_on_that_comment() {
    use luxe_core::guided_review::FindingState;
    let (session, asked) = fix_two_comments(&[
        "Split it.\n```luxe-remediation\n{\"review_id\":\"{review}\",\"revision_id\":\"{revision}\",\
         \"findings\":[{\"finding_id\":\"{first}\",\"status\":\"fixed\"}]}\n```",
    ])
    .await;
    assert_eq!(asked.len(), 1, "a usable report is not asked for again");
    let (reported, left_out) = (&session.findings[0], &session.findings[1]);
    assert_eq!(reported.state, FindingState::NeedsVerification);
    assert!(luxe_notes(reported).is_empty());
    assert_eq!(left_out.state, FindingState::Open);
    assert_eq!(luxe_notes(left_out).len(), 1);
    assert!(luxe_notes(left_out)[0].contains("its report left this comment out"));
}

/// A reader who stops a fix turn is told, on each comment, that it stopped before the agent said
/// anything, not that the agent wrote no report; and a stopped turn is not asked for one.
#[tokio::test]
async fn a_stopped_fix_turn_says_so_on_each_comment() {
    let (session, asked) = fix_two_comments(&[UNTIL_CANCELLED]).await;
    assert_eq!(asked.len(), 1, "a stopped turn is not asked for its report");
    for finding in &session.findings {
        assert_eq!(luxe_notes(finding).len(), 1, "{:?}", finding.thread);
        assert!(luxe_notes(finding)[0].contains("the turn was stopped before it reported"));
    }
}

/// A fix turn whose reply was cut off says so on each comment, rather than blaming the agent for
/// writing no report; a turn that cannot go on is not asked for one.
#[tokio::test]
async fn a_cut_off_fix_turn_says_so_on_each_comment() {
    let (session, asked) = fix_two_comments(&[CUT_OFF]).await;
    assert_eq!(asked.len(), 1);
    for finding in &session.findings {
        assert_eq!(luxe_notes(finding).len(), 1, "{:?}", finding.thread);
        assert!(luxe_notes(finding)[0].contains("its reply was cut off before the report"));
    }
}
