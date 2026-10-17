//! Regression: a session's reported spend is the sum of every provider call it made.
//!
//! The defect this covers cost real money to find. `Event::TurnUsage` is emitted **per provider
//! call**, and a turn that runs tools makes many; the status reducer read them as cumulative-per-
//! turn and added only the positive delta, so a forty-seven-call turn reported roughly its largest
//! single call. A real session billed at $0.47 displayed $0.09.
//!
//! What is asserted here:
//!
//! - the running total equals the sum of the per-call events, for a turn with several calls;
//! - the spend is written to the transcript and a resumed session adopts it;
//! - work that is not the agent turn (the compaction summarizer) is billed to its own source
//!   rather than vanishing.

#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::stream;
use futures::stream::BoxStream;
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::message::ContentBlock;
use luxe_core::provider::{
    CompletionRequest, Cost, ModelInfo, ModelRef, Provider, ProviderError, ProviderEvent,
    StopReason, ToolSchema, Usage,
};
use luxe_core::session::{EntryKind, JsonlSessionStore, SessionId, SessionStore};
use luxe_core::tool::{Tool, ToolContext, ToolRegistry, ToolResultData};
use luxe_core::usage::{SessionUsage, UsageSource};
use luxe_core::HookRegistry;

/// `claude-sonnet-5`'s published rates.
fn sonnet() -> Cost {
    Cost {
        input: 2.0,
        output: 10.0,
        cache_read: 0.2,
        cache_write: 2.5,
        cache_write_1h: 0.0,
        tiers: Vec::new(),
    }
}

fn priced_model() -> ModelInfo {
    ModelInfo {
        r#ref: ModelRef::new("anthropic", "claude-sonnet-5"),
        context_window: 1_000_000,
        max_tokens: 64_000,
        cost: sonnet(),
        ..Default::default()
    }
}

/// Calls a no-op tool `tool_calls` times, then answers — one provider call per step, each with its
/// own usage over a context that grows the way a real turn's does.
struct MultiCallProvider {
    tool_calls: usize,
    seen: AtomicUsize,
}

#[async_trait::async_trait]
impl Provider for MultiCallProvider {
    fn name(&self) -> &str {
        "multi-call"
    }

    async fn stream(
        &self,
        _request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let step = self.seen.fetch_add(1, Ordering::SeqCst);
        let prompt = 10_000 + (step as u32) * 700;
        let usage = Usage {
            prompt_tokens: prompt,
            completion_tokens: 250,
            total_tokens: prompt + 250,
            cache_read_tokens: prompt.saturating_sub(600),
            cache_write_tokens: 600,
            reasoning_tokens: 0,
            cache_write_1h_tokens: 0,
        };
        let mut events = Vec::new();
        if step < self.tool_calls {
            events.push(ProviderEvent::ToolCallStart {
                index: 0,
                id: format!("call-{step}"),
                name: "noop".into(),
            });
            events.push(ProviderEvent::ToolCallDelta { index: 0, json_delta: "{}".into() });
            events.push(ProviderEvent::Usage(usage));
            events.push(ProviderEvent::Done { reason: StopReason::ToolCall });
        } else {
            events.push(ProviderEvent::TextDelta { index: 0, delta: "done".into() });
            events.push(ProviderEvent::Usage(usage));
            events.push(ProviderEvent::Done { reason: StopReason::EndTurn });
        }
        Ok(Box::pin(stream::iter(events)))
    }
}

struct NoopTool;

#[async_trait::async_trait]
impl Tool for NoopTool {
    fn name(&self) -> &str {
        "noop"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "noop".into(),
            description: "Does nothing.".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }

    async fn run(&self, _args: serde_json::Value, _cx: &ToolContext) -> ToolResultData {
        ToolResultData { content: vec![ContentBlock::Text("ok".into())], is_error: false }
    }
}

struct Harness {
    action_tx: tokio::sync::mpsc::Sender<Action>,
    event_rx: tokio::sync::mpsc::Receiver<Event>,
    root: std::path::PathBuf,
    session: SessionId,
    _tmp: tempfile::TempDir,
}

fn harness(provider: Arc<dyn Provider + Send + Sync>, model: ModelInfo) -> Harness {
    harness_with(provider, model, luxe_core::compaction::CompactionSettings::default())
}

