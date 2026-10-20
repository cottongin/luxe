//! Prompt-cache stability — what a session sends must be an *append* to what it sent last time.
//!
//! Every provider that caches does it by prefix, so the request is only cacheable as far as its
//! first changed byte. Everything after that is re-read and re-billed, silently: no error, no
//! warning, just a bigger invoice and a slower first token. Two halves, and both have cost real
//! money here.
//!
//! - **The system prompt stays byte-identical.** Volatile state (a git dirty flag, a todo counter)
//!   must not live in it, the one-time resume briefing is appended rather than prepended, and a
//!   hook's per-turn context stays out of it altogether — it rides in the conversation.
//! - **The transcript only grows.** Editing history in flight — re-truncating an old tool result,
//!   dropping a stale image — rewrites the middle of the array, and a real session spent 71% of
//!   its cost that way while its system prompt sat there perfectly stable.

#![allow(clippy::unwrap_used)]

use futures::stream::{self, BoxStream};
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint};
use luxe_core::message::{ContentBlock, Message};
use luxe_core::provider::{
    CompletionRequest, ModelInfo, Provider, ProviderError, ProviderEvent, StopReason, ToolSchema,
};
use luxe_core::session::{JsonlSessionStore, SessionId, SessionStore};
use luxe_core::todo::{Todo, TodoStatus, TodoStore};
use luxe_core::tool::{Tool, ToolContext, ToolRegistry, ToolResultData};
use luxe_core::HookRegistry;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Records the system prompt of every provider call.
struct CapturingProvider {
    prompts: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl Provider for CapturingProvider {
    fn name(&self) -> &str {
        "capturing"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        self.prompts.lock().await.push(request.system.clone().unwrap_or_default());
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::TextDelta { index: 0, delta: "ok".to_string() },
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ])))
    }
}

/// A tool whose output is big enough to matter, so a handful of calls builds the kind of
/// tool-result-heavy transcript a coding session really sends.
struct BulkyTool;

#[async_trait::async_trait]
impl Tool for BulkyTool {
    fn name(&self) -> &str {
        "read"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "read".to_string(),
            description: "Read a file.".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }

    async fn run(&self, _args: serde_json::Value, _cx: &ToolContext) -> ToolResultData {
        ToolResultData {
            content: vec![ContentBlock::Text("source line\n".repeat(1_200))],
            is_error: false,
        }
    }
}

/// Calls the bulky tool for a fixed number of iterations, recording the transcript it was handed
/// each time.
struct ToolLoopProvider {
    transcripts: Arc<Mutex<Vec<Vec<Message>>>>,
    iterations: usize,
}

#[async_trait::async_trait]
impl Provider for ToolLoopProvider {
    fn name(&self) -> &str {
        "tool-loop"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let call = {
            let mut transcripts = self.transcripts.lock().await;
            transcripts.push(request.messages.clone());
            transcripts.len()
        };
        if call >= self.iterations {
            return Ok(Box::pin(stream::iter(vec![
                ProviderEvent::TextDelta { index: 0, delta: "done".to_string() },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ])));
        }
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::ToolCallStart {
                index: 0,
                id: format!("call-{call}"),
                name: "read".to_string(),
            },
            // Varying arguments: an identical call repeated is a doom loop, and the engine is
            // right to refuse it.
            ProviderEvent::ToolCallDelta {
                index: 0,
                json_delta: format!(r#"{{"path": "file-{call}.rs"}}"#),
            },
            ProviderEvent::ToolCallEnd { index: 0 },
            ProviderEvent::Done { reason: StopReason::ToolCall },
        ])))
    }
}

/// A hook that injects per-turn context, the way a loop or a project hook does.
struct InjectingHook;

#[async_trait::async_trait]
impl Hook for InjectingHook {
    fn name(&self) -> &'static str {
        "injecting"
    }

    fn interest(&self) -> &[HookPoint] {
        &[HookPoint::BeforeAgentStart]
    }

    async fn on(&self, _point: HookPoint, _event: &HookEvent, _cx: &HookCtx) -> HookOutcome {
        HookOutcome::InjectContext("## Loop state\n\nIteration 7 of the current phase.".to_string())
    }
}

