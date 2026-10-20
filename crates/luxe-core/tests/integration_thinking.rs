//! Reasoning/thinking is streamed live, persisted verbatim on the assistant message, and kept in
//! the history the engine hands to providers (each adapter decides how to replay it).
//!
//! The engine must: (1) forward `ProviderEvent::ThinkingDelta` as `Event::ThinkingDelta`,
//! (2) commit a leading `ContentBlock::Thinking` carrying the captured signature (and a
//! `RedactedThinking` block for encrypted reasoning), and (3) preserve those blocks unmodified in
//! the rebuilt request messages on later turns — stripping is now an adapter-level concern.
//!
//! It must also run each model at *its* level: a model may declare where its sessions start
//! (`models.json`'s `thinking`), and a level chosen by hand outranks that for as long as the
//! session lasts.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use futures::stream;
use futures::stream::BoxStream;
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::message::{ContentBlock, Role};
use luxe_core::provider::{
    CompletionRequest, ModelInfo, ModelRef, Provider, ProviderError, ProviderEvent, StopReason,
    ThinkingLevel,
};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use luxe_core::tool::ToolRegistry;
use luxe_core::HookRegistry;
use tokio::sync::Mutex;

/// Records each request it receives and scripts two turns: the first reasons before answering,
/// the second is a plain answer whose request must not carry the prior turn's thinking block.
struct ThinkingProvider {
    calls: Arc<Mutex<Vec<CompletionRequest>>>,
}

#[async_trait::async_trait]
impl Provider for ThinkingProvider {
    fn name(&self) -> &str {
        "thinking-mock"
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
            vec![
                ProviderEvent::ThinkingStart { index: 0 },
                ProviderEvent::ThinkingDelta { index: 0, delta: "Let me reason ".to_string() },
                ProviderEvent::ThinkingDelta { index: 0, delta: "about this.".to_string() },
                ProviderEvent::ThinkingSignature { index: 0, signature: "sig-abc".to_string() },
                ProviderEvent::ThinkingEnd { index: 0 },
                ProviderEvent::TextStart { index: 0 },
                ProviderEvent::TextDelta { index: 0, delta: "Here is the answer.".to_string() },
                ProviderEvent::TextEnd { index: 0 },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ]
        } else {
            vec![
                ProviderEvent::TextDelta { index: 0, delta: "Second answer.".to_string() },
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
) -> EngineConfig {
    engine_config_for(store, session_id, provider, ModelInfo::default())
}

fn engine_config_for(
    store: JsonlSessionStore,
    session_id: luxe_core::session::SessionId,
    provider: Arc<dyn Provider + Send + Sync>,
    model: ModelInfo,
) -> EngineConfig {
    EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider,
        providers: std::collections::HashMap::new(),
        model,
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
        guided_review_project_root: None,
        invoked_from: None,
        external_work: None,
        review_context_files: Vec::new(),
        review_exclude: Vec::new(),
        remember_choices_in: None,
        cache_warming: luxe_core::cache_warming::CacheWarming::Off,
        review_insight_provider: None,
        github: None,
        permissions_file: None,
        max_tool_iterations: luxe_core::engine::DEFAULT_MAX_TOOL_ITERATIONS,
        startup_notices: Vec::new(),
        action_tx: None,
    }
}

/// Drain events for one turn, returning the concatenated thinking deltas and the committed
/// assistant message.
async fn run_turn_collecting(
    action_tx: &tokio::sync::mpsc::Sender<Action>,
    event_rx: &mut tokio::sync::mpsc::Receiver<Event>,
    prompt: &str,
) -> (String, Option<luxe_core::message::Message>) {
    action_tx
        .send(Action::SubmitUserMessage {
            content: prompt.to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    let mut thinking = String::new();
    let mut assistant = None;
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), event_rx.recv()).await {
            Ok(Some(Event::ThinkingDelta { delta, .. })) => thinking.push_str(&delta),
            Ok(Some(Event::MessageCommitted { message, .. }))
                if message.role == Role::Assistant =>
            {
                assistant = Some(message);
            }
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out waiting for turn to end"),
        }
    }
    (thinking, assistant)
}

