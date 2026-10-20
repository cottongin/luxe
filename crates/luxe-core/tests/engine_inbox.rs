//! The engine never waits on its own inbox: what it hands itself cannot deadlock a full channel.
//!
//! The engine is the action channel's only reader. An idle steer that becomes a turn, and an
//! `AgentEnd` hook that keeps the loop going, used to be sent back into that channel from the
//! reading task — so with the channel full, the send waited for a slot only the engine could free,
//! and `Cancel` and `Close` queued behind it forever.

#![allow(clippy::unwrap_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::stream::{self, BoxStream};
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint};
use luxe_core::provider::{
    CompletionRequest, ModelInfo, Provider, ProviderError, ProviderEvent, StopReason,
};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use luxe_core::tool::ToolRegistry;
use luxe_core::HookRegistry;
use tokio::sync::Notify;

/// The engine's inbox holds this many actions; the test fills it past that.
const INBOX: usize = 64;

struct CountingProvider {
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Provider for CountingProvider {
    fn name(&self) -> &str {
        "counting"
    }

    async fn stream(
        &self,
        _request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::TextDelta { index: 0, delta: "ok".to_string() },
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ])))
    }
}

/// Holds the engine at `point` until released, the first time only; at `AgentEnd` it then asks
/// for one more turn.
struct HoldingHook {
    point: HookPoint,
    entered: Arc<Notify>,
    release: Arc<Notify>,
    fired: AtomicUsize,
}

#[async_trait::async_trait]
impl Hook for HoldingHook {
    fn name(&self) -> &'static str {
        "holding"
    }

    fn interest(&self) -> &[HookPoint] {
        std::slice::from_ref(&self.point)
    }

    async fn on(&self, point: HookPoint, _event: &HookEvent, _cx: &HookCtx) -> HookOutcome {
        if self.fired.fetch_add(1, Ordering::SeqCst) > 0 {
            return HookOutcome::Continue;
        }
        self.entered.notify_one();
        self.release.notified().await;
        match point {
            HookPoint::AgentEnd => HookOutcome::ContinueLoop { message: "keep going".into() },
            _ => HookOutcome::Continue,
        }
    }
}

struct Held {
    action_tx: tokio::sync::mpsc::Sender<Action>,
    event_rx: tokio::sync::mpsc::Receiver<Event>,
    task: tokio::task::JoinHandle<luxe_core::engine::Carryover>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
    calls: Arc<AtomicUsize>,
    _tmp: tempfile::TempDir,
}

fn spawn_held_at(point: HookPoint) -> Held {
    spawn_held_with(point, luxe_core::mode::Modes::default())
}

fn spawn_held_with(point: HookPoint, modes: luxe_core::mode::Modes) -> Held {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/inbox").unwrap();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let mut hooks = HookRegistry::new();
    hooks.register(Arc::new(HoldingHook {
        point,
        entered: entered.clone(),
        release: release.clone(),
        fired: AtomicUsize::new(0),
    }));
    let calls = Arc::new(AtomicUsize::new(0));
    let config = EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider: Arc::new(CountingProvider { calls: calls.clone() }),
        providers: std::collections::HashMap::new(),
        model: ModelInfo::default(),
        tool_registry: ToolRegistry::new(),
        hook_registry: hooks,
        session_id,
        system_prompt: None,
        todo_store: None,
        compaction_settings: luxe_core::compaction::CompactionSettings::default(),
        permission: Default::default(),
        modes,
        reviewer: None,
        formatter_registry: None,
        spend_sink: None,
        guided_review_project_root: Some(std::path::PathBuf::from("/nonexistent-for-this-test")),
        invoked_from: Some(tmp.path().to_path_buf()),
        external_work: None,
        review_context_files: Vec::new(),
        review_exclude: Vec::new(),
        remember_choices_in: None,
        cache_warming: luxe_core::cache_warming::CacheWarming::Off,
        review_insight_provider: None,
        github: None,
        permissions_file: None,
        max_tool_iterations: Some(4),
        startup_notices: Vec::new(),
        action_tx: None,
    };
    let (action_tx, event_rx, task) = Engine::spawn_task(config);
    Held { action_tx, event_rx, task, entered, release, calls, _tmp: tmp }
}

/// Starts a turn and waits until the hook holds it.
async fn hold_a_turn(held: &Held) {
    let content = "start".to_string();
    let submit = Action::SubmitUserMessage { content, images: vec![], remediation: None };
    held.action_tx.send(submit).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), held.entered.notified()).await.unwrap();
}

/// The next state the engine reports that `wanted` accepts, failing if none comes in time.
async fn reported_state(
    held: &mut Held,
    wanted: impl Fn(&luxe_core::event::EngineState) -> bool,
) -> luxe_core::event::EngineState {
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), held.event_rx.recv()).await {
            Ok(Some(Event::StateChanged(state))) if wanted(&state) => return state,
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => panic!("the engine never reported the state wanted"),
        }
    }
}