fn config(
    store: JsonlSessionStore,
    session_id: luxe_core::session::SessionId,
    todo_root: std::path::PathBuf,
    provider: Arc<dyn Provider + Send + Sync>,
    hooks: HookRegistry,
) -> EngineConfig {
    EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider,
        providers: std::collections::HashMap::new(),
        model: ModelInfo::default(),
        tool_registry: ToolRegistry::new(),
        hook_registry: hooks,
        session_id,
        system_prompt: None,
        todo_store: Some(TodoStore::new(todo_root)),
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

/// Run `turns` user messages through one engine, returning the system prompt sent for each.
async fn prompts_for_turns(
    root: &std::path::Path,
    hooks: HookRegistry,
    turns: usize,
    between: impl Fn(usize),
) -> Vec<String> {
    let mut store = JsonlSessionStore::new(root.join("sessions"));
    let sid = store.create("/tmp/test").unwrap();
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(CapturingProvider { prompts: prompts.clone() });
    let (tx, mut rx) = Engine::spawn(config(store, sid, root.join("todos"), provider, hooks));

    for turn in 0..turns {
        tx.send(Action::SubmitUserMessage {
            content: format!("turn {turn}"),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
                Ok(Some(Event::TurnEnded { .. })) => break,
                Ok(Some(_)) => {}
                Ok(None) => panic!("event channel closed"),
                Err(_) => panic!("timed out"),
            }
        }
        between(turn);
    }
    tx.send(Action::Close).await.ok();
    let captured = prompts.lock().await.clone();
    captured
}

#[tokio::test]
async fn the_system_prompt_is_byte_identical_across_turns() {
    let tmp = tempfile::tempdir().unwrap();
    let prompts = prompts_for_turns(tmp.path(), HookRegistry::new(), 3, |_| {}).await;

    assert_eq!(prompts.len(), 3);
    assert_eq!(prompts[0], prompts[1], "turn 2 must reuse turn 1's cached prefix");
    assert_eq!(prompts[1], prompts[2], "and turn 3 must reuse turn 2's");
}

/// Working the checklist must not re-write the prefix.
///
/// The prompt used to carry a coarse "## Current Task" pointer, kept deliberately counter-free for
/// exactly this reason; nothing about the live list reaches the prompt at all now, and this is what
/// holds that line.
#[tokio::test]
async fn mutating_todos_does_not_change_the_cached_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let todo_root = tmp.path().join("todos");
    let todo = |id: u32, status: TodoStatus| Todo {
        id,
        subject: format!("step {id}"),
        status,
        active_form: None,
        blocked_by: vec![],
    };

    // The engine mints its own session id, so seed whichever session it opens.
    let store_root = tmp.path().join("sessions");
    let mut store = JsonlSessionStore::new(store_root.clone());
    let sid = store.create("/tmp/test").unwrap();
    let todos = TodoStore::new(todo_root.clone());
    todos.write(&sid.0, &[todo(1, TodoStatus::Pending), todo(2, TodoStatus::Pending)]).unwrap();

    let prompts = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(CapturingProvider { prompts: prompts.clone() });
    let (tx, mut rx) = Engine::spawn(config(
        JsonlSessionStore::new(store_root),
        sid.clone(),
        todo_root.clone(),
        provider,
        HookRegistry::new(),
    ));

    for turn in 0..2 {
        tx.send(Action::SubmitUserMessage {
            content: format!("turn {turn}"),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
                Ok(Some(Event::TurnEnded { .. })) => break,
                Ok(Some(_)) => {}
                Ok(None) => panic!("event channel closed"),
                Err(_) => panic!("timed out"),
            }
        }
        // Complete one todo between turns — the exact churn a live counter would leak.
        TodoStore::new(todo_root.clone())
            .write(&sid.0, &[todo(1, TodoStatus::Completed), todo(2, TodoStatus::Pending)])
            .unwrap();
    }
    tx.send(Action::Close).await.ok();

    let captured = prompts.lock().await.clone();
    assert_eq!(captured.len(), 2);
    assert!(!captured[0].contains("step 1"), "no todo subject reaches the prompt");
    assert_eq!(
        captured[0], captured[1],
        "completing a todo must not rewrite the prompt:\n{}\n---\n{}",
        captured[0], captured[1]
    );
}

