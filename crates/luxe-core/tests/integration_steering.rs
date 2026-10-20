//! Steering: a message sent while a turn is running reaches the model *inside* that turn — and the
//! read-only glances that must not wait for it.
//!
//! The engine buffers `Action::SteerTurn` and promotes it at the checkpoint between tool
//! iterations, so the next provider call carries it as a real user message — no second turn, no
//! silently re-queued action.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use futures::stream;
use futures::stream::BoxStream;
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::message::{ContentBlock, Role};
use luxe_core::provider::{
    CompletionRequest, ModelInfo, Provider, ProviderError, ProviderEvent, StopReason,
};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use luxe_core::tool::{Tool, ToolContext, ToolRegistry, ToolResultData};
use luxe_core::HookRegistry;
use tokio::sync::Mutex;

/// A tool that blocks until the test releases it, so a steer can land mid-turn deterministically.
struct GateTool {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl Tool for GateTool {
    fn name(&self) -> &str {
        "gate"
    }

    fn schema(&self) -> luxe_core::provider::ToolSchema {
        luxe_core::provider::ToolSchema {
            name: "gate".to_string(),
            description: "Block until released.".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }

    async fn run(&self, _args: serde_json::Value, _cx: &ToolContext) -> ToolResultData {
        self.entered.notify_one();
        self.release.notified().await;
        ToolResultData { content: vec![ContentBlock::Text("gate open".into())], is_error: false }
    }
}

/// Call 1 asks for the gate tool; call 2 answers with whatever user text it can see.
struct ScriptedProvider {
    calls: Arc<Mutex<Vec<CompletionRequest>>>,
}

#[async_trait::async_trait]
impl Provider for ScriptedProvider {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn stream(
        &self,
        req: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let call_index = {
            let mut calls = self.calls.lock().await;
            calls.push(req.clone());
            calls.len()
        };
        let events = if call_index == 1 {
            vec![
                ProviderEvent::ToolCallStart {
                    index: 0,
                    id: "call_1".to_string(),
                    name: "gate".to_string(),
                },
                ProviderEvent::ToolCallDelta { index: 0, json_delta: "{}".to_string() },
                ProviderEvent::ToolCallEnd { index: 0 },
                ProviderEvent::Done { reason: StopReason::ToolCall },
            ]
        } else {
            vec![
                ProviderEvent::TextStart { index: 0 },
                ProviderEvent::TextDelta { index: 0, delta: "acknowledged".to_string() },
                ProviderEvent::TextEnd { index: 0 },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }
}

fn engine_config(
    store: JsonlSessionStore,
    session_id: luxe_core::session::SessionId,
    provider: Arc<dyn Provider + Send + Sync>,
    gate: GateTool,
) -> EngineConfig {
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(gate));
    EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider,
        providers: std::collections::HashMap::new(),
        model: ModelInfo::default(),
        tool_registry: tools,
        hook_registry: HookRegistry::new(),
        session_id,
        system_prompt: None,
        todo_store: None,
        compaction_settings: luxe_core::compaction::CompactionSettings::default(),
        permission: Default::default(),
        modes: Default::default(),
        reviewer: None,
        formatter_registry: None,
        spend_sink: None,
        guided_review_project_root: Some(std::path::PathBuf::from("/nonexistent-for-this-test")),
        invoked_from: None,
        external_work: None,
        review_context_files: Vec::new(),
        review_exclude: Vec::new(),
        remember_choices_in: None,
        cache_warming: luxe_core::cache_warming::CacheWarming::Off,
        review_insight_provider: None,
        github: None,
        permissions_file: None,
        max_tool_iterations: luxe_core::engine::DEFAULT_MAX_TOOL_ITERATIONS,
        startup_notices: Vec::new(),
        action_tx: None,
    }
}

#[tokio::test]
async fn a_steer_sent_mid_turn_reaches_the_model_in_that_same_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(ScriptedProvider { calls: calls.clone() });
    let gate = GateTool { entered: entered.clone(), release: release.clone() };

    let (action_tx, mut event_rx) = Engine::spawn(engine_config(store, session_id, provider, gate));

