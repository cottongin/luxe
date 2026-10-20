//! Loop controller — manages the native continuous development loop state and orchestration.
//!
//! The `LoopController` owns [`LoopState`] and provides methods to check/modify loop state,
//! determine when to continue/stop, and coordinate with hooks for self-continuation,
//! context-aware termination, journal bounding, phase tracking, and runaway guards.

use crate::hook::HookRegistry;
pub use crate::loop_state::{LoopLiveness, LoopOwner, LoopSpend, LoopState};
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{watch, Mutex};
use tracing::debug;

/// Errors from loop controller operations.
#[derive(Debug, Error)]
pub enum LoopError {
    /// IO error (file read/write).
    #[error("IO error: {0}")]
    Io(std::io::Error),

    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(serde_json::Error),

    /// Generic error (from LoopState methods).
    #[error("{0}")]
    Other(String),
}

crate::error_cause::from_cause!(LoopError {
    std::io::Error => Io,
    serde_json::Error => Json,
});

/// The loop controller — owns LoopState and provides orchestration methods.
#[derive(Clone)]
pub struct LoopController {
    /// Persistent loop state.
    state: Arc<Mutex<LoopState>>,
    /// Path to the state.json file.
    state_path: PathBuf,
    /// Path to the STOP sentinel file.
    stop_sentinel_path: PathBuf,
    /// Hook registry for the loop hooks (self-continuation, termination, guards, etc.).
    hook_registry: HookRegistry,
    /// Watch sender for the running flag — consumers can await state changes.
    running_tx: watch::Sender<bool>,
}

impl std::fmt::Debug for LoopController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoopController")
            .field("state_path", &self.state_path)
            .field("stop_sentinel_path", &self.stop_sentinel_path)
            .finish()
    }
}

impl LoopController {
    /// Create a new LoopController with the given state file paths.
    pub fn new(state_path: PathBuf, stop_sentinel_path: PathBuf) -> Self {
        let (running_tx, _) = watch::channel(false);
        Self {
            state: Arc::new(Mutex::new(LoopState::default())),
            state_path,
            stop_sentinel_path,
            hook_registry: HookRegistry::new(),
            running_tx,
        }
    }

    /// Set the hook registry for this controller (called by build_default_hooks).
    pub fn set_hook_registry(&mut self, registry: HookRegistry) {
        self.hook_registry = registry;
    }

    /// Get a reference to the hook registry.
    pub fn hook_registry(&self) -> &HookRegistry {
        &self.hook_registry
    }

    /// Load state from disk, or return default if not found.
    pub async fn load(&self) -> Result<(), LoopError> {
        let loaded =
            LoopState::load(&self.state_path).map_err(|e| LoopError::Other(e.to_string()))?;
        let iteration = loaded.iteration;
        let phase = loaded.current_phase;
        *self.state.lock().await = loaded;
        debug!(iteration, phase, "loop state loaded");
        Ok(())
    }

    /// Save current state to disk.
    pub async fn save(&self) -> Result<(), LoopError> {
        let state = self.state.lock().await;
        state.save(&self.state_path).map_err(|e| LoopError::Other(e.to_string()))?;
        Ok(())
    }

    /// Get a clone of the current state.
    pub async fn get_state(&self) -> LoopState {
        self.state.lock().await.clone()
    }

    /// Check if the loop is currently running.
    pub async fn is_running(&self) -> bool {
        let state = self.state.lock().await;
        state.running && !LoopState::is_stopped(&self.stop_sentinel_path)
    }

    /// Get a watch receiver for the running flag.
    pub fn running_rx(&self) -> watch::Receiver<bool> {
        self.running_tx.subscribe()
    }

    /// Start the loop (set running=true, increment iteration, record the driving process).
    pub async fn start(&self) -> Result<(), LoopError> {
        let mut state = self.state.lock().await;
        state.running = true;
        state.iteration += 1;
        // Who to ask about later. A `running: true` with nothing behind it is what let
        // `luxe loop status` report a live run for a process that had been gone for hours.
        state.owner = Some(crate::loop_state::LoopOwner::current());
        drop(state);
        // `stop` leaves a sentinel file behind, and `is_running` reads it as well as the flag — so
        // starting without clearing it sets `running = true` over a loop that every hook still
        // treats as stopped. The symptom is a `luxe loop start` that prints its banner and then
        // does nothing at all, for as long as `.luxe/STOP` exists.
        LoopState::remove_stop_sentinel(&self.stop_sentinel_path)
            .map_err(|e| LoopError::Other(e.to_string()))?;
        self.save().await?;
        self.running_tx.send_replace(true);
        debug!(iteration = self.state.lock().await.iteration, "loop started");
        Ok(())
    }

