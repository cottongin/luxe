//! Fixtures — the doubles and waiters the engine scenarios share.

use std::collections::HashMap;
use std::sync::Arc;

use futures::stream::{self, BoxStream};
use luxe_core::engine::EngineConfig;
use luxe_core::event::Event;
use luxe_core::hook::{HookCtx, HookEvent, HookOutcome, HookPoint, HookRegistry};
use luxe_core::provider::{
    CompletionRequest, ModelInfo, Provider, ProviderError, ProviderEvent, StopReason,
};
use luxe_core::session::{JsonlSessionStore, SessionId};
use luxe_core::tool::ToolRegistry;
use tokio::sync::RwLock;

/// How long a test waits for an engine event before calling it a hang.
///
/// Generous on purpose. Every assertion here is about *what* the engine emitted, never how fast, and
/// the work behind a guided-review load is a dozen `git` invocations. Five seconds was enough on a
/// developer machine and not on a Windows runner with the whole suite running in parallel, which
/// reads as a product failure and is not one. Still well inside nextest's own 120s cutoff, so a real
/// hang still fails rather than wedging the run.
pub(crate) const EVENT_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// Mock provider that scripts a simple conversation:
/// 1. First turn: emits text "Let me read the file" → tool_call(read) → done
/// 2. Second turn: emits text "File edited successfully" → done
pub(crate) struct MockProvider {
    turn_count: Arc<RwLock<usize>>,
    requests: Arc<RwLock<Vec<CompletionRequest>>>,
}

impl MockProvider {
    pub(crate) fn new() -> Self {
        Self { turn_count: Arc::new(RwLock::new(0)), requests: Arc::new(RwLock::new(Vec::new())) }
    }

    /// Every request it was sent, in order
    pub(crate) fn requests(&self) -> Arc<RwLock<Vec<CompletionRequest>>> {
        self.requests.clone()
    }
}

#[async_trait::async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        self.requests.write().await.push(request);
        let mut turn_count = self.turn_count.write().await;
        *turn_count += 1;
        let current_turn = *turn_count;

        let events = if current_turn == 1 {
            // First turn: text → tool_call(read) → done
            vec![
                ProviderEvent::Start,
                ProviderEvent::TextStart { index: 0 },
                ProviderEvent::TextDelta { index: 0, delta: "Let me read the file".to_string() },
                ProviderEvent::TextEnd { index: 0 },
                ProviderEvent::ToolCallStart {
                    index: 0,
                    id: "call_1".to_string(),
                    name: "read".to_string(),
                },
                ProviderEvent::ToolCallDelta {
                    index: 0,
                    json_delta: r#"{"path": "test.txt"}"#.to_string(),
                },
                ProviderEvent::ToolCallEnd { index: 0 },
                ProviderEvent::Done { reason: StopReason::ToolCall },
            ]
        } else {
            // Second turn: text → done
            vec![
                ProviderEvent::Start,
                ProviderEvent::TextStart { index: 0 },
                ProviderEvent::TextDelta {
                    index: 0,
                    delta: "File edited successfully".to_string(),
                },
                ProviderEvent::TextEnd { index: 0 },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ]
        };

        Ok(Box::pin(stream::iter(events)))
    }
}

/// A simple hook that records all hook points it sees.
pub(crate) struct RecordingHook {
    points: Arc<RwLock<Vec<HookPoint>>>,
}

impl RecordingHook {
    pub(crate) fn new() -> (Self, Arc<RwLock<Vec<HookPoint>>>) {
        let points = Arc::new(RwLock::new(Vec::new()));
        (Self { points: points.clone() }, points)
    }
}

#[async_trait::async_trait]
impl luxe_core::hook::Hook for RecordingHook {
    fn name(&self) -> &'static str {
        "recording"
    }

    fn interest(&self) -> &[HookPoint] {
        &[
            HookPoint::SessionStart,
            HookPoint::SessionEnd,
            HookPoint::UserPromptSubmit,
            HookPoint::AgentStart,
            HookPoint::AgentEnd,
            HookPoint::BeforeToolUse,
            HookPoint::AfterToolUse,
        ]
    }

    async fn on(&self, point: HookPoint, _ev: &HookEvent, _cx: &HookCtx) -> HookOutcome {
        self.points.write().await.push(point);
        HookOutcome::Continue
    }
}

