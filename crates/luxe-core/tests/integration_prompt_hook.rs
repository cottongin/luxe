//! Integration: what a turn's prompt carries. `UserPromptSubmit` — a deny blocks the turn before
//! any provider call and commits nothing, and a hook's context rides in the transcript rather than
//! the system prompt — and the directory the prompt reports is the one tools run in.

#![allow(clippy::unwrap_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::stream::{self, BoxStream};
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint};
use luxe_core::message::{ContentBlock, Role};
use luxe_core::provider::{
    CompletionRequest, ModelInfo, Provider, ProviderError, ProviderEvent, StopReason,
};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use luxe_core::tool::{Tool, ToolContext, ToolRegistry, ToolResultData};
use luxe_core::HookRegistry;
use tokio::sync::Mutex;

/// Counts how many times the provider is asked to stream a completion, keeping each request.
struct CountingProvider {
    calls: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<CompletionRequest>>>,
}

#[async_trait::async_trait]
impl Provider for CountingProvider {
    fn name(&self) -> &str {
        "counting"
    }

    async fn stream(
        &self,
        req: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().await.push(req);
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::TextDelta { index: 0, delta: "hi".to_string() },
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ])))
    }
}

/// A prompt policy: refuses a prompt carrying `CORP-SECRET`, and otherwise adds context that
/// changes every turn — the way a hook reporting git state or the time does.
struct PromptPolicyHook {
    turns: AtomicUsize,
}

#[async_trait::async_trait]
impl Hook for PromptPolicyHook {
    fn name(&self) -> &'static str {
        "prompt-policy"
    }

    fn interest(&self) -> &[HookPoint] {
        &[HookPoint::UserPromptSubmit]
    }

    async fn on(&self, _point: HookPoint, ev: &HookEvent, _cx: &HookCtx) -> HookOutcome {
        let HookEvent::Other(event) = ev else { return HookOutcome::Continue };
        let prompt = event["content"].as_str().unwrap_or_default();
        if prompt.contains("CORP-SECRET") {
            return HookOutcome::Deny { reason: "prompt policy".to_string() };
        }
        let turn = self.turns.fetch_add(1, Ordering::SeqCst) + 1;
        HookOutcome::InjectContext(format!("LIVE-CONTEXT turn {turn}"))
    }
}

fn config(
    store: JsonlSessionStore,
    session_id: luxe_core::session::SessionId,
    provider: CountingProvider,
) -> EngineConfig {
    let mut hooks = HookRegistry::new();
    hooks.register(Arc::new(PromptPolicyHook { turns: AtomicUsize::new(0) }));
    EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider: Arc::new(provider),
        providers: std::collections::HashMap::new(),
        model: ModelInfo::default(),
        tool_registry: ToolRegistry::new(),
        hook_registry: hooks,
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
        max_tool_iterations: Some(4),
        startup_notices: Vec::new(),
        action_tx: None,
    }
}

/// Submit `prompt` and drain the turn, returning whether it was reported blocked.
async fn submit(
    tx: &tokio::sync::mpsc::Sender<Action>,
    rx: &mut tokio::sync::mpsc::Receiver<Event>,
    prompt: &str,
) -> bool {
    tx.send(Action::SubmitUserMessage {
        content: prompt.to_string(),
        images: vec![],
        remediation: None,
    })
    .await
    .unwrap();
    let mut blocked = false;
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
            Ok(Some(Event::Error(msg))) if msg.contains("blocked by hook") => blocked = true,
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out"),
        }
    }
    blocked
}

/// Every file's contents under `dir`, concatenated.
///
/// Session claims are skipped: they hold nothing, and on Windows an open session's claim locks its
/// `.lock` file against any other read.
fn everything_under(dir: &std::path::Path) -> String {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|entry| match entry.path() {
            path if path.is_dir() => everything_under(&path),
            path if path.extension().is_some_and(|extension| extension == "lock") => String::new(),
            path => std::fs::read_to_string(path).unwrap(),
        })
        .collect()
}

/// Everything a request says, flattened, so an assertion can ask whether a string went out.
fn request_text(request: &CompletionRequest) -> String {
    let mut text = request.system.clone().unwrap_or_default();
    for message in &request.messages {
        for block in &message.content {
            if let ContentBlock::Text(t) = block {
                text.push_str(t);
            }
        }
    }
    text
}

