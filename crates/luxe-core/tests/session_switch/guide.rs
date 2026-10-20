//! A guide pass runs on the model the session is on, and bills the session that asked for it.

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use luxe_core::guided_review::{ReviewLoadRequestId, ReviewTarget};
use luxe_core::provider::{Cost, ModelRef, Usage};
use luxe_core::session::EntryKind;
use luxe_core::usage::UsageSource;

use super::*;

/// What one guide call was, as the provider that served it saw it.
#[derive(Clone, Debug, PartialEq)]
struct Asked {
    provider: &'static str,
    model: String,
    max_tokens: Option<u32>,
}

/// Records each call and bills a fixed prompt, optionally waiting for a release first.
struct RecordingProvider {
    name: &'static str,
    asked: Arc<Mutex<Vec<Asked>>>,
    hold: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    held: AtomicBool,
}

impl RecordingProvider {
    fn new(name: &'static str, asked: &Arc<Mutex<Vec<Asked>>>) -> Self {
        Self { name, asked: asked.clone(), hold: None, held: AtomicBool::new(false) }
    }
}

#[async_trait::async_trait]
impl Provider for RecordingProvider {
    fn name(&self) -> &str {
        self.name
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let model = request.model.r#ref.id.clone();
        let max_tokens = request.max_tokens;
        self.asked.lock().unwrap().push(Asked { provider: self.name, model, max_tokens });
        let billed = ProviderEvent::Usage(Usage {
            prompt_tokens: 1_000_000,
            completion_tokens: 0,
            total_tokens: 1_000_000,
            ..Usage::default()
        });
        let done = ProviderEvent::Done { reason: StopReason::EndTurn };
        match &self.hold {
            Some((entered, release)) if !self.held.swap(true, Ordering::SeqCst) => {
                entered.notify_one();
                let release = release.clone();
                Ok(Box::pin(
                    stream::once(async move {
                        release.notified().await;
                        billed
                    })
                    .chain(stream::iter([done])),
                ))
            }
            _ => Ok(Box::pin(stream::iter([billed, done]))),
        }
    }
}

use futures::StreamExt;

fn model(provider: &str, id: &str, window: u32, input_cost: f64) -> ModelInfo {
    ModelInfo {
        r#ref: ModelRef::new(provider, id),
        context_window: window,
        max_tokens: 4_096,
        cost: Cost { input: input_cost, ..Cost::default() },
        ..ModelInfo::default()
    }
}

fn git(root: &std::path::Path, arguments: &[&str]) {
    assert!(Command::new("git").args(arguments).current_dir(root).status().unwrap().success());
}

/// Where the repository keeps its sessions: under `.luxe`, as Luxe does, where Guided Review never
/// looks. Anywhere else they are untracked files a review reads, and on Windows an open session's
/// claim locks its `.lock` file against that read, so the review never loads.
fn sessions_root(root: &std::path::Path) -> std::path::PathBuf {
    root.join(".luxe").join("sessions")
}

/// A repository with one committed file, changed in the working tree.
fn changed_repository() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "tests@example.invalid"]);
    git(root, &["config", "user.name", "Luxe Tests"]);
    std::fs::write(root.join("tracked.rs"), "fn original() {}\n").unwrap();
    git(root, &["add", "tracked.rs"]);
    git(root, &["commit", "-q", "-m", "initial fixture"]);
    std::fs::write(root.join("tracked.rs"), "fn changed() {}\n").unwrap();
    dir
}

type Events = tokio::sync::mpsc::Receiver<Event>;

/// The cost of each guide call billed until a guide lands, and the review if it was published
/// meanwhile: a guide that answers at once can land before its review is announced.
async fn guide_costs(
    events: &mut Events,
) -> (Vec<Option<f64>>, Option<luxe_core::guided_review::GuidedReview>) {
    let mut costs = Vec::new();
    let mut review = None;
    first(events, "the guide", |event| {
        match event {
            Event::GuidedReviewLoaded { review: loaded, .. } => review = Some(*loaded),
            Event::TurnUsage { source: UsageSource::Review, cost_usd, .. } => costs.push(cost_usd),
            Event::GuidedReviewGuideUpdated { .. } => return Some(()),
            _ => {}
        }
        None
    })
    .await;
    (costs, review)
}

