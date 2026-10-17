//! P1 regression: a single turn is an inner agentic loop — the model calls a tool, the
//! engine runs it, feeds the result back, and the model produces a final answer *within the
//! same turn* (no second user message required).
//!
//! Also asserts the runaway guard: a model that calls tools forever ends the turn once
//! `max_tool_iterations` is hit.

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

/// A trivial tool that echoes a fixed marker so the test can prove the model saw its output.
struct EchoTool;

#[async_trait::async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn schema(&self) -> luxe_core::provider::ToolSchema {
        luxe_core::provider::ToolSchema {
            name: "echo".to_string(),
            description: "Echo a marker.".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }

    async fn run(&self, _args: serde_json::Value, _cx: &ToolContext) -> ToolResultData {
        ToolResultData {
            content: vec![ContentBlock::Text("ECHO_RESULT_42".to_string())],
            is_error: false,
        }
    }
}

/// A tool (named `echo` so `ScriptedProvider` calls it) whose output contains secret-shaped
/// strings. Proves the engine scrubs tool output before it is committed / emitted / fed back.
struct SecretLeakTool;

#[async_trait::async_trait]
impl Tool for SecretLeakTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn schema(&self) -> luxe_core::provider::ToolSchema {
        luxe_core::provider::ToolSchema {
            name: "echo".to_string(),
            description: "Leak a secret.".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }

    async fn run(&self, _args: serde_json::Value, _cx: &ToolContext) -> ToolResultData {
        ToolResultData {
            content: vec![ContentBlock::Text(
                "api=sk-abcdefghijklmnopqrstuvwxyz0123 gh=ghp_abcdefghijklmnopqrstuvwxyz0123 \
                 custom=plainly-shapeless-credential"
                    .to_string(),
            )],
            is_error: false,
        }
    }
}

/// Records what each provider call saw, and scripts a two-step turn:
/// call 1 → tool_call(echo); call 2 → final text (only if it saw the tool result).
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
            // First call: emit a tool call, no final text.
            vec![
                ProviderEvent::ToolCallStart {
                    index: 0,
                    id: "call_1".to_string(),
                    name: "echo".to_string(),
                },
                ProviderEvent::ToolCallDelta { index: 0, json_delta: "{}".to_string() },
                ProviderEvent::ToolCallEnd { index: 0 },
                ProviderEvent::Done { reason: StopReason::ToolCall },
            ]
        } else {
            // Second call: the tool result must be visible in the request.
            let saw_result = req.messages.iter().any(|m| {
                m.role == Role::Tool
                    && m.content.iter().any(|b| {
                        matches!(b, ContentBlock::ToolResult { content, .. }
                            if content.iter().any(|o| matches!(o,
                                luxe_core::message::ToolOutput::Text(t) if t.contains("ECHO_RESULT_42"))))
                    })
            });
            let reply = if saw_result { "I saw ECHO_RESULT_42" } else { "MISSING_TOOL_RESULT" };
            vec![
                ProviderEvent::TextStart { index: 0 },
                ProviderEvent::TextDelta { index: 0, delta: reply.to_string() },
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
    max_tool_iterations: Option<usize>,
) -> EngineConfig {
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(EchoTool));
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
        guided_review_project_root: None,
        invoked_from: None,
        external_work: None,
        review_context_files: Vec::new(),
        review_exclude: Vec::new(),
        remember_runtime_state: false,
        cache_warming: luxe_core::cache_warming::CacheWarming::Off,
        review_insight_provider: None,
        github: None,
        permissions_file: None,
        max_tool_iterations,
        startup_notices: Vec::new(),
        action_tx: None,
    }
}

/// Drain events until a single `TurnEnded`, collecting the streamed assistant text.
async fn run_one_turn(
    action_tx: &tokio::sync::mpsc::Sender<Action>,
    event_rx: &mut tokio::sync::mpsc::Receiver<Event>,
    prompt: &str,
) -> String {
    action_tx
        .send(Action::SubmitUserMessage {
            content: prompt.to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    let mut text = String::new();
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), event_rx.recv()).await {
            Ok(Some(Event::TokenDelta { delta, .. })) => text.push_str(&delta),
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out waiting for turn to end"),
        }
    }
    text
}

