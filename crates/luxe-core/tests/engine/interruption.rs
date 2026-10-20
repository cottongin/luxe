//! An interrupted turn must not brick the session.
//!
//! One real incident produced all of this: a model emitted three tool calls in one message, the first
//! wedged on a host that never answered, the user interrupted, and luxe quit. What it left behind was
//! an assistant message whose calls had no results — which the OpenAI Responses API answers with
//! `400 No tool output found for function call <id>` on **every** later request, and Anthropic with
//! the equivalent about `tool_use` ids. The session, and every resume of it, was over.
//!
//! A second incident, a year of sessions later, came through the other door and ended the same
//! way: the interrupt itself was handled correctly, and then the turn *after* it was born
//! cancelled — its stream dropped before a single event, committing an empty assistant message
//! that Anthropic answers with `400 text content blocks must be non-empty` forever after. The
//! reader saw the spinner stop, sent one more message, and lost the session to a 400 about a
//! message nobody had written.
//!
//! What is asserted here, in the order the incidents hit:
//!
//! 1. Cancel ends the *batch*, so the queued calls never run.
//! 2. Whatever happens, every call in the transcript has a result.
//! 3. A transcript that already has the gap is repaired on the way to the provider, so a session
//!    saved by an older build still resumes.
//! 4. The next turn does not inherit the cancel.
//! 5. A call that comes back with nothing commits nothing, and says so.
//! 6. A transcript that already carries an empty reply is scrubbed on the way out, same rule.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::fixtures::{
    await_cancel_taken, base_config, events_until_turn_end, pairing, result_text, EVENT_WAIT,
};
use futures::stream::{self, BoxStream};
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::message::{ContentBlock, Message, Role};
use luxe_core::provider::{
    CompletionRequest, ModelInfo, Provider, ProviderError, ProviderEvent, StopReason, ToolSchema,
    Usage,
};
use luxe_core::session::{EntryKind, JsonlSessionStore, SessionId, SessionStore};
use luxe_core::tool::{Tool, ToolContext, ToolRegistry, ToolResultData};
use luxe_core::usage::{UsageSource, UsageTotals};
use tokio::sync::Mutex;