    action_tx
        .send(Action::SubmitUserMessage {
            content: "start the work".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    // Steer while the tool is still running — the turn is genuinely in flight.
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified()).await.unwrap();
    action_tx
        .send(Action::SteerTurn { content: "also check the tests".to_string() })
        .await
        .unwrap();
    // Give the engine a moment to admit the steer, then let the tool finish.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    release.notify_one();

    let mut delivered = 0usize;
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), event_rx.recv()).await {
            Ok(Some(Event::SteeringDelivered { count })) => delivered += count,
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out waiting for the turn to end"),
        }
    }

    assert_eq!(delivered, 1, "the steer is announced as delivered exactly once");
    let calls = calls.lock().await;
    assert_eq!(calls.len(), 2, "one tool round-trip, then the follow-up call");
    let saw_steer = calls[1].messages.iter().any(|message| {
        message.role == Role::User
            && message.content.iter().any(
                |block| matches!(block, ContentBlock::Text(text) if text == "also check the tests"),
            )
    });
    assert!(saw_steer, "the second provider call must carry the steering message");

    let _ = action_tx.send(Action::Close).await;
}

/// A withdrawn steer never reaches the model, even though the engine already had it.
///
/// The half a front-end cannot do itself: a steer is admitted the instant it is submitted — that is
/// what makes it arrive at the next checkpoint rather than after the turn — so a UI clearing its own
/// tray would leave the recalled text queued to arrive anyway, under a composer now showing it as an
/// editable draft. The turn has to carry on regardless: withdrawing a steer is not cancelling.
#[tokio::test]
async fn a_withdrawn_steer_never_reaches_the_model() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(ScriptedProvider { calls: calls.clone() });
    let gate = GateTool { entered: entered.clone(), release: release.clone() };

    let (action_tx, mut event_rx) = Engine::spawn(engine_config(store, session_id, provider, gate));

    action_tx
        .send(Action::SubmitUserMessage {
            content: "start the work".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified()).await.unwrap();
    action_tx
        .send(Action::SteerTurn { content: "also check the tests".to_string() })
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    // Second thoughts, before the checkpoint that would have delivered it.
    action_tx.send(Action::WithdrawSteering).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    release.notify_one();

    let mut delivered = 0usize;
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), event_rx.recv()).await {
            Ok(Some(Event::SteeringDelivered { count })) => delivered += count,
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out waiting for the turn to end"),
        }
    }

    assert_eq!(delivered, 0, "nothing was delivered");
    let calls = calls.lock().await;
    assert_eq!(calls.len(), 2, "the turn ran to completion — a withdrawal is not a cancellation");
    let saw_steer = calls[1].messages.iter().any(|message| {
        message.role == Role::User
            && message.content.iter().any(
                |block| matches!(block, ContentBlock::Text(text) if text == "also check the tests"),
            )
    });
    assert!(!saw_steer, "the withdrawn steer is absent from the request: {:?}", calls[1].messages);

    let _ = action_tx.send(Action::Close).await;
}

#[tokio::test]
async fn a_steer_with_no_turn_running_just_starts_one() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(ScriptedProvider { calls: calls.clone() });
    // Nothing will call the gate here: the scripted provider's first call requests it, so release
    // it immediately to keep the turn moving.
    let release = Arc::new(tokio::sync::Notify::new());
    let gate = GateTool { entered: Arc::new(tokio::sync::Notify::new()), release: release.clone() };
    let releaser = release.clone();
    tokio::spawn(async move {
        loop {
            releaser.notify_one();
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    });

    let (action_tx, mut event_rx) = Engine::spawn(engine_config(store, session_id, provider, gate));
    action_tx.send(Action::SteerTurn { content: "just do it".to_string() }).await.unwrap();

    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), event_rx.recv()).await {
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("a steer with no turn in flight should still run"),
        }
    }

    let calls = calls.lock().await;
    let saw_message = calls[0].messages.iter().any(|message| {
        message.role == Role::User
            && message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::Text(text) if text == "just do it"))
    });
    assert!(saw_message, "the steer became an ordinary user turn");

    let _ = action_tx.send(Action::Close).await;
}