#[tokio::test]
async fn turn_feeds_tool_result_back_to_model() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(ScriptedProvider { calls: calls.clone() });

    let (action_tx, mut event_rx) =
        Engine::spawn(engine_config(store, session_id, provider, Some(25)));

    let text = run_one_turn(&action_tx, &mut event_rx, "use the echo tool").await;

    // The turn produced the final answer without a second user message, and that answer
    // proves the model saw the tool output.
    assert_eq!(text, "I saw ECHO_RESULT_42", "model must see the tool result within the turn");

    // Exactly two provider calls happened inside the single turn.
    assert_eq!(calls.lock().await.len(), 2, "turn should make two provider round-trips");

    action_tx.send(Action::Close).await.unwrap();
}

#[tokio::test]
async fn turn_emits_committed_tool_result_message() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();
    let provider = Arc::new(ScriptedProvider { calls: Arc::new(Mutex::new(Vec::new())) });
    let (action_tx, mut event_rx) =
        Engine::spawn(engine_config(store, session_id, provider, Some(25)));

    action_tx
        .send(Action::SubmitUserMessage {
            content: "use the echo tool".into(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    let mut committed_tool_message = None;
    let mut saw_tool_end_after_commit = false;
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), event_rx.recv()).await {
            Ok(Some(Event::MessageCommitted { message, .. })) if message.role == Role::Tool => {
                committed_tool_message = Some(message);
            }
            Ok(Some(Event::ToolEnded { call_id, .. })) if call_id == "call_1" => {
                saw_tool_end_after_commit = committed_tool_message.is_some();
            }
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out waiting for turn to end"),
        }
    }

    let message = committed_tool_message.expect("tool result must be committed to front-ends");
    assert!(matches!(message.content.as_slice(), [ContentBlock::ToolResult {
        call_id,
        content,
        is_error: false,
    }] if call_id == "call_1" && content.iter().any(|output| matches!(output,
        luxe_core::message::ToolOutput::Text(text) if text == "ECHO_RESULT_42"))));
    assert!(saw_tool_end_after_commit, "ToolEnded must follow the committed tool result");

    action_tx.send(Action::Close).await.unwrap();
}