/// A tool that blocks until released — the wedged call at the head of the batch.
struct GateTool {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl Tool for GateTool {
    fn name(&self) -> &str {
        "gate"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
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

/// A tool that never returns, and reports whether its future was dropped.
///
/// The drop flag is the real assertion: the engine stops a wedged tool by dropping its future, which
/// is only a *stop* if the work goes with it. A tool holding a child process relies on exactly this
/// (`kill_on_drop`), so a regression that merely stopped waiting — leaving the browser running — has to
/// fail here.
struct WedgedTool {
    entered: Arc<tokio::sync::Notify>,
    dropped: Arc<AtomicUsize>,
}

struct DropFlag(Arc<AtomicUsize>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl Tool for WedgedTool {
    fn name(&self) -> &str {
        "wedged"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "wedged".to_string(),
            description: "Never return.".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }

    /// Short enough to assert on, and the same mechanism a 60s network bound uses.
    fn timeout(&self) -> Option<std::time::Duration> {
        Some(std::time::Duration::from_millis(120))
    }

    async fn run(&self, _args: serde_json::Value, _cx: &ToolContext) -> ToolResultData {
        let _flag = DropFlag(self.dropped.clone());
        self.entered.notify_one();
        std::future::pending::<()>().await;
        unreachable!("a pending future never resolves")
    }
}

/// A tool that counts the times it ran. It must never run after a cancel.
struct CounterTool(Arc<AtomicUsize>);

#[async_trait::async_trait]
impl Tool for CounterTool {
    fn name(&self) -> &str {
        "counter"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "counter".to_string(),
            description: "Count invocations.".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }

    async fn run(&self, _args: serde_json::Value, _cx: &ToolContext) -> ToolResultData {
        self.0.fetch_add(1, Ordering::SeqCst);
        ToolResultData { content: vec![ContentBlock::Text("ran".into())], is_error: false }
    }
}

/// Emits the incident's shape on its first call — one `gate`, then two `counter` calls in the same
/// message — and records every request so a test can inspect what reached the wire.
struct BatchProvider {
    calls: Arc<Mutex<Vec<CompletionRequest>>>,
}

#[async_trait::async_trait]
impl Provider for BatchProvider {
    fn name(&self) -> &str {
        "batch"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let first = {
            let mut calls = self.calls.lock().await;
            calls.push(request);
            calls.len() == 1
        };
        let events = if first {
            let mut events = vec![tool_start(0, "call_gate", "gate")];
            events.push(tool_start(1, "call_skipped_a", "counter"));
            events.push(tool_start(2, "call_skipped_b", "counter"));
            events.push(ProviderEvent::Done { reason: StopReason::ToolCall });
            events
        } else {
            vec![
                ProviderEvent::TextDelta { index: 0, delta: "done".to_string() },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }
}

fn tool_start(index: usize, id: &str, name: &str) -> ProviderEvent {
    ProviderEvent::ToolCallStart { index, id: id.to_string(), name: name.to_string() }
}

/// Calls one named tool on its first turn, then answers in prose.
struct SingleCallProvider {
    tool: &'static str,
    calls: Arc<Mutex<Vec<CompletionRequest>>>,
}

#[async_trait::async_trait]
impl Provider for SingleCallProvider {
    fn name(&self) -> &str {
        "single"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let first = {
            let mut calls = self.calls.lock().await;
            calls.push(request);
            calls.len() == 1
        };
        let events = if first {
            vec![
                tool_start(0, "call_wedged", self.tool),
                ProviderEvent::Done { reason: StopReason::ToolCall },
            ]
        } else {
            vec![
                ProviderEvent::TextDelta { index: 0, delta: "gave up".to_string() },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }
}

/// Answers with plain text and reports usage, so the resumed-transcript test can see the request.
struct EchoProvider {
    calls: Arc<Mutex<Vec<CompletionRequest>>>,
}

#[async_trait::async_trait]
impl Provider for EchoProvider {
    fn name(&self) -> &str {
        "echo"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        self.calls.lock().await.push(request);
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::TextDelta { index: 0, delta: "ok".to_string() },
            ProviderEvent::Usage(Usage {
                prompt_tokens: 1_000,
                completion_tokens: 10,
                total_tokens: 1_010,
                ..Usage::default()
            }),
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ])))
    }
}

fn config(
    store: JsonlSessionStore,
    session_id: SessionId,
    provider: Arc<dyn Provider + Send + Sync>,
    tools: ToolRegistry,
) -> EngineConfig {
    EngineConfig {
        model: ModelInfo { context_window: 200_000, ..ModelInfo::default() },
        tool_registry: tools,
        ..base_config(store, session_id, provider)
    }
}

/// Cancelling ends the batch. The queued calls do not run, and all three still come back answered —
/// with the distinction that matters: the one that started may have half-happened, the two that never
/// started are safe to retry.
#[tokio::test]
async fn cancelling_a_batch_stops_the_queued_calls_and_answers_all_of_them() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let ran = Arc::new(AtomicUsize::new(0));
    let calls = Arc::new(Mutex::new(Vec::new()));

    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(GateTool { entered: entered.clone(), release: release.clone() }));
    tools.register(Arc::new(CounterTool(ran.clone())));

    let reader = JsonlSessionStore::new(tmp.path().join("sessions"));
    let (action_tx, mut event_rx) =
        Engine::spawn(config(store, session_id.clone(), Arc::new(BatchProvider { calls }), tools));

    action_tx
        .send(Action::SubmitUserMessage {
            content: "capture the site".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    // Interrupt while the first call is genuinely in flight.
    tokio::time::timeout(EVENT_WAIT, entered.notified()).await.unwrap();
    action_tx.send(Action::Cancel).await.unwrap();
    await_cancel_taken(&mut event_rx).await;
    // Nothing needs releasing — the cancel drops the tool's future where it stands. Release anyway,
    // so a regression that ignores cancellation fails an assertion instead of hanging the suite.
    release.notify_one();
    events_until_turn_end(&mut event_rx).await;

    assert_eq!(
        ran.load(Ordering::SeqCst),
        0,
        "a cancelled turn must not start the calls still queued behind the one it interrupted"
    );

    let messages: Vec<Message> = reader
        .load(&session_id)
        .unwrap()
        .into_iter()
        .filter_map(|entry| match entry.kind {
            EntryKind::Message(message) => Some(message),
            _ => None,
        })
        .collect();
    let (made, answered) = pairing(&messages);
    assert_eq!(made.len(), 3, "the assistant message committed three calls");
    assert_eq!(
        answered.len(),
        3,
        "and the transcript answers all three — an unanswered call is a permanent 400"
    );

    // The three explanations are not interchangeable: only the calls that never started may be
    // described as having had no effect.
    assert!(
        result_text(&messages, "call_gate").contains("partially completed"),
        "the interrupted call may have done something"
    );
    for skipped in ["call_skipped_a", "call_skipped_b"] {
        assert!(
            result_text(&messages, skipped).contains("no effect"),
            "{skipped} never started, so it is safe to retry"
        );
    }

    let _ = action_tx.send(Action::Close).await;
}

/// A session saved with the gap still resumes: the repair happens on the way to the provider, so no
/// migration is needed and the durable log keeps saying what actually happened.
#[tokio::test]
async fn a_transcript_that_already_has_orphaned_calls_still_reaches_the_provider_valid() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    // Exactly the incident: three calls in one assistant message, no tool message, a user message
    // chained straight after it, and a usage record for the call that made them.
    let user = store
        .append(&session_id, None, EntryKind::Message(Message::user("look at the site")))
        .unwrap();
    let assistant = store
        .append(
            &session_id,
            Some(user),
            EntryKind::Message(Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::ToolCall {
                        id: "call_orphan_a".into(),
                        name: "web_fetch".into(),
                        arguments: serde_json::json!({"url": "https://example.invalid"}),
                    },
                    ContentBlock::ToolCall {
                        id: "call_orphan_b".into(),
                        name: "screenshot".into(),
                        arguments: serde_json::json!({"mode": "url"}),
                    },
                ],
            }),
        )
        .unwrap();
    let usage = store
        .append(
            &session_id,
            Some(assistant),
            EntryKind::Usage {
                source: UsageSource::Agent,
                model: "test-model".into(),
                totals: UsageTotals {
                    calls: 1,
                    prompt_tokens: 69_897,
                    completion_tokens: 415,
                    ..UsageTotals::default()
                },
            },
        )
        .unwrap();
    store
        .append(
            &session_id,
            Some(usage),
            EntryKind::Message(Message::user("never mind, do this instead")),
        )
        .unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let (action_tx, mut event_rx) = Engine::spawn(config(
        store,
        session_id.clone(),
        Arc::new(EchoProvider { calls: calls.clone() }),
        ToolRegistry::new(),
    ));

