use super::*;
use futures::stream::{self, BoxStream};
use luxe_core::compaction::CompactionSettings;
use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::guided_review::{
    CheckSummary, GuidedReview, ReviewBase, ReviewContext, ReviewId, ReviewRevisionId,
    ReviewSource, ReviewStats, ReviewTarget,
};
use luxe_core::provider::{
    CompletionRequest, ModelInfo, Provider, ProviderError, ProviderEvent, StopReason,
};
use luxe_core::session::{JsonlSessionStore, SessionStore};
use luxe_core::{HookRegistry, ToolActivityState, ToolRegistry};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::time::{timeout, Duration};

/// How long a test waits for an event the bridge is required to emit.
///
/// Generous on purpose. Every assertion below is about *what* arrived — the scope id, the
/// revision incrementing, which activity was projected — and never about how quickly. Building
/// the first snapshot walks the workspace and shells out to git, so a one-second budget was not
/// measuring the bridge, it was measuring the machine: it passed here and went red under
/// parallel load, which reads as a product failure and is not one. Still far inside nextest's
/// own 120s cutoff, so a genuine hang still fails rather than wedging the run.
const EVENT_WAIT: Duration = Duration::from_secs(30);

struct LifecycleProvider {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl Provider for LifecycleProvider {
    fn name(&self) -> &str {
        "lifecycle"
    }

    async fn stream(
        &self,
        _req: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        // Call 0 primes the initial session (plain text turn); call 1 emits the tool call the
        // "run a tool" turn asserts on; later calls just end the turn.
        let events = match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => vec![
                ProviderEvent::TextDelta { index: 0, delta: "primed".into() },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ],
            1 => vec![
                ProviderEvent::ToolCallStart {
                    index: 0,
                    id: "real-call".into(),
                    name: "missing-tool".into(),
                },
                ProviderEvent::ToolCallDelta { index: 0, json_delta: "{}".into() },
                ProviderEvent::Done { reason: StopReason::ToolCall },
            ],
            _ => vec![
                ProviderEvent::TextDelta { index: 0, delta: "done".into() },
                ProviderEvent::Done { reason: StopReason::EndTurn },
            ],
        };
        Ok(Box::pin(stream::iter(events)))
    }
}

async fn snapshot_for_session(
    events: &mut mpsc::Receiver<Event>,
    expected_session_id: &str,
) -> luxe_core::WorkspaceSnapshot {
    timeout(EVENT_WAIT, async {
        loop {
            let Some(event) = events.recv().await else {
                panic!("bridge stopped before emitting the session snapshot");
            };
            if let Event::WorkspaceUpdated(snapshot) = event {
                if snapshot.session.as_ref().map(|session| session.id.as_str())
                    == Some(expected_session_id)
                {
                    break *snapshot;
                }
            }
        }
    })
    .await
    .expect("bridge must emit a session snapshot promptly")
}