#[tokio::test]
async fn tool_output_is_redacted_before_it_is_committed() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();
    let provider = Arc::new(ScriptedProvider { calls: Arc::new(Mutex::new(Vec::new())) });

    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(SecretLeakTool));
    let config = EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider,
        providers: std::collections::HashMap::new(),
        model: ModelInfo::default(),
        tool_registry: tools,
        hook_registry: HookRegistry::new(),
        session_id: session_id.clone(),
        system_prompt: None,
        todo_store: None,
        compaction_settings: luxe_core::compaction::CompactionSettings::default(),
        permission: Default::default(),
        modes: Default::default(),
        reviewer: None,
        formatter_registry: None,
        spend_sink: None,
        guided_review_project_root: None,
        invoked_from: None,
        external_work: None,
        review_context_files: Vec::new(),
        review_exclude: Vec::new(),
        remember_runtime_state: false,
        cache_warming: luxe_core::cache_warming::CacheWarming::Off,
        review_insight_provider: None,
        github: None,
        permissions_file: None,
        max_tool_iterations: Some(25),
        startup_notices: Vec::new(),
        action_tx: None,
    };
    // a key the provider layer resolved, with no shape a pattern could know
    luxe_core::redact::process_secrets().add("plainly-shapeless-credential");
    let (action_tx, mut event_rx) = Engine::spawn(config);

    action_tx
        .send(Action::SubmitUserMessage {
            content: "leak a secret".into(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    let mut committed_tool_message = None;
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), event_rx.recv()).await {
            Ok(Some(Event::MessageCommitted { message, .. })) if message.role == Role::Tool => {
                committed_tool_message = Some(message);
            }
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out waiting for turn to end"),
        }
    }

    let message = committed_tool_message.expect("tool result must be committed");
    let output = match message.content.as_slice() {
        [ContentBlock::ToolResult { content, .. }] => content
            .iter()
            .filter_map(|o| match o {
                luxe_core::message::ToolOutput::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => panic!("expected a single ToolResult block, got {other:?}"),
    };

    // The raw secrets never reach the committed transcript; the redaction markers do.
    assert!(!output.contains("sk-abcdefghijklmnopqrstuvwxyz0123"), "raw api key leaked: {output}");
    assert!(
        !output.contains("ghp_abcdefghijklmnopqrstuvwxyz0123"),
        "raw gh token leaked: {output}"
    );
    assert!(output.contains("[REDACTED_API_KEY]"), "missing api-key marker: {output}");
    assert!(output.contains("[REDACTED_GITHUB_TOKEN]"), "missing gh-token marker: {output}");
    assert!(!output.contains("plainly-shapeless"), "a loaded api key leaked: {output}");

    action_tx.send(Action::Close).await.unwrap();
}

/// A provider that calls a tool on *every* call — proves the runaway guard ends the turn.
struct AlwaysToolProvider {
    calls: Arc<Mutex<usize>>,
}

#[async_trait::async_trait]
impl Provider for AlwaysToolProvider {
    fn name(&self) -> &str {
        "always-tool"
    }

    async fn stream(
        &self,
        _req: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        *self.calls.lock().await += 1;
        let events = vec![
            ProviderEvent::ToolCallStart {
                index: 0,
                id: "call_x".to_string(),
                name: "echo".to_string(),
            },
            ProviderEvent::ToolCallDelta { index: 0, json_delta: "{}".to_string() },
            ProviderEvent::ToolCallEnd { index: 0 },
            ProviderEvent::Done { reason: StopReason::ToolCall },
        ];
        Ok(Box::pin(stream::iter(events)))
    }
}

#[tokio::test]
async fn turn_wraps_up_gracefully_at_the_configured_tool_iteration_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let calls = Arc::new(Mutex::new(0usize));
    let provider = Arc::new(AlwaysToolProvider { calls: calls.clone() });

    // Bound the inner loop to 3 iterations.
    let (action_tx, mut event_rx) =
        Engine::spawn(engine_config(store, session_id, provider, Some(3)));

    action_tx
        .send(Action::SubmitUserMessage {
            content: "loop forever".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    let mut saw_notice = false;
    let mut saw_error = false;
    let ended;
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), event_rx.recv()).await {
            Ok(Some(Event::Notice(msg))) if msg.contains("tool-call limit reached") => {
                saw_notice = true;
            }
            Ok(Some(Event::Error(_))) => saw_error = true,
            Ok(Some(Event::TurnEnded { reason })) => {
                ended = reason;
                break;
            }
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out — the limit did not end the turn"),
        }
    }

    assert!(saw_notice, "reaching the limit announces the wrap-up");
    assert!(!saw_error, "a configured limit is not an error");
    // The reason a front-end reads. It was the constant `EndTurn` for every ending a turn has,
    // which reported a run that hit its ceiling as one that finished saying what it meant to.
    //
    // This provider keeps calling tools through the wrap-up, so the turn leaves by the refusal
    // door. `a_wrapped_up_turn_that_answers_in_prose_still_reports_the_limit` covers the *ordinary*
    // ending, which leaves by the normal one and is the case that got this wrong first time.
    assert_eq!(
        ended,
        luxe_core::event::StopReason::Other("tool limit".to_string()),
        "the turn has to say which ending this was"
    );
    // 3 tool round-trips, then one final tool-less wrap-up call.
    assert_eq!(*calls.lock().await, 4, "the limit adds a wrap-up call, then stops");

    let _ = action_tx.send(Action::Close).await;
}