/// A refused prompt is refused everywhere: no provider call, nothing in the session file, and
/// nothing in the history the next turn sends.
///
/// The prompt used to be committed before the hook ran, so the guide's own example — refuse a
/// prompt that leaks an internal token — kept the token in the transcript, and the next turn sent
/// it to the provider as history. A real model quoted it back.
#[tokio::test]
async fn a_refused_prompt_leaves_nothing_behind() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/prompt-hook").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = CountingProvider { calls: calls.clone(), requests: requests.clone() };
    let (tx, mut rx) = Engine::spawn(config(store, session_id, provider));

    assert!(submit(&tx, &mut rx, "remember CORP-SECRET-4471 for me").await, "reason surfaced");
    assert_eq!(calls.load(Ordering::SeqCst), 0, "provider must not be called for a blocked prompt");

    assert!(!submit(&tx, &mut rx, "what did I just say?").await);
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert!(!request_text(&requests[0]).contains("CORP-SECRET"), "the refused prompt was sent");
    let saved = everything_under(&tmp.path().join("sessions"));
    assert!(!saved.contains("CORP-SECRET"), "the refused prompt was persisted");
    let _ = tx.send(Action::Close).await;
}

/// A hook's context goes into the conversation, not the system prompt.
///
/// Appended to the system prompt, context that changes every turn changed the cached prefix every
/// turn, and the whole conversation was re-billed on each one — measured with a wire log against a
/// real provider. In the transcript it is history: the next request replays it byte for byte.
#[tokio::test]
async fn hook_context_rides_in_the_transcript_not_the_system_prompt() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/prompt-hook").unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider =
        CountingProvider { calls: Arc::new(AtomicUsize::new(0)), requests: requests.clone() };
    let (tx, mut rx) = Engine::spawn(config(store, session_id, provider));

    submit(&tx, &mut rx, "first").await;
    submit(&tx, &mut rx, "second").await;
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].system, requests[1].system, "the system prompt did not move");
    assert!(!requests[1].system.clone().unwrap_or_default().contains("LIVE-CONTEXT"));

    let first_turn = |request: &CompletionRequest| {
        request.messages.iter().find(|m| m.role == Role::User).cloned().expect("a user message")
    };
    let sent = first_turn(&requests[0]);
    assert!(
        sent.content
            .iter()
            .any(|b| matches!(b, ContentBlock::Text(t) if t == "[hook] LIVE-CONTEXT turn 1")),
        "the context rode with its turn: {sent:?}"
    );
    assert_eq!(sent, first_turn(&requests[1]), "and is replayed verbatim on the next turn");
    assert!(request_text(&requests[1]).contains("LIVE-CONTEXT turn 2"));
    let _ = tx.send(Action::Close).await;
}

/// The prompt's `cwd` is where tools run, not the directory the launch named.
///
/// Launched in `packages/app`, Luxe moves to the project root and keeps the launch directory only
/// to pick instruction files. The prompt reported the launch directory anyway, so a real model
/// was told it was in `packages/app` and `pwd` answered with the root.
#[tokio::test]
async fn the_prompt_reports_the_directory_tools_run_in() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/prompt-hook").unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider =
        CountingProvider { calls: Arc::new(AtomicUsize::new(0)), requests: requests.clone() };
    let launched_in = tmp.path().join("packages").join("app");
    let mut config = config(store, session_id, provider);
    config.invoked_from = Some(launched_in.clone());
    let (tx, mut rx) = Engine::spawn(config);

    submit(&tx, &mut rx, "where am I?").await;
    let system = requests.lock().await[0].system.clone().unwrap_or_default();
    let tools_run_in = luxe_core::tool::ToolContext::working_dir();
    assert!(system.contains(&format!("**cwd**: `{}`", tools_run_in.display())), "{system}");
    assert!(!system.contains(&launched_in.display().to_string()), "reported the launch directory");
    let _ = tx.send(Action::Close).await;
}

/// A trivial tool the scripted provider calls.
struct EchoTool;

#[async_trait::async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn schema(&self) -> luxe_core::provider::ToolSchema {
        luxe_core::provider::ToolSchema {
            name: "echo".to_string(),
            description: "echo".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }
    async fn run(&self, _args: serde_json::Value, _cx: &ToolContext) -> ToolResultData {
        ToolResultData {
            content: vec![ContentBlock::Text("ECHO_RESULT".to_string())],
            is_error: false,
        }
    }
}

/// An AfterToolUse hook that feeds context back to the model.
struct FeedbackHook;

#[async_trait::async_trait]
impl Hook for FeedbackHook {
    fn name(&self) -> &'static str {
        "feedback"
    }

    fn interest(&self) -> &[HookPoint] {
        &[HookPoint::AfterToolUse]
    }
    async fn on(&self, _point: HookPoint, _ev: &HookEvent, _cx: &HookCtx) -> HookOutcome {
        HookOutcome::InjectContext("LINT_FEEDBACK_XYZ".to_string())
    }
}