/// A provider whose calls are all plain text: the turn would end after the first one.
///
/// The nth call's request is recorded so a test can inspect the system prompt it was given, and a
/// notifier fires as each call starts so a steer can be timed against it.
struct TextOnlyProvider {
    calls: Arc<Mutex<Vec<CompletionRequest>>>,
    started: Arc<tokio::sync::Notify>,
    release_first: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl Provider for TextOnlyProvider {
    fn name(&self) -> &str {
        "text-only"
    }

    async fn stream(
        &self,
        req: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let first = {
            let mut calls = self.calls.lock().await;
            calls.push(req);
            calls.len() == 1
        };
        self.started.notify_one();
        // The first call is held open so a steer can land while it is genuinely in flight.
        if first {
            self.release_first.notified().await;
        }
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::TextStart { index: 0 },
            ProviderEvent::TextDelta { index: 0, delta: "continuing with that now".to_string() },
            ProviderEvent::TextEnd { index: 0 },
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ])))
    }
}

/// A steer that lands while the *final* response is streaming must still be answered in this turn.
///
/// The delivery checkpoint only runs at the top of a tool iteration, so a turn that ends on a
/// text-only response used to leave the steer buffered until the user's next submission — a message
/// they watched luxe accept and then apparently ignore.
#[tokio::test]
async fn a_steer_that_lands_during_the_final_response_is_not_stranded() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let started = Arc::new(tokio::sync::Notify::new());
    let release_first = Arc::new(tokio::sync::Notify::new());
    let provider = Arc::new(TextOnlyProvider {
        calls: calls.clone(),
        started: started.clone(),
        release_first: release_first.clone(),
    });
    let gate = GateTool {
        entered: Arc::new(tokio::sync::Notify::new()),
        release: Arc::new(tokio::sync::Notify::new()),
    };

    let (action_tx, mut event_rx) = Engine::spawn(engine_config(store, session_id, provider, gate));
    action_tx
        .send(Action::SubmitUserMessage {
            content: "start the work".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    // Steer while the first (and, without this fix, only) call is still in flight.
    tokio::time::timeout(std::time::Duration::from_secs(5), started.notified()).await.unwrap();
    action_tx
        .send(Action::SteerTurn { content: "wait — check the tests".to_string() })
        .await
        .unwrap();
    // Let the turn's action loop buffer it, then allow the response to complete.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    release_first.notify_one();

    let mut delivered = 0usize;
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), event_rx.recv()).await {
            Ok(Some(Event::SteeringDelivered { count })) => delivered += count,
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out waiting for the turn to end"),
        }
    }

    assert_eq!(delivered, 1, "the steer was delivered inside the turn it was aimed at");
    let calls = calls.lock().await;
    assert!(calls.len() >= 2, "the turn continued for the steer, got {} call(s)", calls.len());
    let saw_steer = calls[calls.len() - 1].messages.iter().any(|message| {
        message.role == Role::User
            && message.content.iter().any(
                |block| matches!(block, ContentBlock::Text(text) if text == "wait — check the tests"),
            )
    });
    assert!(saw_steer, "the extra call carries the steering message");

    let _ = action_tx.send(Action::Close).await;
}

/// The call that follows a mid-task steer is framed as an interruption, so the model answers it and
/// picks the work back up instead of ending the turn on an acknowledgement.
#[tokio::test]
async fn the_call_after_a_mid_task_steer_is_told_to_resume_the_work() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(ScriptedProvider { calls: calls.clone() });
    let gate = GateTool { entered: entered.clone(), release: release.clone() };

    let (action_tx, mut event_rx) = Engine::spawn(engine_config(store, session_id, provider, gate));
    action_tx
        .send(Action::SubmitUserMessage {
            content: "start the work".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified()).await.unwrap();
    action_tx
        .send(Action::SteerTurn { content: "also check the tests".to_string() })
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    release.notify_one();

    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), event_rx.recv()).await {
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out waiting for the turn to end"),
        }
    }

    let calls = calls.lock().await;
    let tail_text = |request: &CompletionRequest| -> String {
        let last = request.messages.last().expect("a request carries a conversation");
        assert_eq!(last.role, Role::User);
        last.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(
        !tail_text(&calls[0]).contains("MID-TURN STEER"),
        "the opening call is not framed as an interruption"
    );
    let framed = tail_text(&calls[1]);
    assert!(framed.contains("also check the tests"), "the steer itself is there: {framed:?}");
    assert!(framed.contains("MID-TURN STEER"), "the post-steer call is framed: {framed:?}");
    assert!(
        framed.contains("ends your turn"),
        "and says why ending on an acknowledgement strands the task: {framed:?}"
    );
    // The framing must not touch the system prompt. Every cached message sits behind it, so one
    // byte of difference there re-bills the whole conversation as a cache write.
    assert_eq!(calls[0].system, calls[1].system, "the system prompt holds still across a steer");

    let _ = action_tx.send(Action::Close).await;
}

