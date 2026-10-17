//! Workspace runtime — bridges engine events to revisioned workspace snapshots.
//!
//! The CLI owns the collector and event fan-out, keeping project inspection out of both
//! front-ends.

use crate::activity_projector::ToolActivityProjector;
use crate::pull_request_watch::PullRequestWatch;
use crate::workspace_data::WorkspaceCollector;
use luxe_core::github::{Branches, Discovery};
use luxe_core::Event;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tokio::time::{sleep_until, Duration, Instant};

const REFRESH_DEBOUNCE: Duration = Duration::from_millis(150);

/// The two places a session touches: where its state is kept, and where its agent works.
#[derive(Debug, Clone)]
pub(crate) struct Roots {
    /// The project, where `.luxe/` keeps sessions, todos and reviews
    pub(crate) state: PathBuf,
    /// The checkout the agent works in: the project itself, or the session's own worktree
    pub(crate) work: PathBuf,
}

#[cfg(test)]
impl Roots {
    /// One path for both, as every session without a worktree has.
    pub(crate) fn same(root: PathBuf) -> Self {
        Self { state: root.clone(), work: root }
    }
}

/// Starts the CLI-owned workspace event bridge.
///
/// `watch`, when given, keeps the branch's pull request in the snapshot; the TUI and `--rpc` pass
/// one, and nothing else runs this bridge, so nothing polls GitHub where nobody is looking.
pub(crate) fn bridge<G: Discovery + Branches + ?Sized + 'static>(
    mut engine_rx: mpsc::Receiver<Event>,
    roots: Roots,
    mut watch: Option<PullRequestWatch<G>>,
) -> mpsc::Receiver<Event> {
    let (event_tx, event_rx) = mpsc::channel(256);
    tokio::spawn(async move {
        let mut collector = WorkspaceCollector::new(roots.work).with_state_root(roots.state);
        let (looked_tx, mut looked_rx) = mpsc::channel(4);
        let first = snapshot(&mut collector, watch.as_mut()).await;
        if event_tx.send(Event::WorkspaceUpdated(Box::new(first))).await.is_err() {
            return;
        }

        let mut activity_projector = ToolActivityProjector::default();
        let mut file_changing_calls = HashMap::new();
        let mut refresh_at = None;
        loop {
            let watch_due = watch.as_ref().and_then(|watch| watch.due(Instant::now()));
            tokio::select! {
                event = engine_rx.recv() => {
                    let Some(event) = event else { break };
                    if let Some(watch) = watch.as_mut() {
                        watch.observe(&event, Instant::now());
                    }
                    if !forward_event(event, &event_tx, &mut collector, &mut activity_projector, &mut file_changing_calls, &mut refresh_at).await {
                        return;
                    }
                }
                _ = sleep_until_some(refresh_at) => {
                    refresh_at = None;
                    let snapshot = snapshot(&mut collector, watch.as_mut()).await;
                    if event_tx.send(Event::WorkspaceUpdated(Box::new(snapshot))).await.is_err() {
                        return;
                    }
                }
                _ = sleep_until_some(watch_due) => {
                    if let Some(watch) = watch.as_mut() {
                        watch.start(Instant::now(), &looked_tx);
                    }
                }
                Some(looked) = looked_rx.recv() => {
                    let Some(watch) = watch.as_mut() else { continue };
                    if let Some(presence) = watch.finish(looked, Instant::now()) {
                        collector.set_pull_request(presence);
                        refresh_at = Some(Instant::now());
                    }
                }
            }
        }
    });
    event_rx
}

/// The watch the TUI and `--rpc` run: the user's own `gh`, at the interval their global config
/// sets.
pub(crate) fn pull_request_watch(root: &std::path::Path) -> Option<PullRequestWatch> {
    let refresh_secs = luxe_core::config::Config::load(root).github.refresh_secs();
    Some(PullRequestWatch::new(
        std::sync::Arc::new(luxe_core::github::GhCli::new()),
        root.to_path_buf(),
        refresh_secs,
    ))
}

/// A fresh snapshot, with the pull request forgotten when the branch under it changed.
async fn snapshot<G: Discovery + Branches + ?Sized + 'static>(
    collector: &mut WorkspaceCollector,
    watch: Option<&mut PullRequestWatch<G>>,
) -> luxe_core::WorkspaceSnapshot {
    let mut snapshot = collector.collect().await;
    if let Some(watch) = watch {
        if watch.saw_branch(snapshot.project.git_branch.as_deref(), Instant::now()) {
            collector.set_pull_request(luxe_core::workspace::PullRequestPresence::Unknown);
            snapshot.pull_request = luxe_core::workspace::PullRequestPresence::Unknown;
        }
    }
    snapshot
}

/// Sleeps until `deadline`, or forever when there is none.
async fn sleep_until_some(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

async fn forward_event(
    event: Event,
    event_tx: &mpsc::Sender<Event>,
    collector: &mut WorkspaceCollector,
    activity_projector: &mut ToolActivityProjector,
    file_changing_calls: &mut HashMap<String, bool>,
    refresh_at: &mut Option<Instant>,
) -> bool {
    let refresh_now = matches!(
        &event,
        Event::WorkspaceRefreshRequested
            | Event::SessionLoaded { .. }
            | Event::GuidedReviewLoaded { .. }
            | Event::ReviewSessionSaved { .. }
            | Event::ReviewSessionRestored { .. }
    );
    if let Event::SessionLoaded { session_id, .. } = &event {
        collector.set_session(session_id.clone());
    }
    if let Event::GuidedReviewLoaded { review, .. } = &event {
        collector.set_guided_review(review);
    }
    // Every persisted feedback change re-reads the dock's open-finding count from the session.
    match &event {
        Event::ReviewSessionSaved { session, .. }
        | Event::ReviewSessionRestored { session, .. } => collector.set_review_findings(session),
        _ => {}
    }
    let refresh_after_tool = match &event {
        Event::ToolStarted { call_id, name } => {
            file_changing_calls.insert(call_id.clone(), changes_workspace(name));
            false
        }
        Event::ToolEnded { call_id, .. } => file_changing_calls.remove(call_id).unwrap_or(false),
        _ => false,
    };
    let activities = activity_projector.project(&event);
    if event_tx.send(event).await.is_err() {
        return false;
    }
    for activity in activities {
        if event_tx.send(Event::ToolActivityUpdated(Box::new(activity))).await.is_err() {
            return false;
        }
    }
    if refresh_now {
        *refresh_at = Some(Instant::now());
    } else if refresh_after_tool {
        *refresh_at = Some(Instant::now() + REFRESH_DEBOUNCE);
    }
    true
}

fn changes_workspace(name: &str) -> bool {
    matches!(name, "write" | "edit" | "bash" | "task" | "todo" | "git")
}

#[cfg(test)]
mod tests;
