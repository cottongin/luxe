//! What belongs to a session stays with it when the engine switches to another one.
//!
//! The engine outlives the sessions it runs: `/new`, `/resume` and the picker swap the transcript
//! under it. Lifecycle hooks used to keep the identity of the session the engine launched with, and
//! detached guide work billed whichever session happened to be open when its usage arrived.

#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use futures::stream::{self, BoxStream};
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint};
use luxe_core::provider::{
    CompletionRequest, ModelInfo, Provider, ProviderError, ProviderEvent, StopReason,
};
use luxe_core::session::{JsonlSessionStore, SessionId, SessionStore};
use luxe_core::tool::ToolRegistry;
use luxe_core::HookRegistry;

mod guide;

/// Long enough for a guide pass over a real repository under the slow Git of wine
const WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// Writes down every point it fires at, with the session its context names.
#[derive(Default)]
struct RecordingHook {
    seen: Arc<Mutex<Vec<(HookPoint, String)>>>,
}

const RECORDED: [HookPoint; 7] = [
    HookPoint::SessionStart,
    HookPoint::SessionEnd,
    HookPoint::UserPromptSubmit,
    HookPoint::BeforeAgentStart,
    HookPoint::AgentStart,
    HookPoint::AgentEnd,
    HookPoint::Error,
];

#[async_trait::async_trait]
impl Hook for RecordingHook {
    fn name(&self) -> &'static str {
        "recording"
    }

    fn interest(&self) -> &[HookPoint] {
        &RECORDED
    }

    async fn on(&self, point: HookPoint, _event: &HookEvent, cx: &HookCtx) -> HookOutcome {
        self.seen.lock().unwrap().push((point, cx.session_id.clone()));
        HookOutcome::Continue
    }
}

/// Answers a prompt with text, or fails one carrying `FAIL` the way a refused request does.
struct AnsweringProvider;

#[async_trait::async_trait]
impl Provider for AnsweringProvider {
    fn name(&self) -> &str {
        "answering"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let failing = request.messages.iter().any(|message| message.text().contains("FAIL"));
        if failing {
            return Err(ProviderError::Other("invalid request".into()));
        }
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::TextDelta { index: 0, delta: "ok".to_string() },
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ])))
    }
}

pub(crate) fn engine_config(
    store: JsonlSessionStore,
    session_id: SessionId,
    project_root: &std::path::Path,
) -> EngineConfig {
    EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider: Arc::new(AnsweringProvider),
        providers: std::collections::HashMap::new(),
        model: ModelInfo::default(),
        tool_registry: ToolRegistry::new(),
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
        guided_review_project_root: Some(project_root.to_path_buf()),
        invoked_from: Some(project_root.to_path_buf()),
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

/// The first event `wanted` picks out of `events`, within the wait.
pub(crate) async fn first<T>(
    events: &mut tokio::sync::mpsc::Receiver<Event>,
    what: &str,
    mut wanted: impl FnMut(Event) -> Option<T>,
) -> T {
    tokio::time::timeout(WAIT, async {
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

async fn turn(
    actions: &tokio::sync::mpsc::Sender<Action>,
    events: &mut tokio::sync::mpsc::Receiver<Event>,
    prompt: &str,
) {
    let content = prompt.to_string();
    actions
        .send(Action::SubmitUserMessage { content, images: vec![], remediation: None })
        .await
        .unwrap();
    first(events, "the turn to end", |event| {
        matches!(event, Event::TurnEnded { .. }).then_some(())
    })
    .await;
}

/// Every lifecycle hook names the session it fires for, and a switch ends one session and starts
/// the next.
///
/// The break this catches: the outer loop kept the launch session's hook context, so after a
/// switch `BeforeAgentStart`, `AgentEnd`, `Error` and the final `SessionEnd` named the session the
/// engine started with, while the prompt hooks of the same turn named the new one — and nothing
/// ever told a hook that the first session had ended.
#[tokio::test]
async fn lifecycle_hooks_follow_the_session_the_engine_is_on() {
    let project = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(project.path().join("sessions"));
    let first_session = store.create("/tmp/a").unwrap();
    let second_session = store.create("/tmp/b").unwrap();
    let mut config = engine_config(store, first_session.clone(), project.path());
    let hook = RecordingHook::default();
    let seen = hook.seen.clone();
    config.hook_registry.register(Arc::new(hook));
    let (actions, mut events, task) = Engine::spawn_task(config);

    turn(&actions, &mut events, "in the first session").await;
    actions.send(Action::LoadSession(second_session.to_string())).await.unwrap();
    first(&mut events, "the switch", |event| match event {
        Event::SessionLoaded { session_id, .. } if session_id == second_session.0 => Some(()),
        _ => None,
    })
    .await;
    turn(&actions, &mut events, "in the second session").await;
    turn(&actions, &mut events, "FAIL in the second session").await;
    actions.send(Action::Close).await.unwrap();
    let drained = tokio::spawn(async move { while events.recv().await.is_some() {} });
    tokio::time::timeout(WAIT, task).await.expect("the engine stops").unwrap();
    drained.abort();

    let seen = seen.lock().unwrap().clone();
    let (a, b) = (first_session.0.as_str(), second_session.0.as_str());
    let switch = seen
        .iter()
        .position(|(point, session)| *point == HookPoint::SessionEnd && session == a)
        .expect("the first session is ended at the switch");
    assert_eq!(seen[0], (HookPoint::SessionStart, a.to_string()), "{seen:?}");
    assert_eq!(seen[switch + 1], (HookPoint::SessionStart, b.to_string()), "{seen:?}");
    for (point, session) in &seen[..switch] {
        assert_eq!(session, a, "{point:?} before the switch: {seen:?}");
    }
    for (point, session) in &seen[switch + 1..] {
        assert_eq!(session, b, "{point:?} after the switch: {seen:?}");
    }
    let after: Vec<HookPoint> = seen[switch + 1..].iter().map(|(point, _)| *point).collect();
    assert!(after.contains(&HookPoint::Error), "the failed turn was reported: {after:?}");
    assert_eq!(after.iter().filter(|point| **point == HookPoint::AgentEnd).count(), 2);
    assert_eq!(after.last(), Some(&HookPoint::SessionEnd), "{after:?}");
}

/// Reopening the session the engine is already on is not a switch: no hook hears of it.
#[tokio::test]
async fn reloading_the_same_session_starts_nothing_new() {
    let project = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(project.path().join("sessions"));
    let only = store.create("/tmp/a").unwrap();
    let mut config = engine_config(store, only.clone(), project.path());
    let hook = RecordingHook::default();
    let seen = hook.seen.clone();
    config.hook_registry.register(Arc::new(hook));
    let (actions, mut events, task) = Engine::spawn_task(config);

    actions.send(Action::LoadSession(only.to_string())).await.unwrap();
    first(&mut events, "the reload", |event| {
        matches!(event, Event::SessionLoaded { .. }).then_some(())
    })
    .await;
    actions.send(Action::Close).await.unwrap();
    let drained = tokio::spawn(async move { while events.recv().await.is_some() {} });
    tokio::time::timeout(WAIT, task).await.expect("the engine stops").unwrap();
    drained.abort();

    let points: Vec<HookPoint> = seen.lock().unwrap().iter().map(|(point, _)| *point).collect();
    assert_eq!(points, [HookPoint::SessionStart, HookPoint::SessionEnd]);
}
