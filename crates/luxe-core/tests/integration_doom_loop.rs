//! Doom-loop guard — a model stuck on one call is stopped, unattended, with feedback it can use.
//!
//! The guard used to raise a permission *ask*, and headless/loop runs auto-allow an ask — so it was
//! a no-op in exactly the mode where a runaway costs the most. Denying instead both stops the loop
//! and hands the model something actionable, in every mode.

#![allow(clippy::unwrap_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::stream::{self, BoxStream};
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::hook::HookRegistry;
use luxe_core::provider::{
    CompletionRequest, ModelInfo, Provider, ProviderError, ProviderEvent, StopReason,
};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use luxe_core::tool::{Tool, ToolContext, ToolRegistry, ToolResultData};

/// A stuck model: always emits the same tool call with the same arguments.
struct StuckProvider {
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Provider for StuckProvider {
    fn name(&self) -> &str {
        "stuck"
    }

    async fn stream(
        &self,
        _req: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let round = self.calls.fetch_add(1, Ordering::SeqCst);
        // After a few rounds, stop asking so the test terminates either way.
        if round >= 6 {
            return Ok(Box::pin(stream::iter(vec![
                ProviderEvent::TextDelta { index: 0, delta: "giving up".to_string() },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ])));
        }
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::ToolCallStart {
                index: 0,
                id: format!("call-{round}"),
                name: "spin".to_string(),
            },
            ProviderEvent::ToolCallDelta { index: 0, json_delta: r#"{"same":"args"}"#.to_string() },
            ProviderEvent::ToolCallEnd { index: 0 },
            ProviderEvent::Done { reason: StopReason::ToolCall },
        ])))
    }
}

/// Counts how many times it actually ran.
struct SpinTool {
    runs: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Tool for SpinTool {
    fn name(&self) -> &str {
        "spin"
    }

    fn schema(&self) -> luxe_core::provider::ToolSchema {
        luxe_core::provider::ToolSchema {
            name: "spin".to_string(),
            description: "does nothing".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }

    async fn run(&self, _args: serde_json::Value, _cx: &ToolContext) -> ToolResultData {
        self.runs.fetch_add(1, Ordering::SeqCst);
        ToolResultData {
            content: vec![luxe_core::ContentBlock::Text("same result".to_string())],
            is_error: false,
        }
    }
}

#[tokio::test]
async fn a_repeated_identical_call_is_blocked_unattended_with_actionable_feedback() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(tmp.path().join("sessions"));
    let session_id = store.create("/tmp/test").unwrap();

    let provider_calls = Arc::new(AtomicUsize::new(0));
    let tool_runs = Arc::new(AtomicUsize::new(0));
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(SpinTool { runs: tool_runs.clone() }));

    let config = EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider: Arc::new(StuckProvider { calls: provider_calls.clone() }),
        providers: std::collections::HashMap::new(),
        model: ModelInfo::default(),
        tool_registry: tools,
        hook_registry: HookRegistry::new(),
        session_id,
        system_prompt: None,
        todo_store: None,
        compaction_settings: luxe_core::compaction::CompactionSettings::default(),
        // The default: permissive, with `ask` auto-allowed the way a headless run resolves it.
        permission: Default::default(),
        modes: Default::default(),
        reviewer: None,
        formatter_registry: None,
        spend_sink: None,
        guided_review_project_root: Some(tmp.path().to_path_buf()),
        invoked_from: None,
        external_work: None,
        review_context_files: Vec::new(),
        review_exclude: Vec::new(),
        remember_runtime_state: false,
        cache_warming: luxe_core::cache_warming::CacheWarming::Off,
        review_insight_provider: None,
        github: None,
        permissions_file: None,
        max_tool_iterations: Some(10),
        startup_notices: Vec::new(),
        action_tx: None,
    };

    let (tx, mut rx) = Engine::spawn(config);
    tx.send(Action::SubmitUserMessage {
        content: "go".to_string(),
        images: vec![],
        remediation: None,
    })
    .await
    .unwrap();

    let mut blocked = Vec::new();
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await {
            // Tool results reach the transcript as blocks on a committed message.
            Ok(Some(Event::MessageCommitted { message, .. })) => {
                for block in &message.content {
                    if let luxe_core::message::ContentBlock::ToolResult { content, .. } = block {
                        for output in content {
                            let luxe_core::message::ToolOutput::Text(text) = output else {
                                continue;
                            };
                            if text.contains("Blocked") {
                                blocked.push(text.clone());
                            }
                        }
                    }
                }
            }
            Ok(Some(Event::TurnEnded { .. })) => break,
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out — the guard did not stop the loop"),
        }
    }
    tx.send(Action::Close).await.ok();

    // Two identical calls run; from the third on the guard blocks them.
    assert_eq!(tool_runs.load(Ordering::SeqCst), 2, "the tool stopped running after two repeats");
    assert!(!blocked.is_empty(), "the model was told the call was blocked");
    let message = &blocked[0];
    assert!(message.contains("spin"), "the message names the tool: {message}");
    assert!(
        message.contains("different approach") || message.contains("already have"),
        "and says what to do instead: {message}"
    );
}