    action_tx.send(Action::LoadSession(session_id.to_string())).await.unwrap();

    // Resume reports the occupancy the provider measured, not a guess and not zero. The dock read
    // 0% of 1.1M on a session holding ~70K tokens, because the estimator counted only text blocks.
    let mut observed = None;
    let deadline = tokio::time::Instant::now() + EVENT_WAIT;
    while observed.is_none() && tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(EVENT_WAIT, event_rx.recv()).await {
            Ok(Some(Event::ContextUsage { used_tokens, context_window })) => {
                observed = Some((used_tokens, context_window));
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => break,
        }
    }
    assert_eq!(
        observed,
        Some((70_312, 200_000)),
        "resumed occupancy is the last call's prompt + completion tokens"
    );

    action_tx
        .send(Action::SubmitUserMessage {
            content: "carry on".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();
    events_until_turn_end(&mut event_rx).await;

    let requests = calls.lock().await;
    let request = requests.first().expect("the turn made a provider call");
    let (made, answered) = pairing(&request.messages);
    assert_eq!(made.len(), 2, "both orphaned calls are still replayed");
    assert_eq!(answered.len(), 2, "and both now carry a result");
    for orphan in ["call_orphan_a", "call_orphan_b"] {
        assert!(
            result_text(&request.messages, orphan).contains("unknown"),
            "a replayed orphan's outcome is genuinely unknowable, and must say so"
        );
    }

    // The durable log is a record, not a workspace: it still shows the calls unanswered.
    let reader = JsonlSessionStore::new(tmp.path().join("sessions"));
    let persisted: Vec<Message> = reader
        .load(&session_id)
        .unwrap()
        .into_iter()
        .filter_map(|entry| match entry.kind {
            EntryKind::Message(message) => Some(message),
            _ => None,
        })
        .collect();
    let (_, persisted_results) = pairing(&persisted);
    assert!(
        !persisted_results.iter().any(|id| id == "call_orphan_a"),
        "the repair belongs to the outgoing copy; rewriting history would lose what happened"
    );

    let _ = action_tx.send(Action::Close).await;
}

/// A tool that never returns costs its declared bound, not the session — and the work is dropped, not
/// merely abandoned. That distinction is the whole reason the deadline lives in the engine: a headless
/// browser left running would still be holding the port, the memory, and the confusion.
#[tokio::test]
async fn a_wedged_tool_hits_its_deadline_and_its_work_is_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let entered = Arc::new(tokio::sync::Notify::new());
    let dropped = Arc::new(AtomicUsize::new(0));
    let calls = Arc::new(Mutex::new(Vec::new()));

    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(WedgedTool { entered: entered.clone(), dropped: dropped.clone() }));