/// The other half of the prefix, and the expensive one.
///
/// A tool-heavy turn used to re-truncate one more stale result on every iteration, which moved the
/// first changed byte into the middle of the transcript and re-billed everything after it as a
/// cache write. Nothing above it moved, so no test and no audit noticed — the invoice did, at four
/// times what the work was worth.
///
/// Forty iterations against a window with room in it: not one of them may edit history.
#[tokio::test]
async fn the_transcript_only_ever_grows_within_a_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let sid = store.create("/tmp/test").unwrap();
    let transcripts = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(ToolLoopProvider { transcripts: transcripts.clone(), iterations: 40 });

    let mut config = config(store, sid, tmp.path().join("todos"), provider, HookRegistry::new());
    config.tool_registry.register(Arc::new(BulkyTool));
    config.max_tool_iterations = Some(40);
    // The shape that was really being billed: a current Claude window, nowhere near full.
    config.model =
        ModelInfo { context_window: 1_000_000, max_tokens: 64_000, ..ModelInfo::default() };
    let (tx, mut rx) = Engine::spawn(config);

    tx.send(Action::SubmitUserMessage {
        content: "read everything".to_string(),
        images: vec![],
        remediation: None,
    })
    .await
    .unwrap();
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(20), rx.recv()).await {
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out"),
        }
    }
    tx.send(Action::Close).await.ok();

    let captured = transcripts.lock().await.clone();
    assert_eq!(captured.len(), 40, "the loop has to be long enough to matter");
    let rendered: Vec<Vec<String>> = captured
        .iter()
        .map(|messages| messages.iter().map(|m| format!("{m:?}")).collect())
        .collect();

    let rewritten: Vec<usize> = rendered
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| pair[0].iter().zip(pair[1].iter()).any(|(a, b)| a != b))
        .map(|(at, _)| at + 1)
        .collect();
    assert!(
        rewritten.is_empty(),
        "calls {rewritten:?} of {} rewrote history the provider had already cached",
        rendered.len()
    );
    // And every call really did append, rather than the loop having quietly stopped early.
    let lengths: Vec<usize> = rendered.iter().map(Vec::len).collect();
    assert!(lengths.windows(2).all(|pair| pair[1] > pair[0]), "{lengths:?}");
}