    /// Increment iteration (called at the start of each turn).
    pub async fn increment_iteration(&self) {
        let mut state = self.state.lock().await;
        state.iteration += 1;
        debug!(iteration = state.iteration, "iteration incremented");
    }

    /// Stop the loop (create STOP sentinel, set running=false).
    pub async fn stop(&self) -> Result<(), LoopError> {
        let mut state = self.state.lock().await;
        state.running = false;
        // Nothing owns a stopped run, and a kept id is one a later process can be mistaken for.
        state.owner = None;
        drop(state);
        LoopState::create_stop_sentinel(&self.stop_sentinel_path)
            .map_err(|e| LoopError::Other(e.to_string()))?;
        self.save().await?;
        self.running_tx.send_replace(false);
        debug!("loop stopped");
        Ok(())
    }

    /// Pause the loop (set running=false, keep iteration/phase).
    pub async fn pause(&self) -> Result<(), LoopError> {
        let mut state = self.state.lock().await;
        state.running = false;
        state.owner = None;
        drop(state);
        self.save().await?;
        self.running_tx.send_replace(false);
        debug!("loop paused");
        Ok(())
    }

    /// Resume the loop (set running=true, preserve iteration/phase).
    ///
    /// Deliberately records no owner: `resume` clears a run to continue, and `luxe loop start` is
    /// what actually drives one. Claiming this process as the owner would be a lie the moment the
    /// command exited.
    pub async fn resume(&self) -> Result<(), LoopError> {
        let mut state = self.state.lock().await;
        state.running = true;
        state.owner = None;
        drop(state);
        LoopState::remove_stop_sentinel(&self.stop_sentinel_path)
            .map_err(|e| LoopError::Other(e.to_string()))?;
        self.save().await?;
        self.running_tx.send_replace(true);
        debug!("loop resumed");
        Ok(())
    }

    /// Record where the current phase starts, so a review can be diffed against it.
    ///
    /// Called when the loop starts and after every phase advance. Best-effort: a project that is not
    /// a git repository has no baseline, and a review there falls back to the working tree.
    pub async fn record_phase_baseline(&self, repo_root: &std::path::Path) {
        let head = tokio::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo_root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .output()
            .await;
        let baseline = match head {
            Ok(output) if output.status.success() => {
                let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
                (!sha.is_empty()).then_some(sha)
            }
            _ => None,
        };
        match &baseline {
            Some(sha) => debug!(%sha, "phase baseline recorded"),
            None => debug!("no phase baseline: not a git repository, or no commits yet"),
        }
        {
            let mut state = self.state.lock().await;
            state.phase_baseline = baseline;
        }
        if let Err(error) = self.save().await {
            // the baseline is still held in memory; only a restart would lose it
            tracing::warn!(%error, "the phase baseline could not be saved");
        }
    }

    /// The commit the current phase started from, if one was recorded.
    pub async fn phase_baseline(&self) -> Option<String> {
        self.state.lock().await.phase_baseline.clone()
    }

    /// Advance to the next phase, on disk first.
    ///
    /// The new phase is saved before it is published in memory, so a failed save leaves both the
    /// file and the live loop on the old phase rather than telling the agent about a phase a
    /// restart would not remember.
    pub async fn advance_phase(&self) -> Result<(), LoopError> {
        let mut state = self.state.lock().await;
        let mut advanced = state.clone();
        advanced.current_phase += 1;
        advanced.save(&self.state_path).map_err(|e| LoopError::Other(e.to_string()))?;
        *state = advanced;
        debug!(phase = state.current_phase, "phase advanced");
        Ok(())
    }

    /// Set the current phase explicitly.
    pub async fn set_phase(&self, phase: u32) -> Result<(), LoopError> {
        let mut state = self.state.lock().await;
        state.current_phase = phase;
        drop(state);
        self.save().await?;
        debug!(phase, "phase set");
        Ok(())
    }

    /// Update a field in the loop state (for testing).
    pub async fn update_state(&self, f: impl FnOnce(&mut LoopState)) {
        let mut state = self.state.lock().await;
        f(&mut state);
    }

