//! Production engine wiring for the continuous loop.
//!
//! Spawns the shared production engine (real provider + full tool set + durable todos)
//! and drives its `Event` stream. The caller supplies the loop hook registry.

use luxe_core::engine::Action;
use luxe_core::Event;
use luxe_core::HookRegistry;

use crate::engine_setup::AgentSettings;

/// Spawn a production engine with the given settings + hook registry and return the
/// event/action channels.
///
/// Delegates to [`crate::engine_setup::build_engine`] so the loop shares the exact wiring
/// the interactive front-ends use — provider/auth, builtin + LSP tools, session, and
/// the durable checklist. The `settings` carry any `--provider`/`--model`/`--api-key`
/// overrides supplied on the `luxe loop` command line.
pub(crate) async fn spawn_production_engine(
    settings: AgentSettings,
    hook_registry: HookRegistry,
) -> anyhow::Result<(tokio::sync::mpsc::Sender<Action>, tokio::sync::mpsc::Receiver<Event>)> {
    // Unattended in the other sense too: nothing here can pick a model, so a run with none chosen
    // ends now with a sentence about it rather than after a phase of refused turns.
    settings.require_model()?;
    // The loop is unattended: an `ask` can't block on a human, so it resolves the way `--on-ask`
    // says — allow unless somebody asked for otherwise. Guardrail `deny`s are enforced either way.
    let ask = settings.unattended_ask();
    let handle = crate::engine_setup::build_engine(&settings, hook_registry, ask).await?;
    Ok((handle.actions, handle.events))
}

/// Drive the engine loop until the controller stops or an error occurs.
///
/// `kickoff` opens the run. It is the same steer an iteration gets — which phase this is and what
/// the plan says it is — not a bare "Continue building.": a first turn told only to continue has
/// nothing to continue from.
pub(crate) async fn drive_loop(
    action_tx: tokio::sync::mpsc::Sender<Action>,
    mut event_rx: tokio::sync::mpsc::Receiver<Event>,
    mut running: tokio::sync::watch::Receiver<bool>,
    kickoff: String,
    mut view: crate::loop_view::LoopView,
) -> anyhow::Result<()> {
    let steer = Action::SubmitUserMessage { content: kickoff, images: vec![], remediation: None };
    action_tx.send(steer).await.map_err(|e| anyhow::anyhow!("action channel closed: {}", e))?;
    view.turn_started();

    // Consume events until the controller stops. Every one of them goes past the view, which is
    // what makes a run watchable at all, for however many hours the loop runs.
    loop {
        tokio::select! {
            Some(event) = event_rx.recv() => {
                view.observe(&event);
                match event {
                    Event::StateChanged { .. } => {
                        tracing::debug!("engine state change");
                    }
                    // The next iteration's steer is already queued by the self-continuation hook,
                    // so a turn ending is a turn beginning.
                    Event::TurnEnded { .. } => {
                        tracing::debug!("engine turn ended");
                        view.turn_started();
                    }
                    _ => {}
                }
            }
            _ = running.changed() => {
                if !*running.borrow() {
                    tracing::info!("controller stopped, exiting drive_loop");
                    break;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use futures::stream::BoxStream;
    use luxe_core::compaction::CompactionSettings;
    use luxe_core::engine::{Engine, EngineConfig};
    use luxe_core::hooks::self_continuation::SelfContinuationHook;
    use luxe_core::loop_controller::LoopController;
    use luxe_core::provider::{
        CompletionRequest, Provider, ProviderError, ProviderEvent, StopReason,
    };
    use luxe_core::session::SessionStore;
    use luxe_core::{HookRegistry, ToolRegistry};
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::sync::RwLock;

    /// Mock provider that emits one turn per `stream` call, tagging each with its index.
    struct TestProvider {
        turn_count: Arc<RwLock<usize>>,
    }

    impl TestProvider {
        fn new() -> Self {
            Self { turn_count: Arc::new(RwLock::new(0)) }
        }
    }

    #[async_trait::async_trait]
    impl Provider for TestProvider {
        fn name(&self) -> &str {
            "test"
        }

        async fn stream(
            &self,
            _req: CompletionRequest,
            _cancel: tokio::sync::watch::Receiver<bool>,
        ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
            let mut turn_count = self.turn_count.write().await;
            *turn_count += 1;
            let current_turn = *turn_count;

            let events = vec![
                ProviderEvent::Start,
                ProviderEvent::TextStart { index: 0 },
                ProviderEvent::TextDelta {
                    index: 0,
                    delta: format!("Turn {} response", current_turn),
                },
                ProviderEvent::TextEnd { index: 0 },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ];

            Ok(Box::pin(stream::iter(events)))
        }
    }

    #[tokio::test]
    async fn test_drive_loop_returns_on_controller_stop() {
        let tmp = tempdir().unwrap();
        let state_path = tmp.path().join("state.json");
        let stop_path = tmp.path().join("STOP");

        // Create controller and start it.
        let controller = Arc::new(LoopController::new(state_path, stop_path));
        controller.start().await.unwrap();

        // Build engine config with mock provider.
        let mut store = luxe_core::session::JsonlSessionStore::new(tmp.path().join("sessions"));
        let session_id = store.create("/test").unwrap();
        let provider = Arc::new(TestProvider::new());
        let provider_for_assert = provider.clone();

        let mut hook_registry = HookRegistry::new();
        hook_registry.register(Arc::new(SelfContinuationHook::new(controller.clone())));

        let engine_config = EngineConfig {
            session_store: std::sync::RwLock::new(Box::new(store)),
            provider,
            providers: std::collections::HashMap::new(),
            model: luxe_core::provider::ModelInfo::default(),
            tool_registry: ToolRegistry::new(),
            hook_registry,
            session_id,
            system_prompt: None,
            todo_store: None,
            compaction_settings: CompactionSettings::default(),
            permission: Default::default(),
            modes: Default::default(),
            reviewer: None,
            formatter_registry: None,
            spend_sink: None,
            permissions_file: None,
            max_tool_iterations: luxe_core::engine::DEFAULT_MAX_TOOL_ITERATIONS,
            startup_notices: Vec::new(),
            action_tx: None,
            guided_review_project_root: None,
            invoked_from: None,
            external_work: None,
            review_context_files: Vec::new(),
            review_exclude: Vec::new(),
            remember_runtime_state: false,
            cache_warming: luxe_core::cache_warming::CacheWarming::Off,
            review_insight_provider: None,
            github: None,
        };

        let (action_tx, event_rx) = Engine::spawn(engine_config);

        // Get a running receiver from the controller.
        let running_rx = controller.running_rx();

        // Drive the loop in a separate task so we can stop it.
        let drive_handle = tokio::spawn(async move {
            drive_loop(
                action_tx,
                event_rx,
                running_rx,
                "Continue building.".into(),
                crate::loop_view::LoopView::new(true),
            )
            .await
        });

        // Wait for some events, then stop the controller.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        controller.stop().await.unwrap();

        // Assert drive_loop returns AND engine ran >=1 turn (not a no-op).
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), drive_handle)
            .await
            .expect("drive_loop should return within timeout")
            .expect("drive_loop should not panic");
        assert!(result.is_ok(), "drive_loop should return Ok");

        let final_turn_count = *provider_for_assert.turn_count.read().await;
        assert!(final_turn_count > 0, "Engine should have run >=1 turn (got {})", final_turn_count);
    }
}
