//! Models — the engine sends the configured model, and refuses a turn with none.

use crate::fixtures::*;
use std::sync::Arc;

use futures::stream::{self, BoxStream};
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::provider::{
    CompletionRequest, ModelInfo, ModelRef, Provider, ProviderError, ProviderEvent, StopReason,
};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use tokio::sync::Mutex;

/// Provider that records the model id from each request.
struct CapturingProvider {
    seen_model: Arc<Mutex<Option<String>>>,
}

#[async_trait::async_trait]
impl Provider for CapturingProvider {
    fn name(&self) -> &str {
        "capturing"
    }

    async fn stream(
        &self,
        req: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        *self.seen_model.lock().await = Some(req.model.r#ref.id.clone());
        let events = vec![
            ProviderEvent::TextStart { index: 0 },
            ProviderEvent::TextDelta { index: 0, delta: "ok".to_string() },
            ProviderEvent::TextEnd { index: 0 },
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ];
        Ok(Box::pin(stream::iter(events)))
    }
}

/// Records the resolved model of every request, so a test can ask what the *wire* was told rather
/// than what the engine believes.
struct ModelRecordingProvider {
    models: Arc<Mutex<Vec<ModelInfo>>>,
}

#[async_trait::async_trait]
impl Provider for ModelRecordingProvider {
    fn name(&self) -> &str {
        "model-recording"
    }

    async fn stream(
        &self,
        req: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        self.models.lock().await.push(req.model.clone());
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::TextDelta { index: 0, delta: "ok".to_string() },
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ])))
    }
}