fn harness_with(
    provider: Arc<dyn Provider + Send + Sync>,
    model: ModelInfo,
    compaction_settings: luxe_core::compaction::CompactionSettings,
) -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("sessions");
    let mut store = JsonlSessionStore::new(root.clone());
    let session = store.create("/tmp/test").unwrap();
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(NoopTool));
    let config = EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider,
        providers: HashMap::new(),
        model,
        tool_registry: tools,
        hook_registry: HookRegistry::new(),
        session_id: session.clone(),
        system_prompt: None,
        todo_store: None,
        compaction_settings,
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
    let (action_tx, event_rx) = Engine::spawn(config);
    Harness { action_tx, event_rx, root, session, _tmp: tmp }
}

/// Everything one turn reported: each call's cost, and the last running total published.
#[derive(Default)]
struct TurnReport {
    call_costs: Vec<f64>,
    calls: usize,
    session: Option<SessionUsage>,
}

async fn run_turn(harness: &mut Harness) -> TurnReport {
    harness
        .action_tx
        .send(Action::SubmitUserMessage {
            content: "go".into(),
            images: Vec::new(),
            remediation: None,
        })
        .await
        .unwrap();

    let mut report = TurnReport::default();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(500), harness.event_rx.recv())
            .await
        {
            Ok(Some(Event::TurnUsage { cost_usd, .. })) => {
                report.calls += 1;
                if let Some(cost) = cost_usd {
                    report.call_costs.push(cost);
                }
            }
            Ok(Some(Event::SessionUsage(usage))) => report.session = Some(usage),
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) | Err(_) => {}
            Ok(None) => break,
        }
    }
    report
}

async fn drain(harness: &mut Harness) {
    while let Ok(Some(_)) =
        tokio::time::timeout(std::time::Duration::from_millis(100), harness.event_rx.recv()).await
    {
    }
}

/// The headline regression. A turn with eleven provider calls must report all eleven.
#[tokio::test]
async fn a_turn_with_many_tool_calls_reports_the_sum_of_its_calls() {
    let provider = Arc::new(MultiCallProvider { tool_calls: 10, seen: AtomicUsize::new(0) });
    let mut harness = harness(provider, priced_model());
    assert!(matches!(harness.event_rx.recv().await, Some(Event::SessionLoaded { .. })));
    drain(&mut harness).await;

    let report = run_turn(&mut harness).await;
    assert_eq!(report.calls, 11, "ten tool steps plus the final answer");

    let expected: f64 = report.call_costs.iter().sum();
    let session = report.session.expect("a running total was published");
    assert_eq!(session.total.calls, 11);
    assert!(
        (session.total.cost_usd - expected).abs() < 1e-9,
        "session total must equal the sum of its calls: {} vs {expected}",
        session.total.cost_usd
    );

    // And the old arithmetic — treating each event as a cumulative restatement — really did
    // collapse to roughly one call. Guard the shape of the bug, not just the number.
    let mut delta_accumulated = 0.0;
    let mut previous = 0.0;
    for cost in &report.call_costs {
        delta_accumulated += (cost - previous).max(0.0);
        previous = *cost;
    }
    assert!(
        session.total.cost_usd > delta_accumulated * 3.0,
        "the regression would report ${delta_accumulated} instead of ${}",
        session.total.cost_usd
    );
}

