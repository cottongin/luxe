//! Regression: the engine must report usage against the model actually serving the turn.
//!
//! Context fullness, the compaction budget, and cost all used to read `EngineConfig.model`, so an
//! in-session `Action::SetModel` renamed the model without moving any of them — switching to a
//! million-token model kept compacting at the old window and pricing at the old rates.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use futures::stream;
use futures::stream::BoxStream;
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::provider::{
    CompletionRequest, Cost, ModelInfo, ModelRef, Provider, ProviderError, ProviderEvent,
    StopReason, Usage,
};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use luxe_core::tool::ToolRegistry;
use luxe_core::HookRegistry;

/// Emits a fixed usage report so the engine's accounting can be observed.
struct UsageProvider {
    usage: Usage,
}

#[async_trait::async_trait]
impl Provider for UsageProvider {
    fn name(&self) -> &str {
        "usage"
    }

    async fn stream(
        &self,
        _request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::TextDelta { index: 0, delta: "ok".into() },
            ProviderEvent::Usage(self.usage.clone()),
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ])))
    }
}

fn engine_with(
    model: ModelInfo,
    usage: Usage,
) -> (tokio::sync::mpsc::Sender<Action>, tokio::sync::mpsc::Receiver<Event>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();
    let config = EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider: Arc::new(UsageProvider { usage }),
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
    };
    let (action_tx, event_rx) = Engine::spawn(config);
    (action_tx, event_rx, tmp)
}

/// Run one turn and return the context window and cost the engine reported for it.
async fn run_turn(
    action_tx: &tokio::sync::mpsc::Sender<Action>,
    event_rx: &mut tokio::sync::mpsc::Receiver<Event>,
) -> (Option<u32>, Option<f64>) {
    action_tx
        .send(Action::SubmitUserMessage {
            content: "hi".into(),
            images: Vec::new(),
            remediation: None,
        })
        .await
        .unwrap();

    let (mut window, mut cost) = (None, None);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(200), event_rx.recv()).await {
            Ok(Some(Event::ContextUsage { context_window, .. })) => window = Some(context_window),
            Ok(Some(Event::TurnUsage { cost_usd, .. })) => cost = cost_usd.or(cost),
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) | Err(_) => {}
            Ok(None) => break,
        }
    }
    (window, cost)
}

#[tokio::test]
async fn in_session_model_switch_moves_the_reported_context_window() {
    let small = ModelInfo {
        r#ref: ModelRef::new("anthropic", "small"),
        context_window: 128_000,
        max_tokens: 4_096,
        ..Default::default()
    };
    let usage = Usage {
        prompt_tokens: 1_000,
        completion_tokens: 10,
        total_tokens: 1_010,
        ..Default::default()
    };
    let (action_tx, mut event_rx, _tmp) = engine_with(small, usage);
    assert!(matches!(event_rx.recv().await, Some(Event::SessionLoaded { .. })));

    let (window, _) = run_turn(&action_tx, &mut event_rx).await;
    assert_eq!(window, Some(128_000), "baseline reports the configured window");

    // Switch to a million-token model, exactly as the `/model` picker does.
    action_tx
        .send(Action::SetModel(ModelInfo {
            r#ref: ModelRef::new("anthropic", "claude-opus-5"),
            context_window: 1_000_000,
            max_tokens: 128_000,
            ..Default::default()
        }))
        .await
        .unwrap();

    let (window, _) = run_turn(&action_tx, &mut event_rx).await;
    assert_eq!(
        window,
        Some(1_000_000),
        "usage must follow the switched-to model, not the engine's configured one"
    );
}

#[tokio::test]
async fn cost_uses_the_active_model_and_bills_cache_reads_at_the_cache_rate() {
    let free = ModelInfo { r#ref: ModelRef::new("anthropic", "free"), ..Default::default() };
    // 200k input of which 180k is a cache hit, 10k output.
    let usage = Usage {
        prompt_tokens: 200_000,
        completion_tokens: 10_000,
        total_tokens: 210_000,
        cache_read_tokens: 180_000,
        ..Default::default()
    };
    let (action_tx, mut event_rx, _tmp) = engine_with(free, usage);
    assert!(matches!(event_rx.recv().await, Some(Event::SessionLoaded { .. })));

    let (_, cost) = run_turn(&action_tx, &mut event_rx).await;
    assert_eq!(cost, None, "a model with no known pricing reports no cost rather than $0.00");

    action_tx
        .send(Action::SetModel(ModelInfo {
            r#ref: ModelRef::new("anthropic", "claude-opus-5"),
            context_window: 1_000_000,
            cost: Cost {
                input: 5.0,
                output: 25.0,
                cache_read: 0.5,
                cache_write: 6.25,
                cache_write_1h: 0.0,
                tiers: Vec::new(),
            },
            ..Default::default()
        }))
        .await
        .unwrap();

    let (_, cost) = run_turn(&action_tx, &mut event_rx).await;
    // 20k fresh @ $5 + 180k cached @ $0.50 + 10k output @ $25 = 0.10 + 0.09 + 0.25.
    let cost = cost.expect("priced model reports a cost");
    assert!((cost - 0.44).abs() < 1e-9, "expected $0.44, got ${cost}");

    // Charging cache hits at the full input rate would have produced $1.25 — nearly 3x too high.
    let naive = (200_000.0 * 5.0 + 10_000.0 * 25.0) / 1_000_000.0;
    assert!(cost < naive, "cache reads must be discounted");
}
