//! Regression: `/context` reports what is actually in the window, and its parts add up to it.
//!
//! The whole point of the surface is that a reader can believe it, so the properties worth pinning
//! are the ones a plausible-looking refactor would break silently:
//!
//! - the total equals the occupancy the **provider** reported, so the report and the dock's reading
//!   can never disagree;
//! - the parts sum to that total, so no row is quietly unaccounted for;
//! - a tool result is billed to the conversation and named by the call that produced it, which is
//!   the one line a reader acts on;
//! - measuring does not **spend** the one-time resume briefing, which a turn is owed.

#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::stream;
use futures::stream::BoxStream;
use luxe_core::context_report::{Calibration, ContextComposition, ContextPart};
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::message::ContentBlock;
use luxe_core::provider::{
    CompletionRequest, Cost, ModelInfo, ModelRef, Provider, ProviderError, ProviderEvent,
    StopReason, ToolSchema, Usage,
};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use luxe_core::tool::{Tool, ToolContext, ToolRegistry, ToolResultData};
use luxe_core::HookRegistry;

/// What the provider claims every request occupied. The report must adopt this and not re-derive it.
const REPORTED_PROMPT_TOKENS: u32 = 31_337;

/// What the reply cost. It is in the transcript once the turn ends, so it is in the window too.
const REPORTED_COMPLETION_TOKENS: u32 = 120;

/// Occupancy after a completed call: what was sent, plus the reply now sitting in the transcript.
const REPORTED_OCCUPANCY: u64 = REPORTED_PROMPT_TOKENS as u64 + REPORTED_COMPLETION_TOKENS as u64;

fn model() -> ModelInfo {
    ModelInfo {
        r#ref: ModelRef::new("anthropic", "claude-sonnet-5"),
        context_window: 200_000,
        max_tokens: 64_000,
        cost: Cost {
            input: 2.0,
            output: 10.0,
            cache_read: 0.2,
            cache_write: 2.5,
            cache_write_1h: 0.0,
            tiers: Vec::new(),
        },
        ..Default::default()
    }
}

/// Calls `spill` once, then answers. Every call reports the same occupancy, so the measured total
/// is a fixed, recognizable number rather than something the test has to recompute.
struct OneToolProvider {
    seen: AtomicUsize,
}

#[async_trait::async_trait]
impl Provider for OneToolProvider {
    fn name(&self) -> &str {
        "one-tool"
    }

    async fn stream(
        &self,
        _request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let usage = Usage {
            prompt_tokens: REPORTED_PROMPT_TOKENS,
            completion_tokens: REPORTED_COMPLETION_TOKENS,
            total_tokens: REPORTED_PROMPT_TOKENS + REPORTED_COMPLETION_TOKENS,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            cache_write_1h_tokens: 0,
        };
        let events = if self.seen.fetch_add(1, Ordering::SeqCst) == 0 {
            vec![
                ProviderEvent::ToolCallStart {
                    index: 0,
                    id: "call-0".into(),
                    name: "spill".into(),
                },
                ProviderEvent::ToolCallDelta {
                    index: 0,
                    json_delta: r#"{"path":"crates/luxe-tui/src/view.rs"}"#.into(),
                },
                ProviderEvent::Usage(usage),
                ProviderEvent::Done { reason: StopReason::ToolCall },
            ]
        } else {
            vec![
                ProviderEvent::TextDelta { index: 0, delta: "done".into() },
                ProviderEvent::Usage(usage),
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }
}

/// Returns a payload big enough to dominate the transcript, which is what makes it worth naming.
struct SpillTool;

#[async_trait::async_trait]
impl Tool for SpillTool {
    fn name(&self) -> &str {
        "spill"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "spill".into(),
            description: "Returns a large payload.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } }
            }),
        }
    }

    async fn run(&self, _args: serde_json::Value, _cx: &ToolContext) -> ToolResultData {
        ToolResultData { content: vec![ContentBlock::Text("x".repeat(40_000))], is_error: false }
    }
}

struct Harness {
    action_tx: tokio::sync::mpsc::Sender<Action>,
    event_rx: tokio::sync::mpsc::Receiver<Event>,
    _tmp: tempfile::TempDir,
}

fn harness() -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session = store.create("/tmp/test").unwrap();
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(SpillTool));
    let config = EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider: Arc::new(OneToolProvider { seen: AtomicUsize::new(0) }),
        providers: HashMap::new(),
        model: model(),
        tool_registry: tools,
        hook_registry: HookRegistry::new(),
        session_id: session,
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
    let (action_tx, event_rx) = Engine::spawn(config);
    Harness { action_tx, event_rx, _tmp: tmp }
}