/// A mid-turn reminder has to reach the model without touching a byte the provider has cached.
///
/// The checklist drifts one way — a model opens one, works, finishes, and never marks anything —
/// and the only place a fact can be added mid-turn for free is a tool result, because that message
/// was going to be appended anyway. Anything else (re-editing an old result, moving the system
/// prompt) rewrites the middle of the array and re-bills everything after it at the write rate.
#[tokio::test]
async fn a_plan_reminder_lands_in_a_tool_result_without_rewriting_history() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let sid = store.create("/tmp/test").unwrap();
    let todo_root = tmp.path().join("todos");
    TodoStore::new(todo_root.clone())
        .write(
            sid.0.as_str(),
            &[Todo {
                id: 1,
                subject: "Wire the composing event".to_string(),
                status: TodoStatus::InProgress,
                active_form: None,
                blocked_by: Vec::new(),
            }],
        )
        .unwrap();

    let transcripts = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(ToolLoopProvider { transcripts: transcripts.clone(), iterations: 12 });
    let mut hooks = HookRegistry::new();
    hooks.register(Arc::new(luxe_core::hooks::PlanReminderHook::new(TodoStore::new(
        todo_root.clone(),
    ))));

    let mut config = config(store, sid, todo_root, provider, hooks);
    config.tool_registry.register(Arc::new(BulkyTool));
    config.max_tool_iterations = Some(12);
    config.model =
        ModelInfo { context_window: 1_000_000, max_tokens: 64_000, ..ModelInfo::default() };
    let (tx, mut rx) = Engine::spawn(config);
    run_turn(&tx, &mut rx, "read everything").await;
    tx.send(Action::Close).await.ok();

    let captured = transcripts.lock().await.clone();
    let last = captured.last().expect("the loop ran");
    let reminded: Vec<usize> = last
        .iter()
        .enumerate()
        .filter(|(_, message)| {
            message.content.iter().any(|block| match block {
                ContentBlock::ToolResult { content, .. } => content.iter().any(|out| {
                    matches!(out, luxe_core::message::ToolOutput::Text(text)
                        if text.contains("Plan check:"))
                }),
                _ => false,
            })
        })
        .map(|(at, _)| at)
        .collect();
    assert_eq!(reminded.len(), 1, "one reminder over eleven calls, not one per call");

    let note = format!("{:?}", last[reminded[0]]);
    assert!(note.contains("10 tool calls since"), "{note}");
    assert!(note.contains("#1 [in progress] Wire the composing event"), "{note}");

    // And it cost nothing: it was part of the result the moment that result was committed, so no
    // later request rewrote a message an earlier one had already been billed for.
    let rendered: Vec<Vec<String>> = captured
        .iter()
        .map(|messages| messages.iter().map(|m| format!("{m:?}")).collect())
        .collect();
    let rewritten: Vec<usize> = rendered
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| pair[0].iter().zip(pair[1].iter()).any(|(a, b)| a != b))
        .map(|(at, _)| at + 1)
        .collect();
    assert!(rewritten.is_empty(), "calls {rewritten:?} rewrote a cached prefix");
}

/// Injected context is per-turn by nature, so it stays out of the system prompt entirely.
///
/// Appending it only protected the prompt *above* it. The conversation sits after the system
/// prompt in every request, so context that changed between turns still moved the prefix in front
/// of all of it — and a real session re-billed its whole history on every turn. It is committed
/// with the turn's user message instead (`integration_prompt_hook` pins where it lands).
#[tokio::test]
async fn injected_context_stays_out_of_the_system_prompt() {
    let tmp = tempfile::tempdir().unwrap();
    let mut hooks = HookRegistry::new();
    hooks.register(Arc::new(InjectingHook));
    let prompts = prompts_for_turns(tmp.path(), hooks, 2, |_| {}).await;

    assert_eq!(prompts[0], prompts[1], "the system prompt did not move between turns");
    assert!(!prompts[0].contains("Iteration 7"), "the hook's context is not in it");
    assert!(prompts[0].starts_with("## Role"), "the stable charter opens the prompt");
}

/// A tool named in the prompt's shell-alternatives guidance, so the `## Tools` section can be
/// caught moving when it becomes callable.
struct NamedTool(&'static str, &'static str);

#[async_trait::async_trait]
impl Tool for NamedTool {
    fn name(&self) -> &str {
        self.0
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.0.to_string(),
            description: self.1.to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }

    async fn run(&self, _args: serde_json::Value, _cx: &ToolContext) -> ToolResultData {
        ToolResultData { content: vec![ContentBlock::Text("ok".into())], is_error: false }
    }
}

/// One provider call, as the tiering decided it: the prompt, which tools it named, and the tool
/// calls its transcript carried.
#[derive(Clone, Debug)]
struct Sent {
    system: String,
    tools: Vec<String>,
    deferred: Vec<String>,
    calls: Vec<String>,
}