/// Spend is written to the transcript, and a session resumed from it continues the same tally.
#[tokio::test]
async fn spend_is_persisted_and_a_resumed_session_adopts_it() {
    let provider = Arc::new(MultiCallProvider { tool_calls: 3, seen: AtomicUsize::new(0) });
    let mut harness = harness(provider, priced_model());
    assert!(matches!(harness.event_rx.recv().await, Some(Event::SessionLoaded { .. })));
    drain(&mut harness).await;

    let report = run_turn(&mut harness).await;
    let live = report.session.expect("a running total was published");
    assert_eq!(live.total.calls, 4);

    // The transcript carries it.
    let store = JsonlSessionStore::new(harness.root.clone());
    let entries = store.load(&harness.session).unwrap();
    let persisted: Vec<_> = entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            EntryKind::Usage { source, model, totals } => Some((*source, model.clone(), *totals)),
            _ => None,
        })
        .collect();
    assert!(!persisted.is_empty(), "usage must reach the transcript");
    let persisted_calls: u64 = persisted.iter().map(|(_, _, totals)| totals.calls).sum();
    let persisted_cost: f64 = persisted.iter().map(|(_, _, totals)| totals.cost_usd).sum();
    assert_eq!(persisted_calls, live.total.calls, "every call is on disk");
    assert!((persisted_cost - live.total.cost_usd).abs() < 1e-9);
    assert!(persisted.iter().all(|(_, model, _)| model == "claude-sonnet-5"), "priced by model");

    // The conversation still resumes: usage entries are chained, so the leaf is still the tip of
    // the conversation and the path back to the root is intact.
    let leaf = luxe_core::session::latest_leaf(&entries).expect("a leaf");
    let path = store.active_path(&harness.session, &leaf).unwrap();
    let messages = path.iter().filter(|entry| matches!(entry.kind, EntryKind::Message(_))).count();
    assert!(messages >= 5, "user + 4 assistant replies survive on the active path: {messages}");

    // Resuming adopts the tally rather than starting at zero.
    harness.action_tx.send(Action::LoadSession(harness.session.to_string())).await.unwrap();
    let mut resumed: Option<SessionUsage> = None;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(300), harness.event_rx.recv())
            .await
        {
            Ok(Some(Event::SessionUsage(usage))) => {
                resumed = Some(usage);
                break;
            }
            Ok(Some(_)) | Err(_) => {}
            Ok(None) => break,
        }
    }
    let resumed = resumed.expect("a resumed session republishes its tally");
    assert_eq!(resumed.total.calls, live.total.calls, "resume continues the same tally");
    assert!((resumed.total.cost_usd - live.total.cost_usd).abs() < 1e-9);
}

/// A cache refresh is persisted as its own entry kind, so an older Luxe can skip it, and a resumed
/// session still counts it, under its own source rather than as conversation.
#[tokio::test]
async fn a_resumed_session_counts_its_cache_refreshes_under_their_own_source() {
    let provider = Arc::new(MultiCallProvider { tool_calls: 0, seen: AtomicUsize::new(0) });
    let mut harness = harness(provider, priced_model());
    assert!(matches!(harness.event_rx.recv().await, Some(Event::SessionLoaded { .. })));
    drain(&mut harness).await;
    let live = run_turn(&mut harness).await.session.expect("a running total was published");

    let mut store = JsonlSessionStore::new(harness.root.clone());
    let leaf = luxe_core::session::latest_leaf(&store.load(&harness.session).unwrap());
    let refresh = luxe_core::usage::UsageTotals {
        calls: 1,
        prompt_tokens: 11_512,
        completion_tokens: 1,
        cache_read_tokens: 11_510,
        cost_usd: 0.00579,
        ..Default::default()
    };
    store
        .append(
            &harness.session,
            leaf,
            EntryKind::CacheWarm { model: "claude-sonnet-5".into(), totals: refresh },
        )
        .unwrap();

    harness.action_tx.send(Action::LoadSession(harness.session.to_string())).await.unwrap();
    let mut resumed: Option<SessionUsage> = None;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while resumed.is_none() && tokio::time::Instant::now() < deadline {
        if let Ok(Some(Event::SessionUsage(usage))) =
            tokio::time::timeout(std::time::Duration::from_millis(300), harness.event_rx.recv())
                .await
        {
            resumed = Some(usage);
        }
    }
    let resumed = resumed.expect("a resumed session republishes its tally");
    let warming = resumed
        .by_source
        .iter()
        .find(|entry| entry.source == UsageSource::CacheWarm)
        .expect("the refresh is in the breakdown");
    assert_eq!(warming.totals.calls, 1);
    assert_eq!(resumed.total.calls, live.total.calls + 1, "and in the total");
    let agent = resumed.by_source.iter().find(|entry| entry.source == UsageSource::Agent);
    assert_eq!(
        agent.map(|entry| entry.totals.calls),
        Some(live.total.calls),
        "not as conversation"
    );
}

