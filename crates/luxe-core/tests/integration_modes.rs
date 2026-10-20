//! Modes, wired: plan mode is held at dispatch, told in the transcript, and moves no cached prefix.
//!
//! Drives the real engine with a scripted model. What is asserted is what reached the provider and
//! what the tools actually did, because a mode that only *says* it is planning is the failure this
//! design exists to rule out.

#![allow(clippy::unwrap_used)]

mod common;

use std::collections::VecDeque;
use std::sync::Arc;

use futures::stream;
use futures::stream::BoxStream;
use luxe_core::engine::{Action, Engine, EngineConfig, OnDemandReview};
use luxe_core::event::{Event, RequestedReviewVerdict};
use luxe_core::message::{ContentBlock, Role};
use luxe_core::mode::{Mode, Modes};
use luxe_core::permission::{AskResolution, PermissionEngine, PermissionSettings};
use luxe_core::phase_gate::PhaseReviewContext;
use luxe_core::phase_review::{
    PhaseReviewOutcome, PhaseReviewRequest, PhaseReviewer, ReviewReason, ReviewedFile,
};
use luxe_core::provider::{
    CompletionRequest, ModelInfo, ModelRef, Provider, ProviderError, ProviderEvent, StopReason,
    ThinkingLevel, ToolSchema,
};
use luxe_core::session::{JsonlSessionStore, SessionId, SessionStore};
use luxe_core::tool::{Tool, ToolContext, ToolRegistry, ToolResultData};
use luxe_core::HookRegistry;
use tokio::sync::Mutex;

// === The scripted model ===

/// One thing the model does on a call.
#[derive(Clone)]
enum Step {
    Call(&'static str, serde_json::Value),
    Say(&'static str),
}

/// Plays its steps one call at a time, then says "done". Keeps every request.
struct Scripted {
    steps: Mutex<VecDeque<Step>>,
    requests: Arc<Mutex<Vec<CompletionRequest>>>,
}

#[async_trait::async_trait]
impl Provider for Scripted {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let count = {
            let mut requests = self.requests.lock().await;
            requests.push(request);
            requests.len()
        };
        let step = self.steps.lock().await.pop_front().unwrap_or(Step::Say("done"));
        let usage = luxe_core::provider::Usage {
            prompt_tokens: 100,
            completion_tokens: 10,
            total_tokens: 110,
            ..Default::default()
        };
        let events = match step {
            Step::Call(name, args) => vec![
                ProviderEvent::ToolCallStart {
                    index: 0,
                    id: format!("call_{count}"),
                    name: name.to_string(),
                },
                ProviderEvent::ToolCallDelta { index: 0, json_delta: args.to_string() },
                ProviderEvent::ToolCallEnd { index: 0 },
                ProviderEvent::Usage(usage),
                ProviderEvent::Done { reason: StopReason::ToolCall },
            ],
            Step::Say(text) => vec![
                ProviderEvent::TextStart { index: 0 },
                ProviderEvent::TextDelta { index: 0, delta: text.to_string() },
                ProviderEvent::TextEnd { index: 0 },
                ProviderEvent::Usage(usage),
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ],
        };
        Ok(Box::pin(stream::iter(events)))
    }
}

/// A tool that records the arguments of every call that actually ran.
struct Recording {
    name: &'static str,
    ran: Arc<Mutex<Vec<serde_json::Value>>>,
}

#[async_trait::async_trait]
impl Tool for Recording {
    fn name(&self) -> &str {
        self.name
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name.to_string(),
            description: format!("The {} tool.", self.name),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "file_path": { "type": "string" },
                    "content": { "type": "string" },
                    "command": { "type": "string" },
                },
            }),
        }
    }

    async fn run(&self, args: serde_json::Value, _cx: &ToolContext) -> ToolResultData {
        self.ran.lock().await.push(args);
        ToolResultData { content: vec![ContentBlock::Text("ok".into())], is_error: false }
    }
}

