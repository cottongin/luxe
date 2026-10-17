//! Capstone engine integration test — spawns an Engine with a manually-built P16 hook registry + MockProvider.
//!
//! Verifies end-to-end: self-continuation (engine re-drives on agent_end via ContinueLoop),
//! multi-turn iteration through a mock two-phase project, and that the loop terminates
//! via runaway guards when max_turns_per_run is reached.

#![allow(clippy::unwrap_used)]

mod common;

use futures::stream;
use futures::stream::BoxStream;
use luxe_core::engine::{Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::message::ContentBlock;
use luxe_core::provider::{CompletionRequest, Provider, ProviderError, ProviderEvent, StopReason};
use luxe_core::session::JsonlSessionStore;
use luxe_core::session::SessionStore;
use luxe_core::tool::ToolRegistry;
use std::sync::Arc;
use tempfile::tempdir;
use tokio::sync::RwLock;

/// Mock provider that simulates a simple two-phase project:
/// Phase 0: "Bootstrap done" → end turn
/// Phase 1: "Core types done" → end turn
struct MockProvider {
    turn_count: Arc<RwLock<usize>>,
}

impl MockProvider {
    fn new() -> Self {
        Self { turn_count: Arc::new(RwLock::new(0)) }
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

        let events = if current_turn <= 2 {
            // First two turns claim a phase the way a real agent does — by calling `loop_control`,
            // not by saying so in prose. Before the review integrator moved to `LoopControl`, the
            // phase advanced on any turn at all and this test passed without the call.
            vec![
                ProviderEvent::Start,
                ProviderEvent::TextStart { index: 0 },
                ProviderEvent::TextDelta {
                    index: 0,
                    delta: format!("Phase {} complete", current_turn / 2),
                },
                ProviderEvent::TextEnd { index: 0 },
                ProviderEvent::ToolCallStart {
                    index: 1,
                    id: format!("call-{current_turn}"),
                    name: "loop_control".into(),
                },
                ProviderEvent::ToolCallDelta {
                    index: 1,
                    json_delta: r#"{"action":"progress"}"#.into(),
                },
                ProviderEvent::ToolCallEnd { index: 1 },
                ProviderEvent::Done { reason: StopReason::ToolCall },
            ]
        } else {
            // Subsequent turns: stop the loop
            vec![
                ProviderEvent::Start,
                ProviderEvent::TextStart { index: 0 },
                ProviderEvent::TextDelta {
                    index: 0,
                    delta: "All phases complete. Stopping loop.".to_string(),
                },
                ProviderEvent::TextEnd { index: 0 },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ]
        };

        Ok(Box::pin(stream::iter(events)))
    }
}

#[tokio::test]
async fn test_capstone_engine_end_to_end() {
    let dir = tempdir().unwrap();
    let repo_root = dir.path().to_path_buf();

    // Create mock PROJECT.md and PLAN.md.
    tokio::fs::write(
        repo_root.join("PROJECT.md"),
        "# Test Project\n\n## Phases\n- Phase 0: Bootstrap\n- Phase 1: Core types\n",
    )
    .await
    .unwrap();

    tokio::fs::write(
        repo_root.join("PLAN.md"),
        "# Plan\n\n## Phase 0\nBootstrap.\n\n## Phase 1\nCore types.",
    )
    .await
    .unwrap();

    // Set up state paths.
    let state_path = repo_root.join(".luxe").join("loop_state.json");
    let stop_path = repo_root.join(".luxe").join("STOP");
    let journal_path = repo_root.join(".luxe").join("loop").join("PROGRESS.md");
    let archive_path = repo_root.join(".luxe").join("loop").join("PROGRESS.archive.md");

    tokio::fs::create_dir_all(repo_root.join(".luxe")).await.unwrap();
    tokio::fs::create_dir_all(repo_root.join(".luxe").join("loop")).await.unwrap();

    // Create a mock PROGRESS.md (small).
    tokio::fs::write(&journal_path, "# Progress\n\n## Phase 0\nDone.\n\n## Phase 1\nIn progress.")
        .await
        .unwrap();

    // Build the hook registry and controller.
    let controller = Arc::new(luxe_core::loop_controller::LoopController::new(
        state_path.clone(),
        stop_path.clone(),
    ));

    // A review gate whose acceptance command always passes, so the test is about the loop.
    let review_gate_hook =
        Arc::new(luxe_core::hooks::review_gate::ReviewGateHook::with_commands(vec![
            common::always_succeeds(),
        ]));

    // Build the hook registry manually to use our custom review gate hook.
    let mut hook_registry = luxe_core::hook::HookRegistry::new();
    hook_registry.register(Arc::new(
        luxe_core::hooks::self_continuation::SelfContinuationHook::new(controller.clone()),
    ));
    hook_registry.register(Arc::new(
        luxe_core::hooks::context_aware_termination::ContextAwareTerminationHook::new(
            controller.clone(),
        ),
    ));
    hook_registry.register(Arc::new(luxe_core::hooks::journal_bounding::JournalBoundingHook::new(
        journal_path.clone(),
        archive_path.clone(),
        24,
    )));
    let integrator =
        luxe_core::hooks::review_integrator::ReviewIntegratorHook::new(controller.clone())
            .with_gate(Arc::new(
                luxe_core::phase_gate::PhaseGate::new().with_commands(review_gate_hook),
            ));
    hook_registry.register(Arc::new(integrator));
    hook_registry.register(Arc::new(
        luxe_core::hooks::completion_challenger::CompletionChallengerHook::new(controller.clone())
            .await,
    ));
    hook_registry.register(Arc::new(luxe_core::hooks::runaway_guards::RunawayGuardsHook::new(
        controller.clone(),
    )));
    hook_registry.register(Arc::new(luxe_core::hooks::fresh::FreshHook::new(journal_path.clone())));

    // Create a tool registry (empty for this test).
    // The loop's phase gate is reached through a tool call, so the registry needs the tool the
    // mock calls. `loop_control` itself lives in `luxe-tools`, which depends on this crate.
    let mut tool_registry = ToolRegistry::new();
    tool_registry.register(Arc::new(common::LoopControlStub));

    // Create the session in the store.
    let mut store = JsonlSessionStore::new(repo_root.join(".luxe").join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    // Build the engine config.
    let config = EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider: Arc::new(MockProvider::new()),
        providers: std::collections::HashMap::new(),
        model: luxe_core::provider::ModelInfo::default(),
        tool_registry,
        hook_registry,
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
        remember_runtime_state: false,
        cache_warming: luxe_core::cache_warming::CacheWarming::Off,
        review_insight_provider: None,
        github: None,
        permissions_file: None,
        max_tool_iterations: luxe_core::engine::DEFAULT_MAX_TOOL_ITERATIONS,
        startup_notices: Vec::new(),
        action_tx: None,
    };

    // Spawn the engine.
    let (action_tx, mut event_rx) = Engine::spawn(config);

    // Start the loop controller.
    controller.start().await.unwrap();

    // Set max_turns_per_run to 5 so the loop self-terminates via runaway guards.
    controller
        .update_state(|state| {
            state.max_turns_per_run = 5;
        })
        .await;

    // Submit a user message to trigger the loop.
    action_tx
        .send(luxe_core::engine::Action::SubmitUserMessage {
            content: "Start the continuous loop and run phases 0 and 1".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    // Collect events for a few seconds to simulate loop execution.
    let mut events_collected = 0;
    let mut turns_completed = 0;
    let timeout = tokio::time::Duration::from_secs(5);
    let start = tokio::time::Instant::now();

    while start.elapsed() < timeout {
        tokio::select! {
            event = event_rx.recv() => {
                match event {
                    Some(Event::StateChanged(state)) => {
                        println!("State changed: {:?}", state);
                        events_collected += 1;
                    }
                    Some(Event::TurnEnded { reason }) => {
                        println!("Turn ended: {:?}", reason);
                        turns_completed += 1;
                        events_collected += 1;
                    }
                    Some(Event::TokenDelta { block, delta }) => {
                        println!("Token delta (block {}): {}", block, &delta[..delta.len().min(50)]);
                        events_collected += 1;
                    }
                    Some(Event::ToolStarted { call_id, name }) => {
                        println!("Tool started: {} ({})", name, call_id);
                        events_collected += 1;
                    }
                    Some(Event::MessageCommitted { message: msg, .. }) => {
                        println!("Message committed: {:?}", msg.content.iter().map(|c| match c { ContentBlock::Text(t) => t.clone(), _ => String::new() }).collect::<Vec<_>>());
                        events_collected += 1;
                    }
                    Some(other) => {
                        println!("Other event: {:?}", other);
                        events_collected += 1;
                    }
                    None => {
                        println!("Event channel closed");
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {}
        }

        // Break after max_turns_per_run turns (5) to avoid waiting for the full timeout.
        if turns_completed >= 5 {
            break;
        }
    }

    // Assert multi-turn iteration happened.
    assert!(
        turns_completed >= 3,
        "Expected at least 3 turns (Phase 0, Phase 1, stop), got {}",
        turns_completed
    );
    assert!(events_collected >= 5, "Expected at least 5 events, got {}", events_collected);

    // Assert phase tracking advanced.
    let state = controller.get_state().await;
    assert!(
        state.current_phase >= 1,
        "Expected phase to advance to at least 1, got {}",
        state.current_phase
    );

    // Assert the loop stopped itself (is_running() == false) — proves runaway guards fired.
    let state = controller.get_state().await;
    assert!(
        !state.running,
        "Expected loop to stop on its own (max_turns_per_run=5), but is_running() is still true"
    );

    println!(
        "Capstone engine test passed: {} turns, {} events, loop self-terminated",
        turns_completed, events_collected
    );
}