    let reader = JsonlSessionStore::new(tmp.path().join("sessions"));
    let (action_tx, mut event_rx) = Engine::spawn(config(
        store,
        session_id.clone(),
        Arc::new(SingleCallProvider { tool: "wedged", calls }),
        tools,
    ));

    let started = tokio::time::Instant::now();
    action_tx
        .send(Action::SubmitUserMessage {
            content: "capture luxe.dev".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();
    tokio::time::timeout(EVENT_WAIT, entered.notified()).await.unwrap();
    events_until_turn_end(&mut event_rx).await;

    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the turn ended on the tool's bound, not on the test's patience"
    );
    assert_eq!(
        dropped.load(Ordering::SeqCst),
        1,
        "the tool's future must be dropped, so anything it holds (a child process) goes with it"
    );

    let messages: Vec<Message> = reader
        .load(&session_id)
        .unwrap()
        .into_iter()
        .filter_map(|entry| match entry.kind {
            EntryKind::Message(message) => Some(message),
            _ => None,
        })
        .collect();
    let (made, answered) = pairing(&messages);
    assert_eq!(made, answered, "a call stopped by its deadline is still a call with a result");
    let text = result_text(&messages, "call_wedged");
    assert!(text.contains("`wedged`"), "name the tool that hung: {text}");
    assert!(text.contains("still running after"), "and say what happened: {text}");

    let _ = action_tx.send(Action::Close).await;
}

/// Records the cancel flag each call is handed, then answers in prose (first call: one `gate`).
struct CancelWatchProvider {
    flags: Arc<Mutex<Vec<bool>>>,
    calls: Arc<Mutex<Vec<CompletionRequest>>>,
}

#[async_trait::async_trait]
impl Provider for CancelWatchProvider {
    fn name(&self) -> &str {
        "cancel-watch"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        self.flags.lock().await.push(*cancel.borrow());
        let first = {
            let mut calls = self.calls.lock().await;
            calls.push(request);
            calls.len() == 1
        };
        let events = if first {
            vec![
                tool_start(0, "call_gate", "gate"),
                ProviderEvent::Done { reason: StopReason::ToolCall },
            ]
        } else {
            vec![
                ProviderEvent::TextDelta { index: 0, delta: "carrying on".to_string() },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }
}

#[tokio::test]
async fn the_turn_after_a_cancel_is_not_born_cancelled() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let flags = Arc::new(Mutex::new(Vec::new()));

    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(GateTool { entered: entered.clone(), release: release.clone() }));

    let reader = JsonlSessionStore::new(tmp.path().join("sessions"));
    let (action_tx, mut event_rx) = Engine::spawn(config(
        store,
        session_id.clone(),
        Arc::new(CancelWatchProvider { flags: flags.clone(), calls: Arc::new(Mutex::new(vec![])) }),
        tools,
    ));