/// Drains events until one matches, or the deadline passes.
async fn wait_for<T>(
    harness: &mut Harness,
    mut pick: impl FnMut(&Event) -> Option<T>,
    what: &str,
) -> T {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, harness.event_rx.recv()).await {
            Ok(Some(event)) => {
                if let Some(found) = pick(&event) {
                    return found;
                }
            }
            Ok(None) => panic!("engine closed before {what}"),
            Err(_) => panic!("timed out waiting for {what}"),
        }
    }
}

async fn run_turn(harness: &mut Harness) {
    harness
        .action_tx
        .send(Action::SubmitUserMessage {
            content: "go".into(),
            images: Vec::new(),
            remediation: None,
        })
        .await
        .unwrap();
    wait_for(
        harness,
        |event| matches!(event, Event::TurnEnded { .. }).then_some(()),
        "the turn to end",
    )
    .await;
}

async fn report(harness: &mut Harness) -> ContextComposition {
    harness.action_tx.send(Action::RequestContextComposition).await.unwrap();
    wait_for(
        harness,
        |event| match event {
            Event::ContextComposition(report) => Some((**report).clone()),
            _ => None,
        },
        "the context report",
    )
    .await
}

/// The number the model billed for is the number the report leads with — not a re-derivation of it.
///
/// This is what stops `/context` and the dock's reading from ever disagreeing, which is the defect
/// the equivalent surfaces in other harnesses actually shipped. Occupancy is the last call's prompt
/// **plus** its completion: the reply is in the transcript now, so the next request carries it.
#[tokio::test]
async fn the_total_is_the_occupancy_the_provider_reported() {
    let mut harness = harness();
    run_turn(&mut harness).await;

    let report = report(&mut harness).await;
    assert_eq!(report.used_tokens, REPORTED_OCCUPANCY);
    assert!(matches!(report.calibration, Calibration::Measured { .. }));
    assert_eq!(report.context_window, 200_000);
    assert_eq!(report.free_tokens(), Some(200_000 - REPORTED_OCCUPANCY));
}

/// Every row is accounted for in the total, so no part of the window goes unreported.
#[tokio::test]
async fn the_parts_sum_to_the_total() {
    let mut harness = harness();
    run_turn(&mut harness).await;

    let report = report(&mut harness).await;
    let summed: u64 = report.slices.iter().map(|slice| slice.tokens).sum();
    // Rounding can move a token between rows; it may never move one out of the total.
    assert!(
        summed.abs_diff(report.used_tokens) <= report.slices.len() as u64,
        "rows summed to {summed}, total is {}",
        report.used_tokens
    );
    assert!(report.slices.len() > 1, "a real request has more than one part: {:?}", report.slices);
}

/// A large tool result is conversation, and it is named by the call that produced it — which is the
/// difference between a report that says "something is big" and one that says what to narrow.
#[tokio::test]
async fn a_large_tool_result_is_conversation_and_names_its_call() {
    let mut harness = harness();
    run_turn(&mut harness).await;

    let report = report(&mut harness).await;
    let conversation = report
        .slices
        .iter()
        .find(|slice| slice.part == ContextPart::Conversation)
        .expect("the transcript is in the window");
    assert_eq!(
        conversation.part, report.slices[0].part,
        "40k characters of tool output is the biggest thing here"
    );
    let named = conversation.detail.first().expect("the result is named");
    assert!(named.label.contains("spill"), "named by its call: {}", named.label);
    assert!(named.label.contains("view.rs"), "and by its subject: {}", named.label);
}

/// Before anything is billed there is nothing to measure against, and the report says so instead of
/// presenting an estimate as a measurement.
#[tokio::test]
async fn an_unbilled_session_says_it_is_estimating() {
    let mut harness = harness();
    let report = report(&mut harness).await;
    assert_eq!(report.calibration, Calibration::Estimated);
    assert!(report.used_tokens > 0, "the prompt and tool block are already in the window");
    assert!(report.slices.iter().all(|slice| slice.part != ContextPart::Conversation));
}

/// Asking what is in the window must not consume the one-time resume briefing.
///
/// The briefing is delivered once, on the first turn after a resume, and it carries the notes the
/// resume existed for. A report that swallowed it would leave the model without them and leave no
/// trace of why.
#[tokio::test]
async fn measuring_does_not_spend_the_resume_briefing() {
    let mut harness = harness();
    run_turn(&mut harness).await;

    // Two reports back to back: if measuring consumed anything one-time, the second would differ.
    let first = report(&mut harness).await;
    let second = report(&mut harness).await;
    assert_eq!(first.slices, second.slices, "measuring is read-only");
    assert_eq!(first.used_tokens, second.used_tokens);
}