// === The engine under test ===

struct Harness {
    actions: tokio::sync::mpsc::Sender<Action>,
    events: tokio::sync::mpsc::Receiver<Event>,
    requests: Arc<Mutex<Vec<CompletionRequest>>>,
    ran: std::collections::HashMap<&'static str, Arc<Mutex<Vec<serde_json::Value>>>>,
    session_id: SessionId,
    _project: tempfile::TempDir,
}

#[derive(Default)]
struct Setup {
    steps: Vec<Step>,
    modes: Modes,
    reviewer: Option<OnDemandReview>,
    sessions: Option<std::path::PathBuf>,
    session_id: Option<SessionId>,
}

fn session_model() -> ModelInfo {
    ModelInfo {
        r#ref: ModelRef::new("openai", "session-model"),
        context_window: 128_000,
        // so a thinking level a mode left behind reaches the request instead of clamping to off
        reasoning: true,
        ..Default::default()
    }
}

fn plan_model() -> ModelInfo {
    ModelInfo {
        r#ref: ModelRef::new("anthropic", "plan-model"),
        context_window: 200_000,
        reasoning: true,
        ..Default::default()
    }
}

async fn spawn(setup: Setup) -> Harness {
    let project = tempfile::tempdir().unwrap();
    let sessions = setup.sessions.unwrap_or_else(|| project.path().join("sessions"));
    let mut store = JsonlSessionStore::new(sessions);
    let session_id = match setup.session_id {
        Some(id) => id,
        None => store.create(&project.path().to_string_lossy()).unwrap(),
    };
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    let mut ran = std::collections::HashMap::new();
    for name in ["read", "write", "edit", "bash"] {
        let record = Arc::new(Mutex::new(Vec::new()));
        tools.register(Arc::new(Recording { name, ran: record.clone() }));
        ran.insert(name, record);
    }
    let config = EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider: Arc::new(Scripted {
            steps: Mutex::new(setup.steps.into()),
            requests: requests.clone(),
        }),
        providers: std::collections::HashMap::new(),
        model: session_model(),
        tool_registry: tools,
        hook_registry: HookRegistry::new(),
        session_id: session_id.clone(),
        system_prompt: None,
        todo_store: None,
        compaction_settings: luxe_core::compaction::CompactionSettings::default(),
        // Every call allowed, so the only thing that can stop one is the mode
        permission: PermissionSettings {
            engine: PermissionEngine::new(Vec::new(), vec![], true),
            ask: AskResolution::AutoAllow,
        },
        modes: setup.modes,
        reviewer: setup.reviewer,
        formatter_registry: None,
        spend_sink: None,
        guided_review_project_root: Some(project.path().to_path_buf()),
        invoked_from: None,
        external_work: None,
        review_context_files: Vec::new(),
        review_exclude: Vec::new(),
        remember_choices_in: None,
        cache_warming: luxe_core::cache_warming::CacheWarming::Off,
        review_insight_provider: None,
        github: None,
        permissions_file: None,
        max_tool_iterations: Some(10),
        startup_notices: Vec::new(),
        action_tx: None,
    };
    let (actions, mut events) = Engine::spawn(config);
    // the startup announcement, so callers see only what they caused
    let _ = events.recv().await;
    Harness { actions, events, requests, ran, session_id, _project: project }
}

impl Harness {
    async fn send(&self, action: Action) {
        self.actions.send(action).await.unwrap();
    }

    /// Sends a message and collects every event until its turn ends.
    async fn turn(&mut self, text: &str) -> Vec<Event> {
        self.send(Action::SubmitUserMessage {
            content: text.into(),
            images: Vec::new(),
            remediation: None,
        })
        .await;
        self.until(|event| matches!(event, Event::TurnEnded { .. })).await
    }