    action_tx
        .send(Action::SubmitUserMessage {
            content: "launch it".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();
    tokio::time::timeout(EVENT_WAIT, entered.notified()).await.unwrap();
    action_tx.send(Action::Cancel).await.unwrap();
    await_cancel_taken(&mut event_rx).await;
    release.notify_one();
    events_until_turn_end(&mut event_rx).await;

    action_tx
        .send(Action::SubmitUserMessage {
            content: "it is open".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();
    events_until_turn_end(&mut event_rx).await;

    assert_eq!(
        *flags.lock().await,
        vec![false, false],
        "a fresh turn must not inherit the last turn's cancel"
    );

    let messages: Vec<Message> = reader
        .load(&session_id)
        .unwrap()
        .into_iter()
        .filter_map(|entry| match entry.kind {
            EntryKind::Message(message) => Some(message),
            _ => None,
        })
        .collect();
    assert!(
        messages
            .iter()
            .flat_map(|message| &message.content)
            .any(|block| matches!(block, ContentBlock::Text(text) if text == "carrying on")),
        "the second turn actually got an answer: {messages:#?}"
    );
    assert!(
        !messages
            .iter()
            .flat_map(|message| &message.content)
            .any(|block| matches!(block, ContentBlock::Text(text) if text.trim().is_empty())),
        "an empty text block is a permanent 400: {messages:#?}"
    );

    let _ = action_tx.send(Action::Close).await;
}

mod empty_replies;

/// A front-end that closes mid-turn ends the turn and the engine with it, the way a cancel ends a
/// turn, rather than leaving the turn to run on behind a front-end that has gone.
///
/// The break this catches: `Close` waited behind the turn, so a TUI leaving for another session
/// gave up waiting and started the next engine while the old one's tools kept running, in a
/// directory that may no longer have been theirs. The call it interrupted is still answered.
#[tokio::test]
async fn closing_mid_turn_stops_the_turn_and_the_engine() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(GateTool { entered: entered.clone(), release: release.clone() }));

    let reader = JsonlSessionStore::new(tmp.path().join("sessions"));
    let provider = Arc::new(SingleCallProvider { tool: "gate", calls });
    let (action_tx, _event_rx, task) =
        Engine::spawn_task(config(store, session_id.clone(), provider, tools));
    action_tx
        .send(Action::SubmitUserMessage {
            content: "wait at the gate".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    tokio::time::timeout(EVENT_WAIT, entered.notified()).await.unwrap();
    action_tx.send(Action::Close).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("the engine closed without the gate ever opening")
        .unwrap();

    let messages: Vec<Message> = reader
        .load(&session_id)
        .unwrap()
        .into_iter()
        .filter_map(|entry| match entry.kind {
            EntryKind::Message(message) => Some(message),
            _ => None,
        })
        .collect();
    let (made, answered) = pairing(&messages);
    assert_eq!((made.len(), answered.len()), (1, 1), "the interrupted call is answered");
}

/// A message queued behind a running turn does not start once a close has stopped that turn: the
/// close is served first, so the engine ends rather than running the next turn behind a front-end
/// that has gone.
#[tokio::test]
async fn a_close_outranks_a_message_queued_behind_the_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(GateTool { entered: entered.clone(), release: release.clone() }));

    let provider = Arc::new(SingleCallProvider { tool: "gate", calls: calls.clone() });
    let (action_tx, _event_rx, task) =
        Engine::spawn_task(config(store, session_id, provider, tools));
    let submit = |content: &str| Action::SubmitUserMessage {
        content: content.to_string(),
        images: vec![],
        remediation: None,
    };
    action_tx.send(submit("wait at the gate")).await.unwrap();
    tokio::time::timeout(EVENT_WAIT, entered.notified()).await.unwrap();
    action_tx.send(submit("and then this")).await.unwrap();
    action_tx.send(Action::Close).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("the engine closed")
        .unwrap();

    assert_eq!(calls.lock().await.len(), 1, "the queued message never reached the provider");
}
