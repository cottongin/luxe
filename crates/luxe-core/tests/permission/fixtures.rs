//! Fixtures — the scripted provider, the counting tool and the engine spawners the gate scenarios share.

use futures::stream;
use futures::stream::BoxStream;
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint};
use luxe_core::message::ContentBlock;
use luxe_core::permission::{
    AskResolution, PermValue, PermissionAction, PermissionConfig, PermissionEngine,
    PermissionReply, PermissionSettings,
};
use luxe_core::provider::{
    CompletionRequest, ModelInfo, Provider, ProviderError, ProviderEvent, StopReason,
};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use luxe_core::tool::{Tool, ToolContext, ToolRegistry, ToolResultData};
use luxe_core::HookRegistry;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

/// A `bash`-named tool that records how many times it actually ran, and with what.
pub(crate) struct CountingBash {
    runs: Arc<AtomicUsize>,
    seen: Arc<Mutex<Vec<serde_json::Value>>>,
}

#[async_trait::async_trait]
impl Tool for CountingBash {
    fn name(&self) -> &str {
        "bash"
    }

    fn schema(&self) -> luxe_core::provider::ToolSchema {
        luxe_core::provider::ToolSchema {
            name: "bash".to_string(),
            description: "Run a command.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "command": { "type": "string" } },
                "required": ["command"],
            }),
        }
    }

    async fn run(&self, args: serde_json::Value, _cx: &ToolContext) -> ToolResultData {
        self.runs.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().await.push(args);
        ToolResultData {
            content: vec![ContentBlock::Text("BASH_RAN".to_string())],
            is_error: false,
        }
    }
}

/// A `BeforeToolUse` hook that answers with whatever the test handed it.
///
/// Stands in for a command hook without a subprocess, so these run on Windows too — what is under
/// test is the engine's half of the contract, not `sh`.
pub(crate) struct ScriptedGuard {
    pub(crate) outcome: HookOutcome,
}

#[async_trait::async_trait]
impl Hook for ScriptedGuard {
    fn name(&self) -> &'static str {
        "scripted-guard"
    }

    fn interest(&self) -> &[HookPoint] {
        &[HookPoint::BeforeToolUse]
    }

    async fn on(&self, _point: HookPoint, _ev: &HookEvent, _cx: &HookCtx) -> HookOutcome {
        self.outcome.clone()
    }
}

/// Scripts each turn: odd provider calls emit a `bash` tool call; even calls end the turn.
pub(crate) struct BashCaller {
    calls: Arc<Mutex<usize>>,
    command: String,
}

impl BashCaller {
    pub(crate) fn running(command: &str) -> Self {
        Self { calls: Arc::new(Mutex::new(0)), command: command.to_string() }
    }
}

#[async_trait::async_trait]
impl Provider for BashCaller {
    fn name(&self) -> &str {
        "bash-caller"
    }

    async fn stream(
        &self,
        _req: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let n = {
            let mut c = self.calls.lock().await;
            *c += 1;
            *c
        };
        let events = if n % 2 == 1 {
            vec![
                ProviderEvent::ToolCallStart {
                    index: 0,
                    id: format!("call_{n}"),
                    name: "bash".to_string(),
                },
                ProviderEvent::ToolCallDelta {
                    index: 0,
                    json_delta: serde_json::json!({ "command": self.command }).to_string(),
                },
                ProviderEvent::ToolCallEnd { index: 0 },
                ProviderEvent::Done { reason: StopReason::ToolCall },
            ]
        } else {
            vec![
                ProviderEvent::TextStart { index: 0 },
                ProviderEvent::TextDelta { index: 0, delta: "done".to_string() },
                ProviderEvent::TextEnd { index: 0 },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }
}

/// Records every hook point it sees, so a test can assert which lifecycle events actually fired.
pub(crate) struct RecordingHook {
    points: Arc<Mutex<Vec<HookPoint>>>,
}

#[async_trait::async_trait]
impl Hook for RecordingHook {
    fn name(&self) -> &'static str {
        "recording"
    }

    fn interest(&self) -> &[HookPoint] {
        &[HookPoint::PermissionRequested, HookPoint::PermissionResolved]
    }

    async fn on(&self, point: HookPoint, _ev: &HookEvent, _cx: &HookCtx) -> HookOutcome {
        self.points.lock().await.push(point);
        HookOutcome::Continue
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn config(
    store: JsonlSessionStore,
    session_id: luxe_core::session::SessionId,
    provider: Arc<dyn Provider + Send + Sync>,
    runs: Arc<AtomicUsize>,
    seen: Arc<Mutex<Vec<serde_json::Value>>>,
    permission: PermissionSettings,
    hooks: HookRegistry,
    permissions_file: Option<std::path::PathBuf>,
) -> EngineConfig {
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(CountingBash { runs, seen }));
    EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider,
        providers: std::collections::HashMap::new(),
        model: ModelInfo::default(),
        tool_registry: tools,
        hook_registry: hooks,
        session_id,
        system_prompt: None,
        todo_store: None,
        compaction_settings: luxe_core::compaction::CompactionSettings::default(),
        permission,
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
        permissions_file,
        max_tool_iterations: Some(8),
        startup_notices: Vec::new(),
        action_tx: None,
    }
}

/// Build a policy with a single global `bash` rule at `action`.
pub(crate) fn bash_policy(action: PermissionAction, ask: AskResolution) -> PermissionSettings {
    let global = PermissionConfig::Keyed(
        [("bash".to_string(), PermValue::Scalar(action))].into_iter().collect(),
    )
    .to_ruleset();
    PermissionSettings { engine: PermissionEngine::new(global, vec![], false), ask }
}

pub(crate) async fn spawn(
    permission: PermissionSettings,
) -> (tokio::sync::mpsc::Sender<Action>, tokio::sync::mpsc::Receiver<Event>, Arc<AtomicUsize>) {
    let (tx, rx, runs, _points) = spawn_with_hooks(permission, HookRegistry::new()).await;
    (tx, rx, runs)
}

/// Spawn behind a `BeforeToolUse` guard, returning the arguments the tool actually received.
pub(crate) async fn spawn_guarded(
    permission: PermissionSettings,
    guard: HookOutcome,
) -> (
    tokio::sync::mpsc::Sender<Action>,
    tokio::sync::mpsc::Receiver<Event>,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<serde_json::Value>>>,
) {
    let mut hooks = HookRegistry::new();
    hooks.register(Arc::new(ScriptedGuard { outcome: guard }));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx, runs, _points) = spawn_inner(permission, hooks, seen.clone()).await;
    (tx, rx, runs, seen)
}

