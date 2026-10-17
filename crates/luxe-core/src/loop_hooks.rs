//! Default hooks for the continuous development loop.
//!
//! Assembles the standard set of hooks used by the native continuous loop:
//! self-continuation, context-aware termination, journal bounding, review integration
//! (which owns phase advancement), completion challenging, runaway guards, and fresh session.

use crate::hook::HookRegistry;
use crate::loop_controller::LoopController;
use std::path::PathBuf;
use std::sync::Arc;

/// Where the loop keeps its journal, under the repository root.
///
/// Its own directory, because the loop rewrites and archives these files: pointing it at a path it
/// does not own means renaming over a file somebody else is keeping.
pub const LOOP_DIR: &str = "loop";
/// The loop's running journal, relative to [`LOOP_DIR`].
pub const JOURNAL_FILE: &str = "PROGRESS.md";
/// Where the journal's older entries accumulate, relative to [`LOOP_DIR`].
pub const ARCHIVE_FILE: &str = "PROGRESS.archive.md";

/// How the loop's journal directory is written when the agent is told where to look.
///
/// Project-relative on purpose: an absolute path in a steer message is a volatile byte in something
/// the model reads every iteration, and it tells the agent nothing it can use that the relative form
/// does not.
pub fn journal_dir_display() -> String {
    format!(".luxe/{LOOP_DIR}")
}

/// The loop's journal and archive paths for a repository.
pub fn journal_paths(repo_root: &std::path::Path) -> (PathBuf, PathBuf) {
    let dir = repo_root.join(".luxe").join(LOOP_DIR);
    (dir.join(JOURNAL_FILE), dir.join(ARCHIVE_FILE))
}

/// Create the journal the loop tells the agent to read, if it is not there yet.
///
/// Every iteration's steer opens with "read the journal to recall state". Without this the first
/// thing the agent does each time is fail to read a file that does not exist — noise in the
/// transcript and a wasted tool call.
async fn ensure_journal(journal_path: &std::path::Path) {
    if journal_path.exists() {
        return;
    }
    if let Some(parent) = journal_path.parent() {
        if tokio::fs::create_dir_all(parent).await.is_err() {
            return;
        }
    }
    // A heading rather than an empty file, so the first read tells the agent what the file is for.
    let seeded = "# Loop journal\n\nWhat this loop has done, newest last. Append a short entry \
                  per iteration: what changed, what it proved, what is next.\n";
    if let Err(error) = tokio::fs::write(journal_path, seeded).await {
        tracing::warn!(%error, path = %journal_path.display(), "could not seed the loop journal");
    }
}

/// Build the default set of hooks for the continuous development loop.
///
/// Returns a configured [`HookRegistry`] with all standard loop hooks registered. `repo_root` is
/// the tree the loop is working in: the journal lives under it and the re-anchor hook reads its
/// planning files.
///
/// Takes the root rather than individual paths on purpose: a call site that hands the journal path
/// to a hook that wants a directory reads files that can never exist and tells the model it was
/// re-anchored from a memory it never loaded.
pub async fn build_default_hooks(
    controller: Arc<LoopController>,
    repo_root: PathBuf,
) -> HookRegistry {
    build_hooks(controller, repo_root, Arc::new(crate::phase_gate::PhaseGate::new())).await
}

/// [`build_default_hooks`], with the phase gate supplied.
///
/// `gate` is what both a `progress` claim and a `complete` claim have to clear. A default one
/// accepts everything, which is what a loop with no acceptance commands and no reviewer model has.
pub async fn build_hooks(
    controller: Arc<LoopController>,
    repo_root: PathBuf,
    gate: Arc<crate::phase_gate::PhaseGate>,
) -> HookRegistry {
    let mut registry = HookRegistry::new();
    let (journal_path, archive_path) = journal_paths(&repo_root);
    ensure_journal(&journal_path).await;
    // Where this phase starts. Without it a review sees the working tree against `HEAD`, which is
    // empty the moment the agent commits its own work.
    controller.record_phase_baseline(&repo_root).await;

    // Runaway guards own the error count, and self-continuation is what waits, so the guards are
    // built first and shared: a failed turn now paces the next iteration instead of being retried
    // immediately.
    let guards = Arc::new(crate::hooks::runaway_guards::RunawayGuardsHook::new(controller.clone()));

    // Self-continuation: enqueues next iteration on AgentEnd. The phase the loop starts on is
    // already briefed by the kickoff message, so the first `AgentEnd` does not send that plan twice.
    let starting_phase = controller.get_state().await.current_phase;
    registry.register(Arc::new(
        crate::hooks::self_continuation::SelfContinuationHook::new(controller.clone())
            .with_pacing(guards.clone())
            .in_project(&repo_root)
            .briefed_on(starting_phase),
    ));

    // Context-aware termination: compact-nudge + pause-end + grace window.
    registry.register(Arc::new(
        crate::hooks::context_aware_termination::ContextAwareTerminationHook::new(
            controller.clone(),
        ),
    ));

    // Journal bounding: auto-archives PROGRESS.md past size cap.
    registry.register(Arc::new(crate::hooks::journal_bounding::JournalBoundingHook::new(
        journal_path,
        archive_path,
        24, // 24 KB default
    )));

    // Review integrator: gates a claimed phase, and advances the phase when it passes.
    registry.register(Arc::new(
        crate::hooks::review_integrator::ReviewIntegratorHook::new(controller.clone())
            .with_gate(gate.clone())
            .in_project(&repo_root),
    ));

    // Completion challenger: forces verification + confirmations before ending.
    // The same gate: `complete` is a claim too.
    let completion_hook =
        crate::hooks::completion_challenger::CompletionChallengerHook::new(controller.clone())
            .await
            .with_gate(gate);
    registry.register(Arc::new(completion_hook));

    // Runaway guards: max turns/no-progress/stalls, and the error count behind the backoff above.
    registry.register(guards);

    // Fresh session: re-anchors from repo memory, once, bounded.
    registry.register(Arc::new(crate::hooks::fresh::FreshHook::new(repo_root)));

    registry
}
