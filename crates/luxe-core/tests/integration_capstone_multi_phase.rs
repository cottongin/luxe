//! Multi-phase capstone test — end-to-end unattended loop with mock provider.
//!
//! Drives a 3-phase mini-project through the continuous loop:
//! 1. Engine spawns with all P16 hooks (self-continuation, phase tracking, completion challenger, runaway guards, review gate)
//! 2. Mock provider emits text for 3 phases (simulating agent building work)
//! 3. Loop self-continues via self_continuation hook on AgentEnd
//! 4. After 3 turns, test drives LoopControl::Complete via hook registry
//! 5. Completion challenger triggers verification (first Complete)
//! 6. Test drives LoopControl::Complete again (second Complete)
//! 7. Loop stops after verification (enough confirmations)
//!
//! Asserts: phase advance, completion challenge, self-termination.

#![allow(clippy::unwrap_used)]

mod common;

use futures::stream;
use futures::stream::BoxStream;
use luxe_core::engine::{Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::hook::{HookEvent, HookPoint, LoopControl};
use luxe_core::message::ContentBlock;
use luxe_core::provider::{CompletionRequest, Provider, ProviderError, ProviderEvent, StopReason};
use luxe_core::session::JsonlSessionStore;
use luxe_core::session::SessionStore;
use luxe_core::tool::ToolRegistry;
use std::sync::Arc;
use tempfile::tempdir;
use tokio::sync::RwLock;

/// Mock provider that simulates a 3-phase project.
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

        let events = match current_turn {
            1 => vec![
                ProviderEvent::Start,
                ProviderEvent::TextStart { index: 0 },
                ProviderEvent::TextDelta {
                    index: 0,
                    delta: "Phase 0: Bootstrap complete.".to_string(),
                },
                ProviderEvent::TextEnd { index: 0 },
                ProviderEvent::ToolCallStart {
                    index: 1,
                    id: "call-1".into(),
                    name: "loop_control".into(),
                },
                ProviderEvent::ToolCallDelta {
                    index: 1,
                    json_delta: r#"{"action":"progress"}"#.into(),
                },
                ProviderEvent::ToolCallEnd { index: 1 },
                ProviderEvent::Done { reason: StopReason::ToolCall },
            ],
            2 => vec![
                ProviderEvent::Start,
                ProviderEvent::TextStart { index: 0 },
                ProviderEvent::TextDelta {
                    index: 0,
                    delta: "Phase 1: Core types complete.".to_string(),
                },
                ProviderEvent::TextEnd { index: 0 },
                ProviderEvent::ToolCallStart {
                    index: 1,
                    id: "call-2".into(),
                    name: "loop_control".into(),
                },
                ProviderEvent::ToolCallDelta {
                    index: 1,
                    json_delta: r#"{"action":"progress"}"#.into(),
                },
                ProviderEvent::ToolCallEnd { index: 1 },
                ProviderEvent::Done { reason: StopReason::ToolCall },
            ],
            3 => vec![
                ProviderEvent::Start,
                ProviderEvent::TextStart { index: 0 },
                ProviderEvent::TextDelta {
                    index: 0,
                    delta: "Phase 2: Agent loop complete.".to_string(),
                },
                ProviderEvent::TextEnd { index: 0 },
                ProviderEvent::ToolCallStart {
                    index: 1,
                    id: "call-3".into(),
                    name: "loop_control".into(),
                },
                ProviderEvent::ToolCallDelta {
                    index: 1,
                    json_delta: r#"{"action":"progress"}"#.into(),
                },
                ProviderEvent::ToolCallEnd { index: 1 },
                ProviderEvent::Done { reason: StopReason::ToolCall },
            ],
            _ => vec![
                ProviderEvent::Start,
                ProviderEvent::TextStart { index: 0 },
                ProviderEvent::TextDelta { index: 0, delta: "All phases complete.".to_string() },
                ProviderEvent::TextEnd { index: 0 },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ],
        };

        Ok(Box::pin(stream::iter(events)))
    }
}