/// Provider that turns the bounded Guide prompt into a complete manifest for integration coverage.
#[derive(Default)]
pub(crate) struct GuideManifestProvider {
    /// Guide passes it was asked for, feedback turns aside
    pub(crate) guides: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl Provider for GuideManifestProvider {
    fn name(&self) -> &str {
        "guide-manifest"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let prompt = request.messages.last().and_then(|message| {
            message.content.iter().find_map(|block| match block {
                luxe_core::message::ContentBlock::Text(text) => Some(text),
                _ => None,
            })
        });
        if let Some(feedback) = prompt.filter(|text| text.starts_with("Guided review feedback")) {
            let review_id = feedback
                .lines()
                .find_map(|line| line.strip_prefix("Review: "))
                .expect("feedback prompt includes review ID");
            let revision_id = feedback
                .lines()
                .find_map(|line| line.strip_prefix("Source revision: "))
                .expect("feedback prompt includes source revision");
            // The bullet now leads with the reviewer-facing label (`R1`), so remediation keys off
            // the explicit durable-ID line rather than parsing display text.
            let finding_id = feedback
                .lines()
                .find_map(|line| line.trim_start().strip_prefix("Finding ID: "))
                .expect("feedback prompt includes finding ID");
            let reply = format!(
                "Applied the focused change.\n```luxe-remediation\n{{\"review_id\":\"{review_id}\",\"revision_id\":\"{revision_id}\",\"findings\":[{{\"finding_id\":\"{finding_id}\",\"status\":\"fixed\",\"note\":\"Updated the tracked source.\"}}]}}\n```"
            );
            return Ok(Box::pin(stream::iter(vec![
                ProviderEvent::TextDelta { index: 0, delta: reply },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ])));
        }
        self.guides.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let manifest = prompt
            .and_then(|text| text.rsplit_once('\n').map(|(_, input)| input))
            .and_then(|input| serde_json::from_str::<serde_json::Value>(input).ok())
            .map(|input| {
                let item_ids = input["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|item| item["item_id"].as_str().map(str::to_owned))
                    .collect::<Vec<_>>();
                serde_json::json!({
                    "chapters": [{
                        "title": "Model Guide",
                        "rationale": "Groups the immutable items supplied to the model.",
                        "kind": "core",
                        "item_ids": item_ids,
                    }]
                })
                .to_string()
            })
            .unwrap_or_else(|| "{}".into());
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::TextDelta { index: 0, delta: manifest },
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ])))
    }
}

/// An engine on `project_root` that answers guides and feedback with [`GuideManifestProvider`].
pub(crate) fn review_engine_config(
    project_root: &std::path::Path,
    session_id: SessionId,
) -> EngineConfig {
    let store = JsonlSessionStore::new(project_root.to_path_buf());
    EngineConfig {
        guided_review_project_root: Some(project_root.to_path_buf()),
        ..base_config(store, session_id, Arc::new(GuideManifestProvider::default()))
    }
}

/// The first event `wanted` picks out of `events`, within the wait.
pub(crate) async fn first<T>(
    events: &mut tokio::sync::mpsc::Receiver<Event>,
    what: &str,
    mut wanted: impl FnMut(Event) -> Option<T>,
) -> T {
    tokio::time::timeout(EVENT_WAIT, async {
        loop {
            match events.recv().await {
                Some(event) => {
                    if let Some(found) = wanted(event) {
                        return found;
                    }
                }
                None => panic!("the event stream closed before {what}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

/// The unstaged empty-tree source the review-feedback tests submit against.
pub(crate) fn review_source() -> luxe_core::guided_review::ReviewSource {
    use luxe_core::guided_review::{ReviewBase, ReviewRevisionId, ReviewSource};
    ReviewSource {
        base: ReviewBase::EmptyTree,
        content_digest: ReviewRevisionId::from_canonical_bytes(b"review"),
        includes_staged: false,
        includes_unstaged: true,
        included_untracked_files: 0,
        captured_at_ms: 42,
        uncovered_paths: Vec::new(),
    }
}

pub(crate) struct RemediationProvider {
    pub(crate) reply: String,
}

#[async_trait::async_trait]
impl Provider for RemediationProvider {
    fn name(&self) -> &str {
        "remediation"
    }

    async fn stream(
        &self,
        _request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::TextDelta { index: 0, delta: self.reply.clone() },
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ])))
    }
}

/// An engine on `store` with every optional piece off: no tools, hooks, reviewer, compaction, cache
/// warming or project root, and the default model.
///
/// A scenario overrides the fields it is about and leaves the rest, so what a test configures is
/// what it reads. A scenario that needs compaction or cache warming asks for it by name.
pub(crate) fn base_config(
    store: JsonlSessionStore,
    session_id: SessionId,
    provider: Arc<dyn Provider + Send + Sync>,
) -> EngineConfig {
    EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider,
        providers: HashMap::new(),
        model: ModelInfo::default(),
        tool_registry: ToolRegistry::new(),
        hook_registry: HookRegistry::new(),
        session_id,
        system_prompt: None,
        todo_store: None,
        compaction_settings: luxe_core::compaction::CompactionSettings {
            enabled: false,
            ..luxe_core::compaction::CompactionSettings::default()
        },
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
        max_tool_iterations: luxe_core::engine::DEFAULT_MAX_TOOL_ITERATIONS,
        startup_notices: Vec::new(),
        action_tx: None,
    }
}

/// A `read` the test can count: it answers every call with the same text and keeps what it was
/// asked.
///
/// Registered where a scenario says a tool ran, so the claim is something to assert — an engine
/// that never dispatched the call leaves [`Self::invocations`] at zero, and one that dispatched it
/// but lost the answer leaves the next request without [`WITNESS_OUTPUT`].
pub(crate) struct WitnessReadTool {
    calls: std::sync::Mutex<Vec<serde_json::Value>>,
}

/// What [`WitnessReadTool`] says every time
pub(crate) const WITNESS_OUTPUT: &str = "witness read: Hello, World!";

impl WitnessReadTool {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self { calls: std::sync::Mutex::new(Vec::new()) })
    }

    /// The arguments of each call that ran
    pub(crate) fn invocations(&self) -> Vec<serde_json::Value> {
        self.calls.lock().expect("no panic holds this lock").clone()
    }
}