/// The guide is written by the model the session switched to, through that model's provider, and
/// priced and bounded by that model — on load and on refresh alike.
///
/// The break this catches: both captured the launch model and the default provider, so after
/// `/model` the guide still went to the old model at the old rates, inside the old window.
#[tokio::test]
async fn the_guide_runs_on_the_model_the_session_switched_to() {
    let repository = changed_repository();
    let root = repository.path();
    let mut store = JsonlSessionStore::new(sessions_root(root));
    let session = store.create("/tmp/a").unwrap();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut config = engine_config(store, session, root);
    config.provider = Arc::new(RecordingProvider::new("launch", &asked));
    config.model = model("launch", "launch-model", 100_000, 1.0);
    config
        .providers
        .insert("switched".into(), Arc::new(RecordingProvider::new("switched", &asked)));
    let (actions, mut events) = Engine::spawn(config);

    actions.send(Action::SetModel(model("switched", "switched-model", 5_000, 7.0))).await.unwrap();
    let request_id = ReviewLoadRequestId::new();
    let load = Action::LoadGuidedReview { request_id, target: ReviewTarget::WorkingTree };
    actions.send(load).await.unwrap();
    let (mut costs, review) = guide_costs(&mut events).await;
    let review = match review {
        Some(review) => review,
        None => {
            first(&mut events, "the review", |event| match event {
                Event::GuidedReviewLoaded { review, .. } => Some(*review),
                _ => None,
            })
            .await
        }
    };
    std::fs::write(root.join("tracked.rs"), "fn changed_again() {}\n").unwrap();
    let request_id = ReviewLoadRequestId::new();
    let refresh = Action::RefreshGuidedReview {
        request_id,
        review_id: review.id.clone(),
        expected_revision: review.revision_id.clone(),
    };
    actions.send(refresh).await.unwrap();
    costs.extend(guide_costs(&mut events).await.0);
    actions.send(Action::Close).await.unwrap();

    let asked = asked.lock().unwrap().clone();
    assert!(asked.len() >= 2, "a guide on load and one on refresh: {asked:?}");
    for call in &asked {
        assert_eq!((call.provider, call.model.as_str()), ("switched", "switched-model"));
        assert!(
            call.max_tokens.is_some_and(|cap| cap < 1_000),
            "bounded by the switched model's window: {call:?}"
        );
    }
    let priced = costs.iter().all(|cost| *cost == Some(7.0));
    assert!(!costs.is_empty() && priced, "priced at the switched model's rate: {costs:?}");
}

/// Guide spend that arrives after a switch settles to the session that asked for it: the open
/// session's live total and its file stay as they were.
///
/// The break this catches: the detached pass held the engine's one ledger, which the switch had
/// just reset for the new session, so the old session's late spend was published as the new one's
/// total and written into its transcript.
#[tokio::test]
async fn late_guide_spend_settles_to_the_session_that_asked() {
    let repository = changed_repository();
    let root = repository.path();
    let mut store = JsonlSessionStore::new(sessions_root(root));
    let asking = store.create("/tmp/a").unwrap();
    let other = store.create("/tmp/b").unwrap();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let mut provider = RecordingProvider::new("launch", &asked);
    provider.hold = Some((entered.clone(), release.clone()));
    let mut config = engine_config(store, asking.clone(), root);
    config.provider = Arc::new(provider);
    config.model = model("launch", "launch-model", 100_000, 1.0);
    let (actions, mut events, task) = Engine::spawn_task(config);

    let load = Action::LoadGuidedReview {
        request_id: ReviewLoadRequestId::new(),
        target: ReviewTarget::WorkingTree,
    };
    actions.send(load).await.unwrap();
    tokio::time::timeout(WAIT, entered.notified()).await.expect("the guide is asked");
    actions.send(Action::LoadSession(other.to_string())).await.unwrap();
    first(&mut events, "the switch", |event| match event {
        Event::SessionLoaded { session_id, .. } if session_id == other.0 => Some(()),
        _ => None,
    })
    .await;
    release.notify_one();
    let mut after_switch = Vec::new();
    first(&mut events, "the guide", |event| {
        let done = matches!(event, Event::GuidedReviewGuideUpdated { .. });
        after_switch.push(event);
        done.then_some(())
    })
    .await;
    actions.send(Action::RefreshWorkspace).await.unwrap();
    first(&mut events, "the next action", |event| {
        let done = matches!(event, Event::WorkspaceRefreshRequested);
        after_switch.push(event);
        done.then_some(())
    })
    .await;
    actions.send(Action::Close).await.unwrap();
    let drained = tokio::spawn(async move { while events.recv().await.is_some() {} });
    tokio::time::timeout(WAIT, task).await.expect("the engine stops").unwrap();
    drained.abort();

    for event in &after_switch {
        if let Event::SessionUsage(usage) = event {
            assert_eq!(usage.total.prompt_tokens, 0, "the open session's total moved: {usage:?}");
        }
        assert!(!matches!(event, Event::TurnUsage { .. }), "published to the wrong session");
    }
    let usage_in = |session: &SessionId| {
        let entries = JsonlSessionStore::new(sessions_root(root)).load(session).unwrap();
        entries
            .iter()
            .filter(|entry| {
                matches!(entry.kind, EntryKind::Usage { source: UsageSource::Review, .. })
            })
            .count()
    };
    assert_eq!(usage_in(&other), 0, "nothing written to the session that did not ask");
    assert_eq!(usage_in(&asking), 1, "the spend is on the session that asked");
}