    /// Record one provider call against the run's spend.
    ///
    /// Held only long enough to add: a dropped figure would under-report the run, which is the
    /// defect this exists to fix. Persisted with the rest of the state at the end of the turn.
    pub async fn record_spend(&self, model_id: &str, totals: &crate::usage::UsageTotals) {
        self.state.lock().await.spend.record(model_id, totals);
    }

    /// Check if context usage exceeds the pause threshold (should end run for compaction).
    pub async fn should_pause_for_compaction(&self, effective_percent: f64) -> bool {
        let state = self.state.lock().await;
        state.should_pause_for_compaction(effective_percent)
    }

    /// Check if we're in the grace period (first N turns of a run).
    pub async fn is_grace_period(&self, iteration: u64) -> bool {
        let state = self.state.lock().await;
        state.is_grace_period(iteration)
    }

    /// Check if the loop should terminate due to runaway guards.
    pub async fn check_runaway_guards(&self) -> LoopTerminationReason {
        let state = self.state.lock().await;
        if !state.running {
            return LoopTerminationReason::Stopped;
        }
        if LoopState::is_stopped(&self.stop_sentinel_path) {
            return LoopTerminationReason::Stopped;
        }
        if state.iteration >= state.max_turns_per_run as u64 {
            return LoopTerminationReason::MaxTurnsReached;
        }
        LoopTerminationReason::Continue
    }

    /// Get a status summary for UI display.
    pub async fn status_summary(&self) -> LoopStatus {
        let state = self.state.lock().await;
        let running = state.running && !LoopState::is_stopped(&self.stop_sentinel_path);
        LoopStatus {
            running,
            liveness: state.liveness(),
            iteration: state.iteration,
            current_phase: state.current_phase,
            max_turns_per_run: state.max_turns_per_run,
            compact_percent: state.compact_percent,
            pause_percent: state.pause_percent,
        }
    }

    /// The live `luxe loop start` already driving this project, if there is one.
    pub async fn owned_by_another_process(&self) -> Option<u32> {
        self.state.lock().await.owned_by_another_process()
    }
}

/// Reason why the loop terminated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopTerminationReason {
    /// Loop was stopped (STOP sentinel or running=false).
    Stopped,
    /// Maximum turns per run reached.
    MaxTurnsReached,
    /// Context-aware termination (pause_percent threshold).
    ContextPause,
    /// Completion challenge not confirmed.
    CompletionNotConfirmed,
    /// Runaway guard (no progress/stalls).
    RunawayGuard,
    /// Continue running.
    Continue,
}

/// Status summary for UI display.
#[derive(Debug, Clone)]
pub struct LoopStatus {
    pub running: bool,
    /// What can be said about the process behind `running`.
    pub liveness: crate::loop_state::LoopLiveness,
    pub iteration: u64,
    pub current_phase: u32,
    pub max_turns_per_run: u32,
    pub compact_percent: u8,
    pub pause_percent: u8,
}

impl LoopStatus {
    /// Whether a `luxe loop start` is actually driving this run right now.
    ///
    /// The flag alone cannot answer it: a killed run leaves `running: true` behind and has no way
    /// to correct it. An unrecorded owner reads as running, because that is what the file says and
    /// there is nothing to contradict it — a state written by an older luxe, or a run marked
    /// resumable and not yet started.
    pub fn is_live(&self) -> bool {
        self.running && !matches!(self.liveness, crate::loop_state::LoopLiveness::Gone { .. })
    }

    /// What the run's process says about it, in one line, or `None` when there is nothing to add.
    pub fn liveness_line(&self) -> Option<String> {
        use crate::loop_state::LoopLiveness;
        match &self.liveness {
            LoopLiveness::Alive { pid } => Some(format!("driven by pid {pid}")),
            LoopLiveness::Gone { pid } => Some(format!(
                "the process that was driving it (pid {pid}) is gone — `luxe loop start` picks the \
                 run up from here"
            )),
            LoopLiveness::Elsewhere { host } => Some(format!(
                "started on {host}, so this machine cannot tell whether it still runs"
            )),
            LoopLiveness::Unrecorded => None,
        }
    }
}

impl std::fmt::Display for LoopStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_live() {
            write!(
                f,
                "loop: iter {} / phase {} ({}% / {}% context)",
                self.iteration, self.current_phase, self.compact_percent, self.pause_percent
            )
        } else {
            write!(f, "loop: paused")
        }
    }
}

#[cfg(test)]
mod tests;