#[tokio::test]
async fn test_multi_phase_capstone_unattended() {
    let dir = tempdir().unwrap();
    let repo_root = dir.path().to_path_buf();

    // Create mock PROJECT.md and PLAN.md.
    tokio::fs::write(
        repo_root.join("PROJECT.md"),
        "# Test Project\n\n## Phases\n- Phase 0: Bootstrap\n- Phase 1: Core types\n- Phase 2: Agent loop\n",
    )
    .await
    .unwrap();

    tokio::fs::write(
        repo_root.join("PLAN.md"),
        "# Plan\n\n## Phase 0\nBootstrap.\n\n## Phase 1\nCore types.\n\n## Phase 2\nAgent loop.",
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

    // Clone the hook registry for later use (it's moved into EngineConfig).
    let hook_registry_for_test = hook_registry.clone();

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

    // Set max_turns_per_run to 10 (high enough to not trip runaway guards during test).
    controller
        .update_state(|state| {
            state.max_turns_per_run = 10;
        })
        .await;

    // Submit a user message to trigger the loop.
    action_tx
        .send(luxe_core::engine::Action::SubmitUserMessage {
            content: "Start the continuous loop and run phases 0, 1, and 2".to_string(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();

    // Collect events for up to 10 seconds.
    let mut events_collected = 0;
    let mut turns_completed = 0;
    let timeout = tokio::time::Duration::from_secs(10);
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

        // Break after 3 turns (one per phase).
        if turns_completed >= 3 {
            break;
        }
    }

    // Assert multi-turn iteration happened.
    assert!(
        turns_completed >= 3,
        "Expected at least 3 turns (Phase 0, Phase 1, Phase 2), got {}",
        turns_completed
    );
    assert!(events_collected >= 5, "Expected at least 5 events, got {}", events_collected);

    // Assert phase tracking advanced to phase 2.
    let state = controller.get_state().await;
    assert!(
        state.current_phase >= 2,
        "Expected phase to advance to at least 2, got {}",
        state.current_phase
    );

    // The other half of what this test is for: the run ends on its own, on a runaway guard. It
    // takes a few more iterations than the three phases above — the stall guard fires on the sixth
    // turn without progress — so this *waits* for it rather than assuming it has already happened.
    //
    // Draining the events is what lets it happen at all: the engine's event channel is bounded, so
    // a test that stops reading stops the loop it is waiting on. The version of this that asserted
    // immediately was passing on the timing of a mock provider, and any change that made an
    // iteration slower — a hook reading a file, say — turned it red.
    let ended = tokio::time::timeout(tokio::time::Duration::from_secs(10), async {
        loop {
            if !controller.get_state().await.running {
                return true;
            }
            match event_rx.recv().await {
                Some(_) => continue,
                None => return !controller.get_state().await.running,
            }
        }
    })
    .await
    .unwrap_or(false);
    assert!(ended, "the run should have ended by itself");
    controller.start().await.unwrap();

    let (dummy_event_tx, _) = tokio::sync::mpsc::channel(1);
    let complete = |registry: luxe_core::hook::HookRegistry, tx: tokio::sync::mpsc::Sender<_>| async move {
        registry
            .invoke(
                HookPoint::LoopControl,
                &HookEvent::LoopControl(LoopControl::Complete),
                &luxe_core::hook::HookCtx {
                    session_id: "test".to_string(),
                    event_tx: tx,
                    hook_registry: registry.clone(),
                },
            )
            .await
    };

    // First `complete` asks for verification rather than taking the claim.
    match complete(hook_registry_for_test.clone(), dummy_event_tx.clone()).await {
        luxe_core::hook::HookOutcome::ContinueLoop { message } => assert!(
            message.contains("VERIFICATION REQUIRED"),
            "First Complete should trigger verification, got: {message}"
        ),
        other => panic!("Expected ContinueLoop with verification message, got {other:?}"),
    }

    // The second is the confirmation it was waiting for.
    match complete(hook_registry_for_test.clone(), dummy_event_tx.clone()).await {
        luxe_core::hook::HookOutcome::ContinueLoop { message } => assert!(
            message.contains("Ending loop"),
            "Second Complete should stop the loop, got: {message}"
        ),
        other => panic!("Expected ContinueLoop with stop message, got {other:?}"),
    }

    let state = controller.get_state().await;
    assert!(!state.running, "Expected loop to stop after second Complete");

    println!(
        "Multi-phase capstone test passed: {} turns, {} events, phase={}, loop stopped",
        turns_completed, events_collected, state.current_phase
    );
}