/// Fills the inbox while the engine is held, with `first` at its head and a sender left waiting
/// behind it, then `Close`. Returns once the inbox is full and that sender is parked.
async fn flood(action_tx: &tokio::sync::mpsc::Sender<Action>, first: Option<Action>) -> usize {
    let refreshes = INBOX + 4;
    let tx = action_tx.clone();
    tokio::spawn(async move {
        if let Some(first) = first {
            tx.send(first).await.unwrap();
        }
        for _ in 0..refreshes {
            tx.send(Action::RefreshWorkspace).await.unwrap();
        }
        tx.send(Action::Close).await.unwrap();
    });
    while action_tx.capacity() > 0 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    refreshes
}

/// Drains events until the engine stops, counting refreshes and noting where turns ended.
async fn drain(held: &mut Held) -> (usize, Vec<usize>) {
    let mut refreshes = 0;
    let mut turns_ended_after = Vec::new();
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(10), held.event_rx.recv()).await {
            Ok(Some(Event::WorkspaceRefreshRequested)) => refreshes += 1,
            Ok(Some(Event::TurnEnded { .. })) => turns_ended_after.push(refreshes),
            Ok(Some(_)) => {}
            Ok(None) => return (refreshes, turns_ended_after),
            Err(_) => panic!("the engine stopped answering: {refreshes} refreshes served"),
        }
    }
}

/// An idle steer becomes a turn even when the inbox is full behind it, and everything queued after
/// it — `Close` last — is still served.
#[tokio::test]
async fn an_idle_steer_with_a_full_inbox_does_not_deadlock() {
    let mut held = spawn_held_at(HookPoint::SessionStart);
    tokio::time::timeout(std::time::Duration::from_secs(5), held.entered.notified()).await.unwrap();
    let steer = Action::SteerTurn { content: "do it".to_string() };
    let refreshes = flood(&held.action_tx, Some(steer)).await;
    held.release.notify_one();

    let (served, turns) = drain(&mut held).await;
    assert_eq!(served, refreshes, "every queued action was served");
    assert_eq!(held.calls.load(Ordering::SeqCst), 1, "the steer ran as a turn");
    assert!(
        turns.len() == 1 && turns[0] >= INBOX - 1,
        "it waited its place behind what was already queued: {turns:?}"
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), held.task).await.unwrap().unwrap();
}

/// An `AgentEnd` hook that keeps the loop going gets its turn even when the inbox filled while it
/// decided, and the engine still closes.
#[tokio::test]
async fn a_continuation_with_a_full_inbox_does_not_deadlock() {
    let mut held = spawn_held_at(HookPoint::AgentEnd);
    let start = "start".to_string();
    held.action_tx
        .send(Action::SubmitUserMessage { content: start, images: vec![], remediation: None })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), held.entered.notified()).await.unwrap();
    let refreshes = flood(&held.action_tx, None).await;
    held.release.notify_one();

    let (served, turns) = drain(&mut held).await;
    assert_eq!(served, refreshes, "every queued action was served");
    assert_eq!(held.calls.load(Ordering::SeqCst), 2, "the continuation ran as a turn");
    assert_eq!(turns.len(), 2, "{turns:?}");
    assert!(turns[1] >= INBOX - 1, "the continuation waited its place: {turns:?}");
    tokio::time::timeout(std::time::Duration::from_secs(5), held.task).await.unwrap().unwrap();
}

/// A mode switch lands while a turn runs. The composer's label is the engine's report of the mode,
/// so a switch held until the turn ended looked like a key that did nothing, and plan mode's hold on
/// writes reached the agent only after the turn had written.
#[tokio::test]
async fn a_mode_switch_lands_while_the_turn_runs() {
    // `AgentStart` is inside the turn, where its actions are served; `AgentEnd` is after it
    let mut held = spawn_held_at(HookPoint::AgentStart);
    hold_a_turn(&held).await;

    held.action_tx.send(Action::SetMode(luxe_core::mode::PLAN.into())).await.unwrap();
    let state = reported_state(&mut held, |state| state.mode == luxe_core::mode::PLAN).await;
    assert!(state.streaming, "the turn is still running, so the report must not say idle");

    held.release.notify_one();
    held.action_tx.send(Action::Close).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), held.task).await.unwrap().unwrap();
}

/// A mode that brings its own model switches at once, but its model waits for the turn to end:
/// swapping the model under a turn in flight hands one provider another's reasoning.
#[tokio::test]
async fn a_mode_switch_mid_turn_leaves_the_model_until_the_turn_ends() {
    let model = ModelInfo {
        r#ref: luxe_core::provider::ModelRef::new("anthropic", "plan-model"),
        ..ModelInfo::default()
    };
    let plan = luxe_core::mode::Mode::plan(Vec::new()).with_model(model);
    let mut held = spawn_held_with(HookPoint::AgentStart, luxe_core::mode::Modes::built_in(plan));
    hold_a_turn(&held).await;

    held.action_tx.send(Action::SetMode(luxe_core::mode::PLAN.into())).await.unwrap();
    let during = reported_state(&mut held, |state| state.mode == luxe_core::mode::PLAN).await;
    assert_ne!(during.model.model, "plan-model", "the running turn keeps its model");

    held.release.notify_one();
    let after = reported_state(&mut held, |state| state.model.model == "plan-model").await;
    assert_eq!(after.mode, luxe_core::mode::PLAN, "and the mode's model follows once it ends");
    held.action_tx.send(Action::Close).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), held.task).await.unwrap().unwrap();
}