#[tokio::test]
async fn bridge_tracks_real_engine_startup_and_new_sessions() {
    let root = tempfile::tempdir().unwrap();
    let mut store = JsonlSessionStore::new(root.path().join("sessions"));
    let initial_session_id = store.create(&root.path().to_string_lossy()).unwrap();
    let config = EngineConfig {
        session_store: std::sync::RwLock::new(Box::new(store)),
        provider: Arc::new(LifecycleProvider { calls: AtomicUsize::new(0) }),
        providers: std::collections::HashMap::new(),
        model: ModelInfo::default(),
        tool_registry: ToolRegistry::new(),
        hook_registry: HookRegistry::new(),
        session_id: initial_session_id.clone(),
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
    let (actions, engine_events) = Engine::spawn(config);
    let mut events =
        bridge(engine_events, Roots::same(root.path().to_path_buf()), None::<PullRequestWatch>);

    let Event::WorkspaceUpdated(initial) = events.recv().await.unwrap() else {
        panic!("expected initial workspace snapshot");
    };
    assert!(initial.session.is_none());
    let initial_snapshot = snapshot_for_session(&mut events, &initial_session_id.0).await;
    assert_eq!(initial_snapshot.session.unwrap().id, initial_session_id.0);

    // Prime the initial session so it carries a message; `New session` then forks a fresh
    // session instead of reusing the (now non-empty) one.
    actions
        .send(Action::SubmitUserMessage {
            content: "prime".into(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();
    timeout(EVENT_WAIT, async {
        loop {
            match events.recv().await {
                Some(Event::TurnEnded { .. }) => break,
                Some(_) => continue,
                None => panic!("engine stopped before the prime turn ended"),
            }
        }
    })
    .await
    .expect("prime turn must end promptly");

    actions.send(Action::NewSession).await.unwrap();
    let new_session_id = timeout(EVENT_WAIT, async {
        loop {
            let Some(event) = events.recv().await else {
                panic!("engine stopped before announcing the new session");
            };
            if let Event::SessionLoaded { session_id, messages } = event {
                assert!(messages.is_empty());
                break session_id;
            }
        }
    })
    .await
    .expect("engine must announce the new session promptly");
    assert_ne!(new_session_id, initial_session_id.0);
    let new_snapshot = snapshot_for_session(&mut events, &new_session_id).await;
    assert_eq!(new_snapshot.session.unwrap().id, new_session_id);

    actions
        .send(Action::SubmitUserMessage {
            content: "run a tool".into(),
            images: vec![],
            remediation: None,
        })
        .await
        .unwrap();
    let (terminal_activity, revisions) = timeout(EVENT_WAIT, async {
        let mut revisions = Vec::new();
        loop {
            let Some(event) = events.recv().await else {
                panic!("bridge stopped before forwarding typed tool activity");
            };
            if let Event::ToolActivityUpdated(activity) = event {
                if activity.call_id == "real-call" {
                    revisions.push(activity.revision);
                    if activity.is_terminal() {
                        break (activity, revisions);
                    }
                }
            }
        }
    })
    .await
    .expect("bridge must project a real tool lifecycle promptly");
    // Composing, settled into the queue when the message landed, dispatched, answered: the
    // card opens while the model is still writing the call and is revised from there, never
    // created a second time.
    assert_eq!(revisions, [0, 1, 2, 3]);
    assert_eq!(terminal_activity.session_id, new_session_id);
    assert_eq!(terminal_activity.state, ToolActivityState::Failed);

    actions.send(Action::Close).await.unwrap();
}

#[tokio::test]
async fn bridge_coalesces_file_changing_tool_completions() {
    let root = tempfile::tempdir().unwrap();
    let (engine_tx, engine_rx) = mpsc::channel(8);
    let mut events =
        bridge(engine_rx, Roots::same(root.path().to_path_buf()), None::<PullRequestWatch>);
    let Event::WorkspaceUpdated(initial) = events.recv().await.unwrap() else {
        panic!("expected initial workspace snapshot");
    };

    for call_id in ["write-1", "edit-1"] {
        let name = if call_id == "write-1" { "write" } else { "edit" };
        engine_tx
            .send(Event::ToolStarted { call_id: call_id.into(), name: name.into() })
            .await
            .unwrap();
        engine_tx
            .send(Event::ToolEnded { call_id: call_id.into(), is_error: false })
            .await
            .unwrap();
    }
    for _ in 0..4 {
        assert!(matches!(
            events.recv().await,
            Some(Event::ToolStarted { .. } | Event::ToolEnded { .. })
        ));
    }

    let Event::WorkspaceUpdated(refreshed) =
        timeout(EVENT_WAIT, events.recv()).await.unwrap().unwrap()
    else {
        panic!("expected coalesced workspace snapshot");
    };
    assert_eq!(refreshed.revision, initial.revision + 1);
    assert!(timeout(REFRESH_DEBOUNCE * 2, events.recv()).await.is_err());
}

#[tokio::test]
async fn bridge_forwards_raw_lifecycle_before_typed_activity() {
    let root = tempfile::tempdir().unwrap();
    let (engine_tx, engine_rx) = mpsc::channel(8);
    let mut events =
        bridge(engine_rx, Roots::same(root.path().to_path_buf()), None::<PullRequestWatch>);
    assert!(matches!(events.recv().await, Some(Event::WorkspaceUpdated(_))));

    engine_tx
        .send(Event::SessionLoaded { session_id: "session-a".into(), messages: vec![] })
        .await
        .unwrap();
    assert!(matches!(events.recv().await, Some(Event::SessionLoaded { .. })));
    let Some(Event::WorkspaceUpdated(snapshot)) = events.recv().await else {
        panic!("expected session snapshot after session load");
    };
    assert_eq!(snapshot.session.as_ref().map(|session| session.id.as_str()), Some("session-a"));

    engine_tx
        .send(Event::ToolStarted { call_id: "call-1".into(), name: "read".into() })
        .await
        .unwrap();
    engine_tx
        .send(Event::ToolProgress { call_id: "call-1".into(), chunk: "reading".into() })
        .await
        .unwrap();
    engine_tx.send(Event::ToolEnded { call_id: "call-1".into(), is_error: false }).await.unwrap();

    assert!(matches!(events.recv().await, Some(Event::ToolStarted { .. })));
    let Some(Event::ToolActivityUpdated(running)) = events.recv().await else {
        panic!("expected running tool activity after raw start");
    };
    assert_eq!(running.revision, 0);
    assert!(matches!(events.recv().await, Some(Event::ToolProgress { .. })));
    let Some(Event::ToolActivityUpdated(progress)) = events.recv().await else {
        panic!("expected progress activity after raw progress");
    };
    assert_eq!(progress.revision, 1);
    assert_eq!(progress.output_preview.as_deref(), Some("reading"));
    assert!(matches!(events.recv().await, Some(Event::ToolEnded { .. })));
    let Some(Event::ToolActivityUpdated(finished)) = events.recv().await else {
        panic!("expected terminal activity after raw completion");
    };
    assert_eq!(finished.revision, 2);
    assert!(finished.is_terminal());
}

#[tokio::test]
async fn bridge_emits_initial_and_explicit_refresh_snapshots() {
    let root = tempfile::tempdir().unwrap();
    let (engine_tx, engine_rx) = mpsc::channel(4);
    let mut events =
        bridge(engine_rx, Roots::same(root.path().to_path_buf()), None::<PullRequestWatch>);

    let Event::WorkspaceUpdated(initial) =
        timeout(EVENT_WAIT, events.recv()).await.unwrap().unwrap()
    else {
        panic!("expected initial workspace snapshot");
    };
    engine_tx.send(Event::WorkspaceRefreshRequested).await.unwrap();
    assert!(matches!(events.recv().await, Some(Event::WorkspaceRefreshRequested)));
    let Event::WorkspaceUpdated(refreshed) = events.recv().await.unwrap() else {
        panic!("expected refreshed workspace snapshot");
    };
    assert_eq!(initial.scope_id, refreshed.scope_id);
    assert_eq!(initial.revision + 1, refreshed.revision);
}

#[tokio::test]
async fn bridge_projects_loaded_guided_review_into_the_next_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let (engine_tx, engine_rx) = mpsc::channel(4);
    let mut events =
        bridge(engine_rx, Roots::same(root.path().to_path_buf()), None::<PullRequestWatch>);
    assert!(matches!(events.recv().await, Some(Event::WorkspaceUpdated(_))));

    let revision_id = ReviewRevisionId::from_canonical_bytes(b"review source");
    let review = GuidedReview {
        conversation: None,
        id: ReviewId::new(),
        revision_id: revision_id.clone(),
        target: ReviewTarget::WorkingTree,
        source: ReviewSource {
            base: ReviewBase::EmptyTree,
            content_digest: revision_id,
            includes_staged: false,
            includes_unstaged: true,
            included_untracked_files: 0,
            captured_at_ms: 5,
            uncovered_paths: Vec::new(),
        },
        context: ReviewContext {
            title: "Pinned working tree".into(),
            goal: None,
            phase_index: None,
            phase_title: None,
            planning_excerpt: None,
            acceptance_excerpt: None,
            source_intent: None,
            source_commits: 0,
            pull_request: None,
        },
        files: Vec::new(),
        chapters: Vec::new(),
        checks: CheckSummary::default(),
        stats: ReviewStats::default(),
        generated_at_ms: 6,
        insight: None,
        provenance: None,
        seen: None,
        brief: None,
    };
    let review_id = review.id.as_str().to_string();
    engine_tx
        .send(Event::GuidedReviewLoaded {
            request_id: luxe_core::ReviewLoadRequestId::new(),
            review: Box::new(review),
        })
        .await
        .unwrap();

    assert!(matches!(events.recv().await, Some(Event::GuidedReviewLoaded { .. })));
    let Some(Event::WorkspaceUpdated(snapshot)) = events.recv().await else {
        panic!("expected workspace snapshot after review load");
    };
    assert_eq!(snapshot.review.as_ref().map(|review| review.id.as_str()), Some(review_id.as_str()));
    assert_eq!(
        snapshot.review.as_ref().map(|review| review.title.as_str()),
        Some("Pinned working tree")
    );
}