#[tokio::test]
async fn thinking_is_streamed_committed_and_replayed_verbatim() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(ThinkingProvider { calls: calls.clone() });
    let (action_tx, mut event_rx) = Engine::spawn(engine_config(store, session_id, provider));

    // Turn 1: the model reasons, then answers.
    let (thinking, assistant) = run_turn_collecting(&action_tx, &mut event_rx, "think first").await;
    assert_eq!(thinking, "Let me reason about this.", "thinking deltas stream live");

    let assistant = assistant.expect("assistant message committed");
    // Reasoning leads the committed message with its captured signature, then the answer text.
    assert!(
        matches!(&assistant.content[0], ContentBlock::Thinking { text, signature: Some(sig), .. }
            if text == "Let me reason about this." && sig == "sig-abc"),
        "committed assistant message keeps a leading signed thinking block: {:?}",
        assistant.content
    );
    assert!(
        matches!(&assistant.content[1], ContentBlock::Text(text) if text == "Here is the answer."),
        "answer text follows the thinking block"
    );

    // Turn 2: the rebuilt request must carry the prior thinking block VERBATIM (text + signature),
    // unmodified — adapters decide how to serialize it, but the engine preserves it.
    let _ = run_turn_collecting(&action_tx, &mut event_rx, "continue").await;
    let calls = calls.lock().await;
    assert_eq!(calls.len(), 2, "one provider call per turn");
    let replayed = &calls[1].messages;
    assert!(
        replayed.iter().flat_map(|message| &message.content).any(|block| {
            matches!(block, ContentBlock::Thinking { text, signature: Some(sig), .. }
                if text == "Let me reason about this." && sig == "sig-abc")
        }),
        "the prior signed thinking block is replayed unmodified: {replayed:?}"
    );
    assert!(
        replayed.iter().flat_map(|message| &message.content).any(|block| {
            matches!(block, ContentBlock::Text(text) if text == "Here is the answer.")
        }),
        "the prior answer text is still replayed"
    );

    action_tx.send(Action::Close).await.unwrap();
}

/// Records requests and emits OpenAI-style reasoning (summary text + item id + encrypted content),
/// so the engine must capture id/encrypted and replay them verbatim on the next turn.
struct OpenaiReasoningProvider {
    calls: Arc<Mutex<Vec<CompletionRequest>>>,
}

#[async_trait::async_trait]
impl Provider for OpenaiReasoningProvider {
    fn name(&self) -> &str {
        "openai-reasoning-mock"
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
            vec![
                ProviderEvent::ThinkingDelta { index: 0, delta: "summary".to_string() },
                ProviderEvent::ReasoningItem {
                    index: 0,
                    id: Some("rs_1".to_string()),
                    encrypted: Some("ENC==".to_string()),
                },
                ProviderEvent::TextDelta { index: 0, delta: "Answer.".to_string() },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ]
        } else {
            vec![
                ProviderEvent::TextDelta { index: 0, delta: "Second.".to_string() },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }
}

#[tokio::test]
async fn openai_reasoning_item_is_captured_and_replayed_verbatim() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(OpenaiReasoningProvider { calls: calls.clone() });
    let (action_tx, mut event_rx) = Engine::spawn(engine_config(store, session_id, provider));

    let (thinking, assistant) = run_turn_collecting(&action_tx, &mut event_rx, "go").await;
    assert_eq!(thinking, "summary", "reasoning summary streams for display");
    let assistant = assistant.expect("assistant message committed");
    assert!(
        matches!(&assistant.content[0], ContentBlock::Thinking { text, signature: None, id: Some(id), encrypted: Some(enc), .. }
            if text == "summary" && id == "rs_1" && enc == "ENC=="),
        "committed reasoning carries the item id + encrypted content: {:?}",
        assistant.content
    );

    let _ = run_turn_collecting(&action_tx, &mut event_rx, "again").await;
    let calls = calls.lock().await;
    assert!(
        calls[1].messages.iter().flat_map(|m| &m.content).any(|b| {
            matches!(b, ContentBlock::Thinking { id: Some(id), encrypted: Some(enc), .. }
                if id == "rs_1" && enc == "ENC==")
        }),
        "the reasoning item is replayed with its id + encrypted content intact: {:?}",
        calls[1].messages
    );

    action_tx.send(Action::Close).await.unwrap();
}

/// A provider that emits a single encrypted (redacted) reasoning block, then an answer.
struct RedactedThinkingProvider;

#[async_trait::async_trait]
impl Provider for RedactedThinkingProvider {
    fn name(&self) -> &str {
        "redacted-mock"
    }

    async fn stream(
        &self,
        _req: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::RedactedThinking { index: 0, data: "ENCRYPTED==".to_string() },
            ProviderEvent::TextDelta { index: 0, delta: "Answer.".to_string() },
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ])))
    }
}