/// A peek at one file's diff is answered *during* the turn, not after it.
///
/// `Action::LoadFileDiff` used to fall into the turn loop's catch-all and be re-queued for
/// afterwards, so `Ctrl+P` mid-turn drew the file list and then sat on "Reading the change…" for
/// however long the agent kept working — over a diff that was on disk the whole time. It reads
/// nothing the turn owns and returns immediately, so there was never a reason to make it wait.
#[tokio::test]
async fn a_file_diff_asked_for_mid_turn_is_answered_mid_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(ScriptedProvider { calls: calls.clone() });
    let gate = GateTool { entered: entered.clone(), release: release.clone() };

    let (action_tx, mut event_rx) = Engine::spawn(engine_config(store, session_id, provider, gate));
    action_tx
        .send(Action::SubmitUserMessage {
            content: "start the work".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    // The tool is blocked, so the turn is genuinely in flight for everything below.
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified()).await.unwrap();
    let request_id = luxe_core::file_diff::FileDiffRequestId::new();
    action_tx
        .send(Action::LoadFileDiff {
            request_id: request_id.clone(),
            path: "src/app/multiselect.rs".to_string(),
        })
        .await
        .unwrap();

    // An answer either way is the point — this project root does not exist, so the honest answer is
    // a failure. What matters is that it arrives while the tool is still blocking the turn.
    let answered = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match event_rx.recv().await {
                Some(Event::FileDiffLoaded { request_id: id, .. })
                | Some(Event::FileDiffFailed { request_id: id, .. }) => {
                    if id == request_id {
                        return true;
                    }
                }
                Some(Event::TurnEnded { .. }) => return false,
                Some(_) => {}
                None => return false,
            }
        }
    })
    .await
    .expect("the peek request was never answered at all");

    assert!(answered, "the diff must not wait for the turn it was asked during");
    release.notify_one();
    let _ = action_tx.send(Action::Close).await;
}

/// A review edit sent while the agent works is answered while it works.
///
/// It used to wait for the turn: the save was deferred like any other action, and since the
/// dispatch of a fix starts the turn before its own save could land, every edit for the length of
/// the agent's work on the last batch was refused as "still saving". No review is open here, so the
/// honest answer is a refusal; what matters is that it arrives while the tool still blocks the turn.
#[tokio::test]
async fn a_review_edit_sent_mid_turn_is_answered_mid_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(ScriptedProvider { calls: calls.clone() });
    let gate = GateTool { entered: entered.clone(), release: release.clone() };

    let (action_tx, mut event_rx) = Engine::spawn(engine_config(store, session_id, provider, gate));
    action_tx
        .send(Action::SubmitUserMessage {
            content: "start the work".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified()).await.unwrap();

    let request_id = luxe_core::guided_review::ReviewSessionSaveRequestId::new();
    action_tx
        .send(Action::EditReview {
            request_id: request_id.clone(),
            review_id: luxe_core::guided_review::ReviewId::new(),
            edit: luxe_core::engine::ReviewEdit::SetOutcome {
                outcome: luxe_core::guided_review::ReviewOutcome::Accepted,
            },
        })
        .await
        .unwrap();
    let answered = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match event_rx.recv().await {
                Some(Event::ReviewSessionSaved { request_id: id, .. })
                | Some(Event::ReviewSessionSaveFailed { request_id: id, .. })
                    if id == request_id =>
                {
                    return true;
                }
                Some(Event::TurnEnded { .. }) | None => return false,
                Some(_) => {}
            }
        }
    })
    .await
    .expect("the review edit was never answered at all");

    assert!(answered, "a review edit must not wait for the turn it was sent during");
    release.notify_one();
    let _ = action_tx.send(Action::Close).await;
}

#[path = "integration_steering/prompt_hook.rs"]
mod prompt_hook;