/// Guide spend that lands after switching away and back joins the open session: its readout
/// counts it, and the conversation goes on after it rather than beside it.
///
/// The break this catches: the late spend was written after the session's latest entry while the
/// engine kept its own leaf, so the next message hung off the old one and the record sat on a
/// dead branch, and the session's total never included it — nor was the readout told until the
/// session's next call of its own.
#[tokio::test]
async fn late_guide_spend_joins_its_session_when_it_is_open_again() {
    let repository = changed_repository();
    let root = repository.path();
    let mut store = JsonlSessionStore::new(sessions_root(root));
    let asking = store.create("/tmp/a").unwrap();
    let other = store.create("/tmp/b").unwrap();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let mut provider = RecordingProvider::new("launch", &asked);
    provider.hold = Some((entered.clone(), release.clone()));
    let mut config = engine_config(store, asking.clone(), root);
    config.provider = Arc::new(provider);
    config.model = model("launch", "launch-model", 100_000, 1.0);
    let (actions, mut events, task) = Engine::spawn_task(config);

    let load = Action::LoadGuidedReview {
        request_id: ReviewLoadRequestId::new(),
        target: ReviewTarget::WorkingTree,
    };
    actions.send(load).await.unwrap();
    tokio::time::timeout(WAIT, entered.notified()).await.expect("the guide is asked");
    for session in [&other, &asking] {
        actions.send(Action::LoadSession(session.to_string())).await.unwrap();
        first(&mut events, "the switch", |event| match event {
            Event::SessionLoaded { session_id, .. } if session_id == session.0 => Some(()),
            _ => None,
        })
        .await;
    }
    release.notify_one();
    guide_costs(&mut events).await;
    actions.send(Action::RefreshWorkspace).await.unwrap();
    let mut announced = None;
    first(&mut events, "the next action", |event| match event {
        Event::SessionUsage(usage) => {
            announced = Some(usage);
            None
        }
        Event::WorkspaceRefreshRequested => Some(()),
        _ => None,
    })
    .await;
    let review_tokens = |usage: luxe_core::usage::SessionUsage| {
        let review = usage.by_source.into_iter().find(|entry| entry.source == UsageSource::Review);
        review.map(|entry| entry.totals.prompt_tokens)
    };
    let announced = announced.expect("the readout is told at once");
    assert_eq!(review_tokens(announced), Some(1_000_000), "announced before any call of its own");
    let content = "after the guide".to_string();
    let submit = Action::SubmitUserMessage { content, images: vec![], remediation: None };
    actions.send(submit).await.unwrap();
    let mut totals = None;
    first(&mut events, "the turn", |event| match event {
        Event::SessionUsage(usage) => {
            totals = Some(usage);
            None
        }
        Event::TurnEnded { .. } => Some(()),
        _ => None,
    })
    .await;
    actions.send(Action::Close).await.unwrap();
    let drained = tokio::spawn(async move { while events.recv().await.is_some() {} });
    tokio::time::timeout(WAIT, task).await.expect("the engine stops").unwrap();
    drained.abort();

    let totals = totals.expect("a session total");
    assert_eq!(review_tokens(totals), Some(1_000_000), "and still counted after the turn");
    let store = JsonlSessionStore::new(sessions_root(root));
    let entries = store.load(&asking).unwrap();
    let leaf = luxe_core::session::latest_leaf(&entries).unwrap();
    let path = store.active_path(&asking, &leaf).unwrap();
    let on_path = path
        .iter()
        .filter(|entry| matches!(entry.kind, EntryKind::Usage { source: UsageSource::Review, .. }))
        .count();
    assert_eq!(on_path, 1, "the record is on the conversation's path");
}