/// Multi-turn history: a second user message must carry the first turn's messages so the
/// provider sees prior context (regression for the parent-chaining bug).
#[tokio::test]
async fn second_turn_carries_prior_history() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    // Reuse ScriptedProvider but only care about the request captured on later calls.
    let calls: Arc<Mutex<Vec<CompletionRequest>>> = Arc::new(Mutex::new(Vec::new()));

    struct TextProvider {
        calls: Arc<Mutex<Vec<CompletionRequest>>>,
    }
    #[async_trait::async_trait]
    impl Provider for TextProvider {
        fn name(&self) -> &str {
            "text"
        }
        async fn stream(
            &self,
            req: CompletionRequest,
            _cancel: tokio::sync::watch::Receiver<bool>,
        ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
            self.calls.lock().await.push(req);
            Ok(Box::pin(stream::iter(vec![
                ProviderEvent::TextDelta { index: 0, delta: "ok".to_string() },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ])))
        }
    }

    let provider = Arc::new(TextProvider { calls: calls.clone() });
    let (action_tx, mut event_rx) =
        Engine::spawn(engine_config(store, session_id, provider, Some(25)));

    let _ = run_one_turn(&action_tx, &mut event_rx, "first message").await;
    let _ = run_one_turn(&action_tx, &mut event_rx, "second message").await;

    let calls = calls.lock().await;
    assert_eq!(calls.len(), 2);
    // The second request must include BOTH user messages plus the first assistant reply.
    let second = &calls[1];
    let user_texts: Vec<String> = second
        .messages
        .iter()
        .filter(|m| m.role == Role::User)
        .filter_map(|m| match m.content.first() {
            Some(ContentBlock::Text(t)) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert!(
        user_texts.iter().any(|t| t == "first message")
            && user_texts.iter().any(|t| t == "second message"),
        "second turn must carry prior history, got: {user_texts:?}"
    );

    drop(action_tx);
}

/// A provider that calls a tool until it is handed a request with no tools, then answers in prose.
///
/// The ordinary shape of a wrapped-up turn, and the one the reason got wrong: the tool-less final
/// call succeeds, so the turn leaves through the "no tool calls" door rather than the refusal.
struct ToolsUntilAskedToStop;

#[async_trait::async_trait]
impl Provider for ToolsUntilAskedToStop {
    fn name(&self) -> &str {
        "tools-until-asked-to-stop"
    }

    async fn stream(
        &self,
        req: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let events = if req.tools.is_empty() {
            vec![
                ProviderEvent::TextStart { index: 0 },
                ProviderEvent::TextDelta { index: 0, delta: "wrapping up".to_string() },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ]
        } else {
            vec![
                ProviderEvent::ToolCallStart {
                    index: 0,
                    id: "call_y".to_string(),
                    name: "echo".to_string(),
                },
                ProviderEvent::ToolCallDelta { index: 0, json_delta: "{}".to_string() },
                ProviderEvent::Done { reason: StopReason::ToolCall },
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }
}

/// A turn stopped by its ceiling has to say so even when it stops politely.
///
/// The wrap-up call is tool-less, so a well-behaved model answers in prose and the loop exits by
/// the same branch a normal turn does. Reporting that as `EndTurn` tells a front-end the model
/// finished when it was in fact cut short — which is exactly what the field is read for.
#[tokio::test]
async fn a_wrapped_up_turn_that_answers_in_prose_still_reports_the_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let (action_tx, mut event_rx) =
        Engine::spawn(engine_config(store, session_id, Arc::new(ToolsUntilAskedToStop), Some(2)));

    action_tx
        .send(Action::SubmitUserMessage {
            content: "keep going".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    let ended = loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), event_rx.recv()).await {
            Ok(Some(Event::TurnEnded { reason })) => break reason,
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out — the limit did not end the turn"),
        }
    };

    assert_eq!(
        ended,
        luxe_core::event::StopReason::Other("tool limit".to_string()),
        "a turn cut short by its ceiling must not report a normal ending"
    );

    let _ = action_tx.send(Action::Close).await;
}

/// Refuses every call, the way an outage or a bad key does.
struct FailingProvider;

#[async_trait::async_trait]
impl Provider for FailingProvider {
    fn name(&self) -> &str {
        "failing"
    }

    async fn stream(
        &self,
        _request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        // One a retry cannot fix, so the turn fails at once rather than after the backoff
        Err(ProviderError::Other("400: the request is malformed".to_string()))
    }
}

/// A turn that failed still stopped, and only `TurnEnded` says so.
///
/// The error itself was always announced, so this looked handled. What was missing is the frame
/// that closes the turn: the TUI leaves its spinner up, a `--format json` reader never sees the
/// turn finish, and an RPC client waits on a turn that is already over. Every other way out of the
/// loop sends one — the runaway guard, a cancel, a truncated reply — and the failure path is the
/// one that did not.
#[tokio::test]
async fn a_failed_turn_still_tells_the_front_end_it_ended() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let (action_tx, mut event_rx) =
        Engine::spawn(engine_config(store, session_id, Arc::new(FailingProvider), Some(4)));
    action_tx
        .send(Action::SubmitUserMessage {
            content: "this one cannot work".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    let mut reported = None;
    let ended = loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), event_rx.recv()).await {
            Ok(Some(Event::Error(message))) => reported = Some(message),
            Ok(Some(Event::TurnEnded { reason })) => break reason,
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("a failed turn left the front-end waiting"),
        }
    };

    // And it says the turn failed rather than passing it off as a normal finish.
    assert_eq!(ended, luxe_core::event::StopReason::Other("error".to_string()));
    let reported = reported.expect("the failure is announced too");
    assert!(reported.contains("malformed"), "{reported}");

    action_tx.send(Action::Close).await.ok();
}