/// Records each request; call 1 emits a tool call, call 2 reports whether it saw the hook feedback.
struct FeedbackProbeProvider {
    calls: Arc<Mutex<Vec<CompletionRequest>>>,
}

#[async_trait::async_trait]
impl Provider for FeedbackProbeProvider {
    fn name(&self) -> &str {
        "feedback-probe"
    }
    async fn stream(
        &self,
        req: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let n = {
            let mut calls = self.calls.lock().await;
            calls.push(req.clone());
            calls.len()
        };
        let events = if n == 1 {
            vec![
                ProviderEvent::ToolCallStart { index: 0, id: "c1".into(), name: "echo".into() },
                ProviderEvent::ToolCallDelta { index: 0, json_delta: "{}".into() },
                ProviderEvent::ToolCallEnd { index: 0 },
                ProviderEvent::Done { reason: StopReason::ToolCall },
            ]
        } else {
            let saw = req.messages.iter().any(|m| {
                m.role == Role::Tool
                    && m.content.iter().any(|b| match b {
                        ContentBlock::ToolResult { content, .. } => content.iter().any(|o| {
                            matches!(o, luxe_core::message::ToolOutput::Text(t) if t.contains("LINT_FEEDBACK_XYZ"))
                        }),
                        _ => false,
                    })
            });
            let reply = if saw { "saw feedback" } else { "MISSING" };
            vec![
                ProviderEvent::TextDelta { index: 0, delta: reply.to_string() },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }
}

#[tokio::test]
async fn after_tool_use_hook_feedback_reaches_the_model() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/feedback-hook").unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));

    let mut hooks = HookRegistry::new();
    hooks.register(Arc::new(FeedbackHook));
    let config = feedback_engine(store, session_id, hooks, requests);
    let (tx, mut rx) = Engine::spawn(config);
    tx.send(Action::SubmitUserMessage {
        content: "go".to_string(),
        images: vec![],
        remediation: None,
    })
    .await
    .unwrap();

    let mut text = String::new();
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
            Ok(Some(Event::TokenDelta { delta, .. })) => text.push_str(&delta),
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            Ok(None) => panic!("channel closed"),
            Err(_) => panic!("timed out"),
        }
    }
    assert!(
        text.contains("saw feedback"),
        "model should see the AfterToolUse hook feedback: {text:?}"
    );
}

/// An engine whose provider calls `echo` once and then reports whether it saw hook feedback.
fn feedback_engine(
    store: JsonlSessionStore,
    session_id: luxe_core::session::SessionId,
    hooks: HookRegistry,
    requests: Arc<Mutex<Vec<CompletionRequest>>>,
) -> EngineConfig {
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(EchoTool));
    EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider: Arc::new(FeedbackProbeProvider { calls: requests.clone() }),
        providers: std::collections::HashMap::new(),
        model: ModelInfo::default(),
        tool_registry: tools,
        hook_registry: hooks,
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
        max_tool_iterations: Some(4),
        startup_notices: Vec::new(),
        action_tx: None,
    }
}

/// A key the process resolved is scrubbed from what a hook adds, at both points where hook context
/// joins the transcript: after a tool, and with the prompt.
struct LeakyHook;

#[async_trait::async_trait]
impl Hook for LeakyHook {
    fn name(&self) -> &'static str {
        "leaky"
    }

    fn interest(&self) -> &[HookPoint] {
        &[HookPoint::UserPromptSubmit, HookPoint::AfterToolUse]
    }

    async fn on(&self, point: HookPoint, _ev: &HookEvent, _cx: &HookCtx) -> HookOutcome {
        HookOutcome::InjectContext(format!("{point:?} says hook-shapeless-secret-31"))
    }
}

#[tokio::test]
async fn hook_context_is_scrubbed_of_resolved_secrets() {
    luxe_core::redact::process_secrets().add("hook-shapeless-secret-31");
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/leaky-hook").unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut hooks = HookRegistry::new();
    hooks.register(Arc::new(LeakyHook));
    let (tx, mut rx) = Engine::spawn(feedback_engine(store, session_id, hooks, requests.clone()));
    submit(&tx, &mut rx, "go").await;
    let requests = requests.lock().await;
    let sent = format!("{:?}", requests.last().expect("a second request").messages);
    assert!(sent.contains("UserPromptSubmit says") && sent.contains("AfterToolUse says"), "{sent}");
    assert!(!sent.contains("hook-shapeless-secret-31"), "{sent}");
    let _ = tx.send(Action::Close).await;
}