    /// Every event up to and including the first that `done` accepts.
    async fn until(&mut self, done: impl Fn(&Event) -> bool) -> Vec<Event> {
        let mut seen = Vec::new();
        loop {
            let event =
                tokio::time::timeout(std::time::Duration::from_secs(20), self.events.recv())
                    .await
                    .expect("an event in time")
                    .expect("the engine is running");
            let finished = done(&event);
            seen.push(event);
            if finished {
                return seen;
            }
        }
    }

    async fn ran(&self, tool: &str) -> Vec<serde_json::Value> {
        self.ran[tool].lock().await.clone()
    }
}

/// The text blocks of the last user message in `request`.
fn last_user_texts(request: &CompletionRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .rev()
        .find(|message| message.role == Role::User)
        .map(|message| {
            message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text(text) => Some(text.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every tool result the model was sent, as text.
fn tool_results(request: &CompletionRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult { content, .. } => Some(format!("{content:?}")),
            _ => None,
        })
        .collect()
}

fn tool_names(request: &CompletionRequest) -> Vec<String> {
    request.tools.iter().map(|tool| tool.name.clone()).collect()
}

// === Plan mode is held at dispatch ===

/// The break this catches: a plan mode that is only an instruction. The model tries the edit and
/// the shell anyway, and neither may run; the one write the mode allows does, and is announced.
#[tokio::test]
async fn plan_mode_writes_only_the_plan() {
    let mut engine = spawn(Setup {
        steps: vec![
            Step::Call("edit", serde_json::json!({"path": "src/lib.rs", "content": "x"})),
            Step::Call("edit", serde_json::json!({"file_path": "src/lib.rs", "content": "x"})),
            Step::Call("bash", serde_json::json!({"command": "echo hi > notes.txt"})),
            Step::Call("read", serde_json::json!({"path": "src/lib.rs"})),
            Step::Call("write", serde_json::json!({"path": "PLAN.md", "content": "# Plan"})),
            Step::Say("The plan is ready."),
        ],
        ..Default::default()
    })
    .await;

    engine.send(Action::SetMode("plan".into())).await;
    let events = engine.turn("plan the rate limiter").await;

    assert!(engine.ran("edit").await.is_empty(), "plan mode ran an edit");
    assert!(engine.ran("bash").await.is_empty(), "plan mode ran the shell");
    assert_eq!(engine.ran("read").await.len(), 1, "reading is what planning is");
    let written = engine.ran("write").await;
    assert_eq!(written.len(), 1, "the plan itself is written");
    assert_eq!(written[0]["path"], "PLAN.md");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::PlanWritten { path } if path == "PLAN.md")),
        "a front-end is told the plan exists"
    );

    let requests = engine.requests.lock().await;
    let results = tool_results(requests.last().unwrap());
    let refusals: Vec<_> = results.iter().filter(|result| result.contains("plan mode")).collect();
    assert_eq!(refusals.len(), 3, "each refusal says why: {results:#?}");
    assert!(
        refusals[0].contains("`src/lib.rs`") && refusals[0].contains("writes only `PLAN.md`"),
        "{refusals:#?}"
    );
    assert!(
        refusals[1].contains("`src/lib.rs`"),
        "a path spelled `file_path` is judged too: {refusals:#?}"
    );
}

/// A plan file reached by walking out of the project, or through a link, is not the plan file.
#[tokio::test]
async fn a_plan_path_that_leaves_the_project_is_refused() {
    let mut engine = spawn(Setup {
        steps: vec![
            Step::Call("write", serde_json::json!({"path": "../PLAN.md", "content": "x"})),
            Step::Call("write", serde_json::json!({"path": "/tmp/PLAN.md", "content": "x"})),
            Step::Call("write", serde_json::json!({"path": "./docs/../PLAN.md", "content": "x"})),
        ],
        ..Default::default()
    })
    .await;
    engine.send(Action::SetMode("plan".into())).await;
    engine.turn("plan").await;

    let written = engine.ran("write").await;
    assert_eq!(written.len(), 1, "only the spelling that lands on the plan ran: {written:?}");
    assert_eq!(written[0]["path"], "./docs/../PLAN.md");
}