/// A model with no published rates reports tokens but claims no cost.
#[tokio::test]
async fn an_unpriced_model_reports_tokens_without_inventing_a_cost() {
    let provider = Arc::new(MultiCallProvider { tool_calls: 1, seen: AtomicUsize::new(0) });
    let unpriced = ModelInfo {
        r#ref: ModelRef::new("lmstudio", "local"),
        context_window: 200_000,
        ..Default::default()
    };
    let mut harness = harness(provider, unpriced);
    assert!(matches!(harness.event_rx.recv().await, Some(Event::SessionLoaded { .. })));
    drain(&mut harness).await;

    let report = run_turn(&mut harness).await;
    let session = report.session.expect("a running total was published");
    assert!(report.call_costs.is_empty(), "no call claims a price");
    assert_eq!(session.total.unpriced_calls, session.total.calls);
    assert!(session.total.prompt_tokens > 0, "tokens are still counted");
    assert_eq!(session.total.cost_label(), None, "and no cost is shown at all");
}

/// Compaction is the one provider call the user never asked for. Its input is the whole slice of
/// context being summarized, which on a long session makes it the single most expensive call — and
/// it used to be spent entirely off the books.
#[tokio::test]
async fn the_compaction_summarizer_bills_to_its_own_source() {
    let provider = Arc::new(MultiCallProvider { tool_calls: 0, seen: AtomicUsize::new(0) });
    // A window small enough that the very first reply puts the session over the threshold.
    let model = ModelInfo { context_window: 12_000, max_tokens: 1_000, ..priced_model() };
    let settings = luxe_core::compaction::CompactionSettings {
        enabled: true,
        compact_at_fraction: 0.1,
        keep_recent_tokens: 1,
        reserve_tokens: 500,
        ..Default::default()
    };
    let mut harness = harness_with(provider, model, settings);
    assert!(matches!(harness.event_rx.recv().await, Some(Event::SessionLoaded { .. })));
    drain(&mut harness).await;

    // The threshold is checked at the *start* of a turn against the previous turn's occupancy, so
    // the second submission is the one that compacts.
    run_turn(&mut harness).await;
    harness
        .action_tx
        .send(Action::SubmitUserMessage {
            content: "go again".into(),
            images: Vec::new(),
            remediation: None,
        })
        .await
        .unwrap();

    let mut sources = Vec::new();
    let mut session = None;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut ended = false;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(400), harness.event_rx.recv())
            .await
        {
            Ok(Some(Event::TurnUsage { source, .. })) => sources.push(source),
            Ok(Some(Event::SessionUsage(usage))) => session = Some(usage),
            Ok(Some(Event::TurnEnded { .. })) => ended = true,
            // Compaction runs after the turn ends, so keep listening past it.
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) if ended => break,
            Err(_) => {}
        }
    }

    assert!(
        sources.contains(&UsageSource::Compaction),
        "the summarizer's call must be reported: {sources:?}"
    );
    let session = session.expect("a running total");
    let compaction = session.source(UsageSource::Compaction).expect("compaction has its own share");
    assert!(compaction.calls > 0);
    assert!(compaction.cost_usd > 0.0, "and it is priced");
    assert!(
        session.total.cost_usd > session.source(UsageSource::Agent).unwrap().cost_usd,
        "the roll-up includes work the turn did not do"
    );
}

/// Every source the ledger knows about is reachable through the same path, and the roll-up is the
/// sum of its parts — so a future call site cannot report into a total that nobody displays.
#[tokio::test]
async fn the_breakdown_sums_to_the_total() {
    let provider = Arc::new(MultiCallProvider { tool_calls: 2, seen: AtomicUsize::new(0) });
    let mut harness = harness(provider, priced_model());
    assert!(matches!(harness.event_rx.recv().await, Some(Event::SessionLoaded { .. })));
    drain(&mut harness).await;

    let session = run_turn(&mut harness).await.session.expect("a running total");
    let summed: f64 = session.by_source.iter().map(|entry| entry.totals.cost_usd).sum();
    assert!((summed - session.total.cost_usd).abs() < 1e-9);
    assert_eq!(
        session.source(UsageSource::Agent).map(|totals| totals.calls),
        Some(session.total.calls),
        "an ordinary turn bills entirely to the agent"
    );
}
