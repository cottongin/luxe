//! Regression: `max_tokens` is a ceiling on one reply, never a claim on the input window.
//!
//! The engine used to send the resolved model's `max_tokens` verbatim on every call. With the
//! reference local config (`context_window: 262144, max_tokens: 131072`) that means compacting at
//! 242K and then asking for 131K of output on top — 373K against a 262K window. llama.cpp and LM
//! Studio clamp that silently, so it looks fine right up until the endpoint is vLLM or OpenAI,
//! which reject the request outright.
//!
//! Asserted here, end to end through a real turn: a small transcript passes the configured cap
//! through untouched, and once the provider has reported a near-full context the next call's cap
//! shrinks to what the window has left.

#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use futures::stream;
use futures::stream::BoxStream;
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::provider::{
    CompletionRequest, ModelInfo, ModelRef, Provider, ProviderError, ProviderEvent, StopReason,
    Usage,
};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use luxe_core::tool::ToolRegistry;
use luxe_core::HookRegistry;
use tokio::sync::Mutex;

/// The reference LM Studio shape: a 262K window with a 131K output cap.
const CONTEXT_WINDOW: u32 = 262_144;
const MAX_TOKENS: u32 = 131_072;
/// What the provider claims the last call occupied — nearly the whole window.
const REPORTED_PROMPT: u32 = 242_144;

/// Answers in one line and reports whatever occupancy the test asked for, recording every request.
struct BudgetProvider {
    calls: Arc<Mutex<Vec<CompletionRequest>>>,
    report_prompt_tokens: AtomicU32,
}

#[async_trait::async_trait]
impl Provider for BudgetProvider {
    fn name(&self) -> &str {
        "budget-mock"
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        self.calls.lock().await.push(request);
        let prompt = self.report_prompt_tokens.load(Ordering::SeqCst);
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::TextDelta { index: 0, delta: "ok".into() },
            ProviderEvent::Usage(Usage {
                prompt_tokens: prompt,
                completion_tokens: 200,
                total_tokens: prompt + 200,
                ..Usage::default()
            }),
            ProviderEvent::Done { reason: StopReason::EndTurn },
        ])))
    }
}

fn model() -> ModelInfo {
    ModelInfo {
        r#ref: ModelRef::new("lmstudio", "qwen3.8-27b"),
        context_window: CONTEXT_WINDOW,
        max_tokens: MAX_TOKENS,
        ..Default::default()
    }
}

async fn run_turn(
    action_tx: &tokio::sync::mpsc::Sender<Action>,
    event_rx: &mut tokio::sync::mpsc::Receiver<Event>,
    prompt: &str,
) {
    action_tx
        .send(Action::SubmitUserMessage {
            content: prompt.to_string(),
            images: Vec::new(),
            remediation: None,
        })
        .await
        .unwrap();
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), event_rx.recv()).await {
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out waiting for the turn to end"),
        }
    }
}

#[tokio::test]
async fn the_output_cap_follows_what_the_window_has_left() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(BudgetProvider {
        calls: calls.clone(),
        // Turn one reports a nearly empty context …
        report_prompt_tokens: AtomicU32::new(1_000),
    });
    let reporter = provider.clone();

    let config = EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider,
        providers: HashMap::new(),
        model: model(),
        tool_registry: ToolRegistry::new(),
        hook_registry: HookRegistry::new(),
        session_id,
        system_prompt: None,
        todo_store: None,
        // Off: this test is about one request's output cap, not about when to summarize.
        compaction_settings: luxe_core::compaction::CompactionSettings {
            enabled: false,
            ..Default::default()
        },
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
    let (action_tx, mut event_rx) = Engine::spawn(config);

    run_turn(&action_tx, &mut event_rx, "hello").await;
    // … and the second reports a context that is nearly full.
    reporter.report_prompt_tokens.store(REPORTED_PROMPT, Ordering::SeqCst);
    run_turn(&action_tx, &mut event_rx, "again").await;
    run_turn(&action_tx, &mut event_rx, "once more").await;

    let calls = calls.lock().await;
    assert_eq!(calls.len(), 3, "one provider call per turn");

    // An almost-empty window: the configured ceiling is what a long reply may actually use.
    assert_eq!(calls[0].max_tokens, Some(MAX_TOKENS), "a small transcript keeps the full cap");
    assert_eq!(calls[1].max_tokens, Some(MAX_TOKENS));

    // Once the provider has measured a 242K context, the cap is what is left of the window.
    let capped = calls[2].max_tokens.expect("a cap");
    assert!(capped < MAX_TOKENS, "the cap must shrink, got {capped}");
    let prompt = u64::from(REPORTED_PROMPT) + 200;
    assert!(
        prompt + u64::from(capped) <= u64::from(CONTEXT_WINDOW),
        "prompt {prompt} + cap {capped} must fit a {CONTEXT_WINDOW}-token window"
    );

    action_tx.send(Action::Close).await.unwrap();
}