/// The names of the tool calls in `messages`, in order.
fn call_names(messages: &[Message]) -> Vec<String> {
    let blocks = messages.iter().flat_map(|message| &message.content);
    blocks
        .filter_map(|block| match block {
            ContentBlock::ToolCall { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// Asks for `activate_tools {"names": ["test"]}` on its first call when `activate` is set, and
/// answers in text otherwise, recording every request.
struct ActivatingProvider {
    sent: Arc<Mutex<Vec<Sent>>>,
    activate: bool,
}

#[async_trait::async_trait]
impl Provider for ActivatingProvider {
    fn name(&self) -> &str {
        "activating"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let names = |schemas: &[ToolSchema]| schemas.iter().map(|s| s.name.clone()).collect();
        let first = {
            let mut sent = self.sent.lock().await;
            sent.push(Sent {
                system: request.system.clone().unwrap_or_default(),
                tools: names(&request.tools),
                deferred: names(&request.deferred_tools),
                calls: call_names(&request.messages),
            });
            sent.len() == 1
        };
        if !(first && self.activate) {
            return Ok(Box::pin(stream::iter(vec![
                ProviderEvent::TextDelta { index: 0, delta: "ok".to_string() },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ])));
        }
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::ToolCallStart {
                index: 0,
                id: "call-activate".to_string(),
                name: luxe_core::tool::ACTIVATE_TOOLS_NAME.to_string(),
            },
            ProviderEvent::ToolCallDelta { index: 0, json_delta: r#"{"names":["test"]}"#.into() },
            ProviderEvent::ToolCallEnd { index: 0 },
            ProviderEvent::Done { reason: StopReason::ToolCall },
        ])))
    }
}

/// Core `bash` and `read` with a deferred `test`, tiered the way `luxe-cli` wires a real session.
/// `bash` is what makes the prompt's guidance section render at all, and `test` is one of the
/// tools that section names, so an activation that reached the prompt would show up twice.
fn tiered_tools() -> ToolRegistry {
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(BulkyTool));
    tools.register(Arc::new(NamedTool("bash", "Run a command.")));
    tools.register(Arc::new(NamedTool("test", "Run the suite. Prefer it over bash.")));
    // Answers `ok` the way the real one does when every name it was given is deferred
    tools.register(Arc::new(NamedTool(luxe_core::tool::ACTIVATE_TOOLS_NAME, "Load tools.")));
    tools.set_tiers(std::collections::HashSet::from(["read".to_string(), "bash".to_string()]));
    tools
}

fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

/// Activating a deferred tool adds it to `tools`, and moves nothing above it or in the deferred
/// list an in-place adapter loads it from.
///
/// The break the prompt half catches, found with `luxe wire` on a loop run: both the `## Tools`
/// guidance and the on-demand catalog were derived from the *activated* set, so the first
/// `activate_tools` call rewrote the system prompt underneath a cached prefix, and every token after
/// a change is re-read at the write rate. Lazy tools exist to shrink the request; a prompt that
/// moves with them gives the saving straight back.
#[tokio::test]
async fn activating_a_tool_does_not_rewrite_the_prompt_above_it() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let sid = store.create("/tmp/test").unwrap();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(ActivatingProvider { sent: sent.clone(), activate: true });

    let mut config = config(store, sid, tmp.path().join("todos"), provider, HookRegistry::new());
    config.tool_registry = tiered_tools();
    let (tx, mut rx) = Engine::spawn(config);
    run_turn(&tx, &mut rx, "before").await;
    run_turn(&tx, &mut rx, "after").await;
    tx.send(Action::Close).await.ok();

    let sent = sent.lock().await.clone();
    assert_eq!(sent.len(), 3, "the activation, its follow-up, and the next turn: {sent:?}");
    let prompt = &sent[0].system;
    assert!(prompt.contains("**test**"), "the catalog names it up front: {prompt}");
    assert!(prompt.contains("## Tools"), "and the guidance section is there to move: {prompt}");
    assert!(sent.iter().all(|call| &call.system == prompt), "the prompt never moves");

    assert_eq!(sent[0].tools, strings(&["activate_tools", "bash", "read"]));
    for call in &sent[1..] {
        assert_eq!(
            call.tools,
            strings(&["activate_tools", "bash", "read", "test"]),
            "the transcript's activation reaches every later request"
        );
    }
    assert!(
        sent.iter().all(|call| call.deferred == strings(&["test"])),
        "the deferred list is the same before and after: {sent:?}"
    );
}