#[async_trait::async_trait]
impl luxe_core::tool::Tool for WitnessReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn schema(&self) -> luxe_core::provider::ToolSchema {
        luxe_core::provider::ToolSchema {
            name: "read".to_string(),
            description: "Read a file.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
            }),
        }
    }

    async fn run(
        &self,
        args: serde_json::Value,
        _cx: &luxe_core::tool::ToolContext,
    ) -> luxe_core::tool::ToolResultData {
        self.calls.lock().expect("no panic holds this lock").push(args);
        luxe_core::tool::ToolResultData {
            content: vec![luxe_core::message::ContentBlock::Text(WITNESS_OUTPUT.to_string())],
            is_error: false,
        }
    }
}

/// Events until `stop` says so, then all of them; a panic naming `waiting_for` when the wait runs
/// out or the stream closes first.
///
/// Fails, rather than returning what it has: a drain that gives up quietly lets the assertions
/// after it pass on whatever had arrived by then.
pub(crate) async fn events_until(
    events: &mut tokio::sync::mpsc::Receiver<Event>,
    waiting_for: &str,
    stop: impl Fn(&Event) -> bool,
) -> Vec<Event> {
    let mut seen = Vec::new();
    loop {
        match tokio::time::timeout(EVENT_WAIT, events.recv()).await {
            Ok(Some(event)) => {
                let done = stop(&event);
                seen.push(event);
                if done {
                    return seen;
                }
            }
            Ok(None) => panic!("the event stream closed before {waiting_for}"),
            Err(_) => panic!("timed out waiting for {waiting_for}; got {} events", seen.len()),
        }
    }
}

/// Every event up to and including the turn's end.
pub(crate) async fn events_until_turn_end(
    events: &mut tokio::sync::mpsc::Receiver<Event>,
) -> Vec<Event> {
    events_until(events, "the turn to end", |event| matches!(event, Event::TurnEnded { .. })).await
}

/// Wait until the engine has actually *taken* the cancel, rather than sleeping and hoping.
///
/// `Action::Cancel` travels the same channel as every other action, so "sent" and "in effect" are
/// two different moments. The engine announces the second one with a `cancelled` state snapshot,
/// which is the only sound synchronization point a test has.
pub(crate) async fn await_cancel_taken(events: &mut tokio::sync::mpsc::Receiver<Event>) {
    events_until(
        events,
        "the cancel to take effect",
        |event| matches!(event, Event::StateChanged(state) if state.status == "cancelled"),
    )
    .await;
}

/// The ids of every tool call in `messages`, and of every call something answered.
pub(crate) fn pairing(messages: &[luxe_core::message::Message]) -> (Vec<String>, Vec<String>) {
    use luxe_core::message::ContentBlock;
    let mut calls = Vec::new();
    let mut results = Vec::new();
    for block in messages.iter().flat_map(|message| &message.content) {
        match block {
            ContentBlock::ToolCall { id, .. } => calls.push(id.clone()),
            ContentBlock::ToolResult { call_id, .. } => results.push(call_id.clone()),
            _ => {}
        }
    }
    (calls, results)
}

/// The text of the result that answers `call_id`; panics when nothing does.
pub(crate) fn result_text(messages: &[luxe_core::message::Message], call_id: &str) -> String {
    use luxe_core::message::{ContentBlock, ToolOutput};
    for block in messages.iter().flat_map(|message| &message.content) {
        if let ContentBlock::ToolResult { call_id: id, content, .. } = block {
            if id == call_id {
                return content
                    .iter()
                    .map(|output| match output {
                        ToolOutput::Text(text) => text.clone(),
                        ToolOutput::Image { .. } => "[image]".to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
            }
        }
    }
    panic!("no result for {call_id}");
}