// === The prefix holds and the transcript says ===

/// The break this catches: a mode carried in the system prompt or the tool list, which re-bills the
/// whole conversation at the write rate every time somebody presses shift+tab.
#[tokio::test]
async fn a_mode_switch_moves_neither_the_system_prompt_nor_the_tools() {
    let mut engine = spawn(Setup::default()).await;

    engine.turn("hello").await;
    engine.send(Action::SetMode("plan".into())).await;
    engine.turn("plan it").await;
    engine.turn("and the next part").await;
    engine.send(Action::SetMode("build".into())).await;
    engine.turn("build it").await;

    let requests = engine.requests.lock().await;
    assert_eq!(requests.len(), 4);
    for request in requests.iter() {
        assert_eq!(request.system, requests[0].system, "the system prompt moved");
        assert_eq!(tool_names(request), tool_names(&requests[0]), "the tools moved");
    }

    let notes = |index: usize| last_user_texts(&requests[index]);
    assert!(
        !notes(0).iter().any(|text| text.starts_with("[mode:")),
        "a session that never plans hears nothing about modes: {:?}",
        notes(0)
    );
    assert!(notes(1)
        .iter()
        .any(|text| text.starts_with("[mode: plan]") && text.contains("## Phase N")));
    assert!(notes(2).iter().any(|text| text.starts_with("[mode: plan] Still in plan mode")));
    assert!(notes(3).iter().any(|text| text.starts_with("[mode: build]")));
}

/// Toggling there and back before sending tells the model nothing: it never saw the other mode.
#[tokio::test]
async fn a_round_trip_before_sending_announces_nothing() {
    let mut engine = spawn(Setup::default()).await;
    engine.send(Action::SetMode("plan".into())).await;
    engine.send(Action::SetMode("build".into())).await;
    engine.turn("hello").await;

    let requests = engine.requests.lock().await;
    let texts = last_user_texts(&requests[0]);
    assert!(!texts.iter().any(|text| text.starts_with("[mode:")), "{texts:?}");
}

/// An engine with one read-only mode, a review's, carries no notes: it never switched, and its
/// refusals say what the mode holds back when it matters.
#[tokio::test]
async fn a_single_mode_engine_carries_no_mode_notes() {
    let mut engine = spawn(Setup {
        modes: Modes::only(Mode::read_only("review")),
        steps: vec![Step::Call("edit", serde_json::json!({"path": "a.rs"}))],
        ..Default::default()
    })
    .await;
    engine.turn("review this").await;
    engine.turn("and this").await;

    assert!(engine.ran("edit").await.is_empty(), "a review engine changes nothing");
    let requests = engine.requests.lock().await;
    for request in requests.iter() {
        let texts = last_user_texts(request);
        assert!(!texts.iter().any(|text| text.starts_with("[mode:")), "{texts:?}");
    }
}

/// A name that is not a mode is a message, and the session stays where it was.
#[tokio::test]
async fn an_unknown_mode_is_a_message_not_a_switch() {
    let _outside = common::outside_any_project();
    let mut engine = spawn(Setup::default()).await;
    engine.send(Action::SetMode("planner".into())).await;
    let notice = engine.until(|event| matches!(event, Event::Notice(_))).await;
    assert!(
        matches!(notice.last(), Some(Event::Notice(text)) if text.contains("planner") && text.contains("plan")),
        "{notice:?}"
    );
    engine.turn("hello").await;
    let requests = engine.requests.lock().await;
    assert!(!last_user_texts(&requests[0]).iter().any(|text| text.starts_with("[mode:")));
}