/// Like [`spawn`] but injects a hook registry and returns the recording hook's shared log, so a
/// test can assert which hook points fired during the run.
pub(crate) async fn spawn_with_hooks(
    permission: PermissionSettings,
    hooks: HookRegistry,
) -> (
    tokio::sync::mpsc::Sender<Action>,
    tokio::sync::mpsc::Receiver<Event>,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<HookPoint>>>,
) {
    spawn_inner(permission, hooks, Arc::new(Mutex::new(Vec::new()))).await
}

/// Like [`spawn`], but the scripted model runs `command` instead of `echo hi`.
pub(crate) async fn spawn_running(
    permission: PermissionSettings,
    command: &str,
) -> (tokio::sync::mpsc::Sender<Action>, tokio::sync::mpsc::Receiver<Event>, Arc<AtomicUsize>) {
    let (tx, rx, runs, _points) = spawn_inner_running(
        permission,
        HookRegistry::new(),
        Arc::new(Mutex::new(Vec::new())),
        command,
    )
    .await;
    (tx, rx, runs)
}

pub(crate) async fn spawn_inner(
    permission: PermissionSettings,
    hooks: HookRegistry,
    seen: Arc<Mutex<Vec<serde_json::Value>>>,
) -> (
    tokio::sync::mpsc::Sender<Action>,
    tokio::sync::mpsc::Receiver<Event>,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<HookPoint>>>,
) {
    spawn_inner_running(permission, hooks, seen, "echo hi").await
}

pub(crate) async fn spawn_inner_running(
    permission: PermissionSettings,
    mut hooks: HookRegistry,
    seen: Arc<Mutex<Vec<serde_json::Value>>>,
    command: &str,
) -> (
    tokio::sync::mpsc::Sender<Action>,
    tokio::sync::mpsc::Receiver<Event>,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<HookPoint>>>,
) {
    let points = Arc::new(Mutex::new(Vec::new()));
    hooks.register(Arc::new(RecordingHook { points: points.clone() }));
    let tmp = tempfile::tempdir().unwrap();
    // Leak the tempdir so the session store outlives the test body without a guard field.
    let path = tmp.keep();
    let mut store = JsonlSessionStore::new(path.join("sessions"));
    let session_id = store.create("/tmp/perm-test").unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(BashCaller::running(command));
    let cfg = config(store, session_id, provider, runs.clone(), seen, permission, hooks, None);
    let (tx, rx) = Engine::spawn(cfg);
    (tx, rx, runs, points)
}

/// Drive one turn; auto-answer any permission prompt with `reply`. Returns how many prompts fired.
pub(crate) async fn run_turn_answering(
    tx: &tokio::sync::mpsc::Sender<Action>,
    rx: &mut tokio::sync::mpsc::Receiver<Event>,
    prompt: &str,
    reply: Option<PermissionReply>,
) -> usize {
    tx.send(Action::SubmitUserMessage {
        content: prompt.to_string(),
        images: vec![],
        remediation: None,
    })
    .await
    .unwrap();
    let mut prompts = 0;
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
            Ok(Some(Event::PermissionRequested { id, .. })) => {
                prompts += 1;
                if let Some(reply) = reply {
                    tx.send(Action::RespondPermission { id, reply }).await.unwrap();
                }
            }
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out waiting for turn to end"),
        }
    }
    prompts
}

/// Like [`run_turn_answering`] but never answers, and hands back the tool results the model read.
pub(crate) async fn run_turn_collecting_results(
    tx: &tokio::sync::mpsc::Sender<Action>,
    rx: &mut tokio::sync::mpsc::Receiver<Event>,
    prompt: &str,
) -> (usize, Vec<String>) {
    use luxe_core::message::{ContentBlock, ToolOutput};

    tx.send(Action::SubmitUserMessage {
        content: prompt.to_string(),
        images: vec![],
        remediation: None,
    })
    .await
    .unwrap();
    let (mut prompts, mut results) = (0, Vec::new());
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
            Ok(Some(Event::PermissionRequested { .. })) => prompts += 1,
            Ok(Some(Event::MessageCommitted { message, .. })) => {
                for block in &message.content {
                    let ContentBlock::ToolResult { content, .. } = block else { continue };
                    for output in content {
                        if let ToolOutput::Text(text) = output {
                            results.push(text.clone());
                        }
                    }
                }
            }
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out waiting for turn to end"),
        }
    }
    (prompts, results)
}