#[tokio::test]
async fn redacted_thinking_is_committed_and_surfaced() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();
    let provider = Arc::new(RedactedThinkingProvider);
    let (action_tx, mut event_rx) = Engine::spawn(engine_config(store, session_id, provider));

    let (thinking, assistant) = run_turn_collecting(&action_tx, &mut event_rx, "go").await;
    // The redacted block has no readable text, so a placeholder marker is surfaced for display.
    assert_eq!(thinking, "[redacted reasoning]", "redacted reasoning surfaces a display marker");
    let assistant = assistant.expect("assistant message committed");
    assert!(
        matches!(&assistant.content[0], ContentBlock::RedactedThinking { data } if data == "ENCRYPTED=="),
        "the encrypted reasoning block is committed verbatim: {:?}",
        assistant.content
    );

    action_tx.send(Action::Close).await.unwrap();
}

/// A provider that reasons twice in one message, the way a model does when it emits several
/// reasoning items before answering.
struct TwoReasoningItemsProvider;

#[async_trait::async_trait]
impl Provider for TwoReasoningItemsProvider {
    fn name(&self) -> &str {
        "two-items-mock"
    }

    async fn stream(
        &self,
        _req: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::ThinkingDelta { index: 0, delta: "First item.".to_string() },
            ProviderEvent::ThinkingDelta { index: 1, delta: "Second item.".to_string() },
            ProviderEvent::TextDelta { index: 2, delta: "Answer.".to_string() },
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ])))
    }
}

/// Reasoning deltas carry the provider's own block index.
///
/// A message can hold several reasoning items, and the engine commits one `ContentBlock::Thinking`
/// each. A front-end told they were all block 0 has no way to keep them apart, so it merges them
/// into a single streaming block and then has to tear that down and rebuild it the moment the real
/// shape commits — which a reader sees as the reasoning reflowing under them.
#[tokio::test]
async fn reasoning_deltas_carry_the_block_they_belong_to() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();
    let (action_tx, mut event_rx) =
        Engine::spawn(engine_config(store, session_id, Arc::new(TwoReasoningItemsProvider)));

    action_tx
        .send(Action::SubmitUserMessage {
            content: "go".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    let mut deltas: Vec<(usize, String)> = Vec::new();
    let mut assistant = None;
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), event_rx.recv()).await {
            Ok(Some(Event::ThinkingDelta { block, delta })) => deltas.push((block, delta)),
            Ok(Some(Event::MessageCommitted { message, .. }))
                if message.role == Role::Assistant =>
            {
                assistant = Some(message);
            }
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out waiting for turn to end"),
        }
    }

    assert_eq!(
        deltas,
        vec![(0, "First item.".to_string()), (1, "Second item.".to_string())],
        "each delta names the reasoning item it belongs to"
    );

    // And the shape it described is the shape that commits, which is the whole point.
    let committed = assistant.expect("assistant message committed");
    let reasoning: Vec<&str> = committed
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Thinking { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(reasoning, ["First item.", "Second item."]);
    let blocks: usize =
        deltas.iter().map(|(block, _)| *block).collect::<std::collections::HashSet<_>>().len();
    assert_eq!(blocks, reasoning.len(), "as many streamed blocks as committed ones");

    action_tx.send(Action::Close).await.unwrap();
}

/// A model that answers with one word, recording the reasoning level each request asked for.
struct LevelRecordingProvider {
    levels: Arc<Mutex<Vec<ThinkingLevel>>>,
}

#[async_trait::async_trait]
impl Provider for LevelRecordingProvider {
    fn name(&self) -> &str {
        "level-recording-mock"
    }

    async fn stream(
        &self,
        req: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        self.levels.lock().await.push(req.thinking);
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::TextDelta { index: 0, delta: "ok".to_string() },
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ])))
    }
}

/// A model that accepts every level, so these tests are about precedence rather than clamping.
fn reasoning_model(provider: &str, id: &str, starts_at: Option<ThinkingLevel>) -> ModelInfo {
    let levels = ThinkingLevel::ALL
        .iter()
        .filter(|level| **level != ThinkingLevel::Off)
        .map(|level| (level.as_str().to_string(), Some(level.as_str().to_string())))
        .collect();
    ModelInfo {
        r#ref: ModelRef::new(provider, id),
        reasoning: true,
        thinking_level_map: levels,
        default_thinking: starts_at,
        ..Default::default()
    }
}

/// The level reported by the `StateChanged` whose status names `change`.
///
/// Matched on the status rather than taking the next snapshot to arrive: the engine also announces
/// its startup state, so "the first one" answers a different question than the test is asking.
async fn level_after(
    event_rx: &mut tokio::sync::mpsc::Receiver<Event>,
    change: &str,
) -> ThinkingLevel {
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), event_rx.recv()).await {
            Ok(Some(Event::StateChanged(state))) if state.status == change => {
                return state.thinking
            }
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out waiting for `{change}`"),
        }
    }
}

