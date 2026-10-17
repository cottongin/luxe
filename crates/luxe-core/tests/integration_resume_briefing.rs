//! Resume briefing — on resuming a session with open todos, the first message carries a one-time
//! "Resuming" briefing listing them, and a fresh session does not; and the system prompt is
//! written once per session.

#![allow(clippy::unwrap_used)]

use futures::stream::{self, BoxStream};
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::hook::HookRegistry;
use luxe_core::message::Message;
use luxe_core::provider::{
    CompletionRequest, ModelInfo, Provider, ProviderError, ProviderEvent, StopReason,
};
use luxe_core::session::{EntryKind, JsonlSessionStore, SessionStore};
use luxe_core::todo::{Todo, TodoStatus, TodoStore};
use luxe_core::tool::ToolRegistry;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Captures the system prompt and every text of the first provider call, then ends the turn.
struct CapturingProvider {
    system: Arc<Mutex<Option<String>>>,
}

/// The texts of a request's messages, joined, after its system prompt.
fn whole_request(request: &CompletionRequest) -> String {
    let texts = request.messages.iter().map(Message::text).collect::<Vec<_>>().join("\n");
    format!("{}\n=== messages ===\n{texts}", request.system.clone().unwrap_or_default())
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
        {
            let mut slot = self.system.lock().await;
            if slot.is_none() {
                *slot = Some(whole_request(&request));
            }
        }
        let events = vec![
            ProviderEvent::TextStart { index: 0 },
            ProviderEvent::TextDelta { index: 0, delta: "ok".to_string() },
            ProviderEvent::TextEnd { index: 0 },
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ];
        Ok(Box::pin(stream::iter(events)))
    }
}

fn config(
    store: JsonlSessionStore,
    session_id: luxe_core::session::SessionId,
    todo_root: std::path::PathBuf,
    provider: Arc<dyn Provider + Send + Sync>,
) -> EngineConfig {
    EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider,
        providers: std::collections::HashMap::new(),
        model: ModelInfo::default(),
        tool_registry: ToolRegistry::new(),
        hook_registry: HookRegistry::new(),
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
        max_tool_iterations: Some(8),
        startup_notices: Vec::new(),
        action_tx: None,
    }
}

/// Seeds one open todo for `session_id` under `<root>/todos`.
fn seed_todos(root: &std::path::Path, session_id: &str) {
    TodoStore::new(root.join("todos"))
        .write(
            session_id,
            &[
                Todo {
                    id: 1,
                    subject: "split the module".to_string(),
                    status: TodoStatus::InProgress,
                    active_form: Some("splitting the module".to_string()),
                    blocked_by: vec![],
                },
                Todo {
                    id: 2,
                    subject: "update the call sites".to_string(),
                    status: TodoStatus::Completed,
                    active_form: None,
                    blocked_by: vec![],
                },
            ],
        )
        .unwrap();
}

async fn run_one_turn(
    store: JsonlSessionStore,
    sid: luxe_core::session::SessionId,
    root: &std::path::Path,
) -> String {
    let system = Arc::new(Mutex::new(None));
    let provider = Arc::new(CapturingProvider { system: system.clone() });
    let (tx, mut rx) = Engine::spawn(config(store, sid, root.join("todos"), provider));

    tx.send(Action::SubmitUserMessage {
        content: "continue".to_string(),
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
    tx.send(Action::Close).await.ok();
    let captured = system.lock().await.clone().unwrap_or_default();
    captured
}

#[tokio::test]
async fn resumed_session_gets_a_one_time_briefing() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let sid = store.create("/tmp/test").unwrap();
    // A prior committed message makes this a *resume* on the engine's startup load.
    store.append(&sid, None, EntryKind::Message(Message::user("earlier work"))).unwrap();
    seed_todos(tmp.path(), &sid.0);

    let request = run_one_turn(store, sid, tmp.path()).await;
    let (system, messages) = request.split_once("=== messages ===").unwrap();

    assert!(messages.contains("## Resuming"), "missing briefing:\n{request}");
    assert!(!system.contains("## Resuming"), "it rides in the message, not the prompt");
    assert!(messages.contains("#1 [in progress] split the module"), "missing todo:\n{request}");
    assert!(
        !request.contains("update the call sites"),
        "a finished todo is not outstanding work:\n{request}"
    );
}

#[tokio::test]
async fn a_resumed_session_with_nothing_open_gets_no_briefing() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let sid = store.create("/tmp/test").unwrap();
    store.append(&sid, None, EntryKind::Message(Message::user("earlier work"))).unwrap();
    TodoStore::new(tmp.path().join("todos"))
        .write(
            &sid.0,
            &[Todo {
                id: 1,
                subject: "already done".to_string(),
                status: TodoStatus::Completed,
                active_form: None,
                blocked_by: vec![],
            }],
        )
        .unwrap();

    let system = run_one_turn(store, sid, tmp.path()).await;

    assert!(!system.contains("## Resuming"), "nothing to resume:\n{system}");
}

#[tokio::test]
async fn fresh_session_gets_no_briefing() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let sid = store.create("/tmp/test").unwrap();
    // No prior messages: a fresh session. Todos exist, but resume detection is message-based.
    seed_todos(tmp.path(), &sid.0);

    let system = run_one_turn(store, sid, tmp.path()).await;

    assert!(!system.contains("## Resuming"), "fresh session must not get a briefing:\n{system}");
}

#[path = "integration_resume_briefing/snapshot.rs"]
mod snapshot;
