//! Integration: what the front-end found before there was an engine still reaches the reader.
//!
//! A CLI cannot print these itself and be read by everyone — the TUI owns the terminal — so they
//! ride [`EngineConfig::startup_notices`] and arrive as `Event::Notice`. The ordering is the whole
//! point: `SessionLoaded` is what a front-end clears its conversation on, so a notice emitted
//! before it is wiped by the surface that was supposed to show it.

#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;

use futures::stream::{self, BoxStream};
use luxe_core::engine::{Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::provider::{
    CompletionRequest, ModelInfo, Provider, ProviderError, ProviderEvent, StopReason,
};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use luxe_core::tool::ToolRegistry;
use luxe_core::HookRegistry;

/// Never asked anything: this test never submits a turn.
struct SilentProvider;

#[async_trait::async_trait]
impl Provider for SilentProvider {
    fn name(&self) -> &str {
        "silent"
    }

    async fn stream(
        &self,
        _req: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        Ok(Box::pin(stream::iter(vec![ProviderEvent::Done { reason: StopReason::EndTurn }])))
    }
}

fn config(store: JsonlSessionStore, notices: Vec<String>) -> EngineConfig {
    let mut store = store;
    let session_id = store.create("startup-notices").unwrap();
    EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider: Arc::new(SilentProvider),
        providers: std::collections::HashMap::new(),
        model: ModelInfo::default(),
        tool_registry: ToolRegistry::new(),
        hook_registry: HookRegistry::new(),
        session_id,
        system_prompt: None,
        startup_notices: notices,
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
        action_tx: None,
    }
}

/// Every notice arrives, in the order it was given, after the session has loaded.
#[tokio::test]
async fn a_startup_notice_arrives_after_the_session_it_would_otherwise_be_wiped_by() {
    let _outside = common::outside_any_project();
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let (_actions, mut events) = Engine::spawn(config(
        store,
        vec!["--no-tool websearch: no such tool".into(), "project root is elsewhere".into()],
    ));

    let mut loaded = false;
    let mut notices = Vec::new();
    while notices.len() < 2 {
        match tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
            .await
            .expect("the engine says something")
        {
            Some(Event::SessionLoaded { .. }) => loaded = true,
            Some(Event::Notice(notice)) => {
                assert!(loaded, "a notice before SessionLoaded is a notice nobody reads: {notice}");
                notices.push(notice);
            }
            Some(_) => {}
            None => panic!("event channel closed"),
        }
    }

    assert_eq!(
        notices,
        vec![
            "--no-tool websearch: no such tool".to_string(),
            "project root is elsewhere".to_string()
        ],
        "in the order the front-end gave them"
    );
}

/// With nothing to say, nothing is said. A front-end that opens on an empty conversation should
/// stay that way.
#[tokio::test]
async fn a_run_with_nothing_wrong_emits_no_notices() {
    let _outside = common::outside_any_project();
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let (_actions, mut events) = Engine::spawn(config(store, Vec::new()));

    let mut loaded = false;
    // Drain what the engine says on its way up; the session load is the last of it here.
    while let Ok(Some(event)) =
        tokio::time::timeout(std::time::Duration::from_millis(300), events.recv()).await
    {
        match event {
            Event::SessionLoaded { .. } => loaded = true,
            Event::Notice(notice) => panic!("nothing asked for this: {notice}"),
            _ => {}
        }
    }
    assert!(loaded, "the engine still announced its session");
}
