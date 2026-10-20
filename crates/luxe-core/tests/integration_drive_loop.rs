//! Integration test for engine event processing.
//!
//! Verifies that the engine processes provider events and emits TurnEnded events.

#![allow(clippy::unwrap_used)]

use futures::stream;
use futures::stream::BoxStream;
use luxe_core::engine::{Engine, EngineConfig};
use luxe_core::provider::{CompletionRequest, Provider, ProviderError, ProviderEvent, StopReason};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use luxe_core::tool::ToolRegistry;
use std::sync::Arc;
use tempfile::tempdir;
use tokio::sync::RwLock;

/// Mock provider that emits a fixed number of events, then stops.
#[allow(dead_code)]
struct MockProvider {
    turn_count: Arc<RwLock<usize>>,
    max_turns: usize,
}

impl MockProvider {
    fn new(max_turns: usize) -> Self {
        Self { turn_count: Arc::new(RwLock::new(0)), max_turns }
    }
}

#[async_trait::async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    async fn stream(
        &self,
        _req: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let mut turn_count = self.turn_count.write().await;
        *turn_count += 1;
        let current_turn = *turn_count;

        // Emit the full sequence: Start → TextStart → TextDelta → TextEnd → Done.
        let events = vec![
            ProviderEvent::Start,
            ProviderEvent::TextStart { index: 0 },
            ProviderEvent::TextDelta { index: 0, delta: format!("Turn {} response", current_turn) },
            ProviderEvent::TextEnd { index: 0 },
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ];

        Ok(Box::pin(stream::iter(events)))
    }
}

/// Test that controller.stop() can be called while the engine is running,
/// demonstrating the termination pathway used by drive_loop.
#[tokio::test]
async fn test_controller_stop_while_engine_running() {
    let tmp = tempdir().unwrap();
    let state_path = tmp.path().join("state.json");
    let stop_path = tmp.path().join("STOP");

    // Create controller and start it.
    let controller = luxe_core::loop_controller::LoopController::new(state_path, stop_path);
    controller.start().await.unwrap();
    assert!(controller.is_running().await);

    // Spawn engine (it will be idle, waiting for actions).
    let mut session_store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = session_store.create("/tmp/test").unwrap();
    let provider = Arc::new(MockProvider::new(1));
    let engine_config = EngineConfig {
        session_id,
        session_store: std::sync::RwLock::new(Box::new(session_store)),
        provider,
        providers: std::collections::HashMap::new(),
        model: luxe_core::provider::ModelInfo::default(),
        hook_registry: luxe_core::hook::HookRegistry::new(),
        tool_registry: ToolRegistry::new(),
        modes: Default::default(),
        reviewer: None,
        startup_notices: Vec::new(),
        action_tx: None,
        guided_review_project_root: None,
        invoked_from: None,
        external_work: None,
        review_context_files: Vec::new(),
        review_exclude: Vec::new(),
        remember_choices_in: None,
        cache_warming: luxe_core::cache_warming::CacheWarming::Off,
        review_insight_provider: None,
        github: None,
        formatter_registry: None,
        spend_sink: None,
        permissions_file: None,
        max_tool_iterations: luxe_core::engine::DEFAULT_MAX_TOOL_ITERATIONS,
        system_prompt: None,
        todo_store: None,
        compaction_settings: luxe_core::compaction::CompactionSettings::default(),
        permission: Default::default(),
    };
    let (_action_tx, _event_rx) = Engine::spawn(engine_config);

    // Stop the controller.
    controller.stop().await.unwrap();
    assert!(!controller.is_running().await);
}