/// A resumed session loads what its own history activated, and nothing else.
///
/// The activated set used to live in memory, so a resume started from the core tier while the
/// transcript showed the model a tool it had loaded, and a `/new` session in the same process
/// inherited the last one's activations.
#[tokio::test]
async fn a_resumed_session_keeps_what_its_history_activated() {
    let tmp = tempfile::tempdir().unwrap();
    let sessions = tmp.path().join("sessions");
    let mut store = JsonlSessionStore::new(sessions.clone());
    let sid = store.create("/tmp/test").unwrap();
    let first = tiered_turn(store, &sid, tmp.path(), true).await;
    assert_eq!(first.len(), 2, "the activation and its follow-up");

    let resumed = tiered_turn(JsonlSessionStore::new(sessions), &sid, tmp.path(), false).await;
    assert_eq!(resumed.len(), 1, "one call for the resumed turn: {resumed:?}");
    assert_eq!(resumed[0].tools, strings(&["activate_tools", "bash", "read", "test"]));
}

/// A compacted session still carries the tool its summarized history loaded, restated after the
/// summary.
///
/// The break this catches: the request builder dropping that restatement, which unloads the tool
/// on an adapter that loads tools in place while the prompt promises it stays loaded.
#[tokio::test]
async fn a_compacted_session_restates_what_its_summary_replaced() {
    let tmp = tempfile::tempdir().unwrap();
    let sessions = tmp.path().join("sessions");
    let mut store = JsonlSessionStore::new(sessions.clone());
    let sid = store.create("/tmp/test").unwrap();
    tiered_turn(store, &sid, tmp.path(), true).await;

    // A summary standing for everything before the newest entry, the activation included
    let mut store = JsonlSessionStore::new(sessions);
    let leaf = luxe_core::session::latest_leaf(&store.load(&sid).unwrap()).unwrap();
    store.compact(&sid, Some(leaf.clone()), "They loaded test.", &leaf, 0).unwrap();
    let resumed = tiered_turn(store, &sid, tmp.path(), false).await;

    assert_eq!(resumed.len(), 1, "one call for the resumed turn: {resumed:?}");
    assert_eq!(resumed[0].calls, strings(&["activate_tools"]), "only the restatement");
    assert_eq!(resumed[0].tools, strings(&["activate_tools", "bash", "read", "test"]));
}

/// One turn in a fresh engine on `store` over the tiered tools, as a new process would run it,
/// returning every provider call.
///
/// With `activate`, the provider loads `test` on its first call; without, it never activates
/// anything itself and the engine resumes `sid` first, so what is loaded came from the history.
async fn tiered_turn(
    store: JsonlSessionStore,
    sid: &SessionId,
    dir: &std::path::Path,
    activate: bool,
) -> Vec<Sent> {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(ActivatingProvider { sent: sent.clone(), activate });
    let mut run = config(store, sid.clone(), dir.join("todos"), provider, HookRegistry::new());
    run.tool_registry = tiered_tools();
    let (tx, mut rx) = Engine::spawn(run);
    if !activate {
        tx.send(Action::LoadSession(sid.to_string())).await.unwrap();
    }
    run_turn(&tx, &mut rx, if activate { "activate" } else { "resumed" }).await;
    tx.send(Action::Close).await.ok();
    let calls = sent.lock().await.clone();
    calls
}

/// Submit one message and wait for the turn it starts to end.
async fn run_turn(
    tx: &tokio::sync::mpsc::Sender<Action>,
    rx: &mut tokio::sync::mpsc::Receiver<Event>,
    content: &str,
) {
    tx.send(Action::SubmitUserMessage {
        content: content.to_string(),
        images: vec![],
        remediation: None,
    })
    .await
    .unwrap();
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out"),
        }
    }
}