/// A resumed session whose transcript last said `plan` is told it is building now.
///
/// The break this catches: starting the announcement from the default on load, which sends a model
/// that believes it is still planning into a build session with nothing correcting it.
#[tokio::test]
async fn a_resumed_session_hears_that_the_mode_changed() {
    let sessions = tempfile::tempdir().unwrap();
    let first = {
        let mut engine =
            spawn(Setup { sessions: Some(sessions.path().to_path_buf()), ..Default::default() })
                .await;
        engine.send(Action::SetMode("plan".into())).await;
        engine.turn("plan it").await;
        engine.send(Action::Close).await;
        engine.session_id.clone()
    };

    let mut resumed = spawn(Setup {
        sessions: Some(sessions.path().to_path_buf()),
        session_id: Some(first.clone()),
        ..Default::default()
    })
    .await;
    resumed.send(Action::LoadSession(first.0.clone())).await;
    resumed.until(|event| matches!(event, Event::SessionLoaded { .. })).await;
    resumed.turn("now build it").await;

    let requests = resumed.requests.lock().await;
    let texts = last_user_texts(requests.last().unwrap());
    assert!(texts.iter().any(|text| text.starts_with("[mode: build]")), "{texts:?}");
}

// === A mode's model comes and goes with it ===

/// The break this catches, which shipped in roles: an override applied on the way in and never
/// undone, so leaving plan left the build on the plan's model, window and thinking level.
#[tokio::test]
async fn leaving_a_mode_puts_the_session_model_back() {
    let modes = Modes::built_in(
        Mode::plan(Vec::new()).with_model(plan_model()).with_thinking(ThinkingLevel::High),
    );
    let mut engine = spawn(Setup { modes, ..Default::default() }).await;

    engine.turn("before").await;
    engine.send(Action::SetMode("plan".into())).await;
    engine.turn("plan").await;
    engine.send(Action::SetMode("build".into())).await;
    engine.turn("after").await;

    let requests = engine.requests.lock().await;
    let (before, planning, after) = (&requests[0], &requests[1], &requests[2]);
    assert_eq!(planning.model.r#ref.id, "plan-model");
    assert_eq!(planning.model.context_window, 200_000, "the plan model brings its own window");
    assert_eq!(planning.thinking, ThinkingLevel::High);
    assert_eq!(after.model.r#ref, before.model.r#ref, "the plan's model was left behind");
    assert_eq!(after.model.context_window, before.model.context_window);
    assert_eq!(after.thinking, before.thinking, "the plan's thinking level was left behind");
}

/// A model picked by hand while planning is the user's, and survives leaving plan mode.
#[tokio::test]
async fn a_model_chosen_inside_a_mode_survives_leaving_it() {
    let modes = Modes::built_in(Mode::plan(Vec::new()).with_model(plan_model()));
    let mut engine = spawn(Setup { modes, ..Default::default() }).await;

    engine.send(Action::SetMode("plan".into())).await;
    let chosen =
        ModelInfo { r#ref: ModelRef::new("openai", "picked-by-hand"), ..Default::default() };
    engine.send(Action::SetModel(chosen)).await;
    engine.send(Action::SetMode("build".into())).await;
    engine.turn("go").await;

    let requests = engine.requests.lock().await;
    assert_eq!(requests.last().unwrap().model.r#ref.id, "picked-by-hand");
}

// === A requested review comes back to the agent ===

#[derive(Debug)]
struct FixedReviewer {
    outcome: PhaseReviewOutcome,
    asked: Arc<Mutex<Vec<PhaseReviewRequest>>>,
}

#[async_trait::async_trait]
impl PhaseReviewer for FixedReviewer {
    async fn review(&self, request: PhaseReviewRequest) -> PhaseReviewOutcome {
        self.asked.lock().await.push(request);
        self.outcome.clone()
    }
}

#[derive(Debug)]
struct FixedContext(&'static str);

#[async_trait::async_trait]
impl PhaseReviewContext for FixedContext {
    async fn diff(&self) -> Result<String, String> {
        Ok(self.0.to_string())
    }
    async fn planning(&self, _phase: u32) -> String {
        "## Phase 0\nAdd a limiter.".into()
    }
    async fn files(&self, _diff: &str, _planning: &str) -> Vec<ReviewedFile> {
        Vec::new()
    }
}

fn reviewer(
    outcome: PhaseReviewOutcome,
    diff: &'static str,
) -> (OnDemandReview, Arc<Mutex<Vec<PhaseReviewRequest>>>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let review = OnDemandReview {
        label: "openai/reviewer".into(),
        reviewer: Arc::new(FixedReviewer { outcome, asked: asked.clone() }),
        context: Arc::new(FixedContext(diff)),
    };
    (review, asked)
}

/// Findings go back to the agent as its next turn, with leave to disagree.
#[tokio::test]
async fn a_review_with_findings_becomes_the_agent_s_next_turn() {
    let (review, asked) =
        reviewer(PhaseReviewOutcome::Fix(vec!["src/a.rs:3 unwraps a fallible read".into()]), "+ x");
    let mut engine = spawn(Setup { reviewer: Some(review), ..Default::default() }).await;

    engine.send(Action::RequestReview { focus: Some("error handling".into()) }).await;
    let events = engine.until(|event| matches!(event, Event::TurnEnded { .. })).await;

    assert!(events.iter().any(
        |event| matches!(event, Event::ReviewStarted { model } if model == "openai/reviewer")
    ));
    assert!(events.iter().any(|event| matches!(
        event,
        Event::ReviewFinished { verdict: RequestedReviewVerdict::Fix, findings } if findings.len() == 1
    )));
    let asked = asked.lock().await;
    assert_eq!(
        asked[0].reason,
        ReviewReason::Requested { focus: Some("error handling".into()) },
        "the reviewer is told it was asked, and what to look at"
    );
    assert_eq!(asked[0].diff, "+ x");
    let requests = engine.requests.lock().await;
    let texts = last_user_texts(requests.last().unwrap()).join("\n");
    assert!(texts.contains("- src/a.rs:3 unwraps a fallible read"), "{texts}");
}

/// A clean review is shown and nothing more: no turn is spent on "nothing to fix".
#[tokio::test]
async fn a_clean_review_starts_no_turn() {
    let (review, _) = reviewer(PhaseReviewOutcome::Continue, "+ x");
    let mut engine = spawn(Setup { reviewer: Some(review), ..Default::default() }).await;

    engine.send(Action::RequestReview { focus: None }).await;
    let events = engine.until(|event| matches!(event, Event::ReviewFinished { .. })).await;
    assert!(matches!(
        events.last(),
        Some(Event::ReviewFinished { verdict: RequestedReviewVerdict::Continue, .. })
    ));
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(engine.requests.lock().await.is_empty(), "a clean verdict cost a model call");
}

/// Nothing to review is said rather than sent to a reviewer to be paid for.
#[tokio::test]
async fn an_empty_working_tree_is_not_sent_to_the_reviewer() {
    let (review, asked) = reviewer(PhaseReviewOutcome::Continue, "");
    let mut engine = spawn(Setup { reviewer: Some(review), ..Default::default() }).await;

    engine.send(Action::RequestReview { focus: None }).await;
    let events = engine.until(|event| matches!(event, Event::ReviewFinished { .. })).await;
    assert!(matches!(
        events.last(),
        Some(Event::ReviewFinished { verdict: RequestedReviewVerdict::Unavailable, findings })
            if findings[0].contains("nothing uncommitted")
    ));
    assert!(asked.lock().await.is_empty());
}

/// Without a reviewer, a request says so.
#[tokio::test]
async fn a_session_without_a_reviewer_says_so() {
    let _outside = common::outside_any_project();
    let mut engine = spawn(Setup::default()).await;
    engine.send(Action::RequestReview { focus: None }).await;
    let events = engine.until(|event| matches!(event, Event::Notice(_))).await;
    assert!(matches!(events.last(), Some(Event::Notice(text)) if text.contains("no reviewer")));
}