/// A mid-session model switch carries that model's compat flags to the adapter.
///
/// The seam the whole feature rests on, and the reason it was inert for so long: an adapter is built
/// once per provider, so the *only* way a model's own request-shaping quirks can reach the wire is on
/// the `ModelInfo` that travels with the request. Asserted here rather than at the adapter, because
/// what could break is the engine dropping or rebuilding the model between `SetModel` and the call.
#[tokio::test]
async fn a_switched_models_compat_travels_with_the_request() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();
    let models = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(ModelRecordingProvider { models: models.clone() });

    let launch =
        ModelInfo { r#ref: ModelRef::new("lmstudio", "qwen3.8-27b"), ..Default::default() };
    let config = EngineConfig { model: launch, ..base_config(store, session_id, provider) };
    let (action_tx, mut event_rx) = Engine::spawn(config);

    // The build with its own template: it takes thinking as a template argument, and its chat
    // template raises on a `developer` role.
    let sibling = ModelInfo {
        r#ref: ModelRef::new("lmstudio", "qwen3.6-27b-fable-fusion"),
        compat: Some(luxe_core::provider::CompatFlags {
            thinking_format: Some("chat_template".into()),
            supports_developer_role: false,
            ..Default::default()
        }),
        ..Default::default()
    };
    action_tx.send(Action::SetModel(sibling)).await.unwrap();
    action_tx
        .send(Action::SubmitUserMessage {
            content: "go".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();
    events_until_turn_end(&mut event_rx).await;

    let seen = models.lock().await;
    let asked = seen.first().expect("one provider call");
    assert_eq!(asked.r#ref.id, "qwen3.6-27b-fable-fusion");
    let compat = asked.compat.as_ref().expect("the switched model's flags reached the request");
    assert_eq!(compat.thinking_format.as_deref(), Some("chat_template"));
    assert!(!compat.supports_developer_role);

    action_tx.send(Action::Close).await.unwrap();
}

#[tokio::test]
async fn engine_announces_sessions_and_forwards_configured_model() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();
    let expected_session_id = session_id.to_string();

    let seen_model = Arc::new(Mutex::new(None));
    let provider = Arc::new(CapturingProvider { seen_model: seen_model.clone() });

    let config = EngineConfig {
        model: ModelInfo {
            r#ref: ModelRef::new("openai_compat", "my-configured-model"),
            ..Default::default()
        },
        ..base_config(store, session_id, provider)
    };

    let (action_tx, mut event_rx) = Engine::spawn(config);
    let Some(Event::SessionLoaded { session_id, messages }) = event_rx.recv().await else {
        panic!("engine must announce its initial session");
    };
    assert_eq!(session_id, expected_session_id);
    assert!(messages.is_empty());

    // Prime the initial session so it carries a message; `New session` then forks a fresh session
    // rather than reusing the (empty) one.
    action_tx
        .send(Action::SubmitUserMessage {
            content: "prime".into(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();
    events_until_turn_end(&mut event_rx).await;

    action_tx.send(Action::NewSession).await.unwrap();
    let new_session_id = loop {
        match event_rx.recv().await {
            Some(Event::SessionLoaded { session_id, messages }) => {
                assert!(messages.is_empty());
                break session_id;
            }
            Some(_) => {}
            None => panic!("engine stopped before announcing the new session"),
        }
    };
    assert_ne!(new_session_id, expected_session_id);

    action_tx
        .send(Action::SubmitUserMessage {
            content: "hi".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    events_until_turn_end(&mut event_rx).await;

    let seen = seen_model.lock().await.clone();
    assert_eq!(
        seen.as_deref(),
        Some("my-configured-model"),
        "engine must forward the configured model id to the provider"
    );
}

#[tokio::test]
async fn new_session_reuses_an_empty_session_instead_of_orphaning_it() {
    // Asking for a new session while the current one is still empty (header only) must reuse it,
    // not spawn an orphan `.jsonl`. This keeps the dashboard 'n' / `/new` flow from littering the
    // session store when the user hasn't typed anything yet.
    let tmp = tempfile::tempdir().unwrap();
    let sessions_root = tmp.path().join("sessions");
    let mut store = JsonlSessionStore::new(&sessions_root);
    let session_id = store.create("/tmp/test").unwrap();
    let expected_session_id = session_id.to_string();

    let config = base_config(
        store,
        session_id,
        Arc::new(CapturingProvider { seen_model: Arc::new(Mutex::new(None)) }),
    );

    let (action_tx, mut event_rx) = Engine::spawn(config);
    assert!(matches!(event_rx.recv().await, Some(Event::SessionLoaded { .. })));

    action_tx.send(Action::NewSession).await.unwrap();
    let reused = loop {
        match event_rx.recv().await {
            Some(Event::SessionLoaded { session_id, messages }) => {
                assert!(messages.is_empty());
                break session_id;
            }
            Some(_) => {}
            None => panic!("engine must re-announce a session for a new-session request"),
        }
    };
    assert_eq!(reused, expected_session_id, "an empty session must be reused, not orphaned");

    // The store must still hold exactly one session file (reuse, not a fresh orphan).
    fn count_jsonl(dir: &std::path::Path) -> usize {
        let mut count = 0;
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    count += count_jsonl(&path);
                } else if path.extension().is_some_and(|ext| ext == "jsonl") {
                    count += 1;
                }
            }
        }
        count
    }
    assert_eq!(count_jsonl(&sessions_root), 1, "reuse must not create a second session file");
}

/// A turn with no model chosen is refused, and the transcript is left alone.
///
/// luxe resolves no default: with nothing chosen the front-end hands the engine a model with an
/// empty id, and the gate at the top of every turn is what makes that safe. The provider is never
/// reached, and the user's message is not committed — a refused turn must not leave a message in
/// the history that would replay on every later request.
#[tokio::test]
async fn a_turn_with_no_model_is_refused_before_anything_is_committed() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();
    let seen_model = Arc::new(Mutex::new(None));
    let provider = Arc::new(CapturingProvider { seen_model: seen_model.clone() });

    // What `AgentSettings::launch_model` hands over when nobody has chosen: a provider, and no id.
    let unset = ModelInfo { r#ref: ModelRef::new("openai_compat", ""), ..Default::default() };
    assert!(unset.r#ref.is_unset());
    let config = EngineConfig { model: unset, ..base_config(store, session_id.clone(), provider) };
    let (action_tx, mut event_rx) = Engine::spawn(config);
    action_tx
        .send(Action::SubmitUserMessage {
            content: "go".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    let refusal =
        events_until_turn_end(&mut event_rx).await.into_iter().find_map(|event| match event {
            Event::Error(message) => Some(message),
            _ => None,
        });
    let refusal = refusal.expect("the refusal is reported, not swallowed");
    assert!(refusal.contains("no model selected"), "{refusal}");
    assert!(refusal.contains("/model"), "and says what to do about it: {refusal}");
    assert_eq!(*seen_model.lock().await, None, "the provider was never asked");

    // A turn that never ran leaves no history behind it — the session's own header aside.
    let store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let entries = store.load(&session_id).unwrap();
    assert!(
        entries
            .iter()
            .all(|entry| matches!(entry.kind, luxe_core::session::EntryKind::Header { .. })),
        "the refused message was committed: {:?}",
        entries.iter().map(|entry| &entry.kind).collect::<Vec<_>>()
    );

    action_tx.send(Action::Close).await.unwrap();
}