/// A model's own configured level is where its sessions start — no action required.
///
/// It has to reach the *request*, not just the footer: the whole complaint behind the field is a
/// 27B local model spending minutes at `xhigh` because that is what the frontier model in the last
/// session was set to. Nothing is sent here, so this also pins that the engine seeds itself rather
/// than waiting to be told.
#[tokio::test]
async fn a_models_own_thinking_level_is_where_its_session_starts() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();
    let levels = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(LevelRecordingProvider { levels: levels.clone() });
    let (action_tx, mut event_rx) = Engine::spawn(engine_config_for(
        store,
        session_id,
        provider,
        reasoning_model("lmstudio", "qwen3.8-27b", Some(ThinkingLevel::Medium)),
    ));

    let _ = run_turn_collecting(&action_tx, &mut event_rx, "go").await;
    assert_eq!(levels.lock().await.as_slice(), [ThinkingLevel::Medium]);

    action_tx.send(Action::Close).await.unwrap();
}

/// Without a configured level nothing changes: a session starts off, as it always did.
#[tokio::test]
async fn a_model_that_declares_nothing_still_starts_off() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();
    let levels = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(LevelRecordingProvider { levels: levels.clone() });
    let (action_tx, mut event_rx) = Engine::spawn(engine_config_for(
        store,
        session_id,
        provider,
        reasoning_model("anthropic", "claude-sonnet-5", None),
    ));

    let _ = run_turn_collecting(&action_tx, &mut event_rx, "go").await;
    assert_eq!(levels.lock().await.as_slice(), [ThinkingLevel::Off]);

    action_tx.send(Action::Close).await.unwrap();
}

/// Switching models takes each model's own level, and a level chosen by hand outranks it.
///
/// Three branches, one rule — *this is where you start, not a ceiling*. Applying the configured
/// level unconditionally would throw away the choice you made on the way back to a model; never
/// applying it leaves the carried-over level in place, which is the defect the field exists to fix;
/// and a model nobody has said anything about has to keep behaving exactly as it did before.
#[tokio::test]
async fn switching_models_takes_each_models_level_and_keeps_a_deliberate_choice() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();
    let levels = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(LevelRecordingProvider { levels });
    let frontier = reasoning_model("anthropic", "claude-sonnet-5", None);
    let local = reasoning_model("lmstudio", "qwen3.8-27b", Some(ThinkingLevel::Medium));
    let (action_tx, mut event_rx) =
        Engine::spawn(engine_config_for(store, session_id, provider, frontier.clone()));

    // Deliberately deep on the model that has no opinion of its own.
    action_tx.send(Action::SetThinkingLevel(ThinkingLevel::XHigh)).await.unwrap();
    assert_eq!(level_after(&mut event_rx, "thinking: xhigh").await, ThinkingLevel::XHigh);

    // Switching to the local model drops to the level *it* declares, rather than inheriting one it
    // would spend minutes on.
    action_tx.send(Action::SetModel(local.clone())).await.unwrap();
    assert_eq!(
        level_after(&mut event_rx, "model: lmstudio/qwen3.8-27b").await,
        ThinkingLevel::Medium
    );

    // A bump on the local model holds …
    action_tx.send(Action::SetThinkingLevel(ThinkingLevel::High)).await.unwrap();
    assert_eq!(level_after(&mut event_rx, "thinking: high").await, ThinkingLevel::High);

    // … and so does the deeper one chosen for the frontier model, which is what makes the level a
    // property of the model you are on rather than of the session.
    action_tx.send(Action::SetModel(frontier)).await.unwrap();
    assert_eq!(
        level_after(&mut event_rx, "model: anthropic/claude-sonnet-5").await,
        ThinkingLevel::XHigh
    );

    // Returning to the local model restores the choice made for it — its configured level is where
    // it starts, and the session has since been told otherwise.
    action_tx.send(Action::SetModel(local)).await.unwrap();
    assert_eq!(
        level_after(&mut event_rx, "model: lmstudio/qwen3.8-27b").await,
        ThinkingLevel::High
    );

    // A model with nothing said about it either way keeps the level in hand, as every model did
    // before any of this existed.
    action_tx
        .send(Action::SetModel(reasoning_model("openai", "gpt-5.6-luna", None)))
        .await
        .unwrap();
    assert_eq!(level_after(&mut event_rx, "model: openai/gpt-5.6-luna").await, ThinkingLevel::High);

    action_tx.send(Action::Close).await.unwrap();
}
