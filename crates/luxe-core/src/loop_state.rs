//! Loop state — persistent state for the native continuous development loop.
//!
//! Tracks whether the loop is running, current phase, iteration count, and configuration
//! for context-aware termination, journal bounding, and runaway guards. State is persisted
//! to `state.json` so it survives reloads and can be inspected/stopped externally.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Persistent state for the continuous development loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopState {
    /// Whether the loop is currently running.
    pub running: bool,
    /// Current iteration count (increments each turn).
    pub iteration: u64,
    /// Current phase index (0-based).
    pub current_phase: u32,
    /// Commit the current phase started from, when the project is a git repository.
    ///
    /// What the phase review is diffed against. Without it the review compares the working tree to
    /// `HEAD`, which shows nothing at all once the agent commits its own work — and committing is
    /// the normal, desirable thing for it to do. A reviewer handed an empty diff declines to judge,
    /// and the phase advances on its acceptance commands alone.
    #[serde(default)]
    pub phase_baseline: Option<String>,
    /// Maximum turns per run before forced pause.
    pub max_turns_per_run: u32,
    /// Context usage percentage the engine compacts at (0-100), as `luxe loop start` set it
    pub compact_percent: u8,
    /// Context usage percentage to end the run for compaction (0-100).
    pub pause_percent: u8,
    /// Maximum journal size in KB before auto-archive.
    pub journal_max_kb: u32,
    /// Number of confirmations required before completion.
    pub confirmations_required: u8,
    /// Grace period at run start (turns) before context nudges apply.
    pub context_grace_turns: u32,
    /// Maximum consecutive turns with no progress before force-end.
    pub max_no_progress_turns: u32,
    /// Maximum stalls before pausing for human.
    pub max_stalls: u32,
    /// Rate limit backoff in milliseconds.
    pub rate_limit_backoff_ms: u64,
    /// Compaction timeout in milliseconds.
    pub compact_timeout_ms: u64,
    /// Maximum consecutive errors before pausing.
    pub max_consecutive_errors: u32,
    /// What this run has spent, across every model and session it used.
    #[serde(default)]
    pub spend: LoopSpend,
    /// The process driving this run, while one is.
    ///
    /// `running` is a record of intent and nothing more: a run that was killed, crashed, or lost
    /// its terminal never gets to correct it, so `luxe loop status` reported a live loop for as
    /// long as the file said so. This is what makes the claim checkable.
    #[serde(default)]
    pub owner: Option<LoopOwner>,
}

/// The process a run belongs to, and the machine it belongs to it on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopOwner {
    /// Process id of the `luxe loop start` that is driving the run
    pub pid: u32,
    /// The machine that id means something on
    pub host: String,
    /// Unix seconds at which the run started
    pub started_at: i64,
}

impl LoopOwner {
    /// This process, now.
    pub fn current() -> Self {
        Self {
            pid: std::process::id(),
            host: crate::program::hostname(),
            started_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_secs() as i64)
                .unwrap_or_default(),
        }
    }

    /// Whether this record describes a process still running on this machine.
    ///
    /// A record written elsewhere answers `Unknown` rather than guessing: a process id from another
    /// box can name a live stranger here, and a project directory does travel — a synced folder, a
    /// network share, a container mount.
    pub fn liveness(&self) -> LoopLiveness {
        if self.host != crate::program::hostname() {
            return LoopLiveness::Elsewhere { host: self.host.clone() };
        }
        match crate::program::process_is_alive(self.pid) {
            true => LoopLiveness::Alive { pid: self.pid },
            false => LoopLiveness::Gone { pid: self.pid },
        }
    }
}

/// What can be said about the process behind a `running: true`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopLiveness {
    /// A `luxe loop start` is driving this run right now.
    Alive {
        /// The process driving it
        pid: u32,
    },
    /// The run recorded a process, and that process is gone.
    Gone {
        /// The process that was driving it
        pid: u32,
    },
    /// The run was started on another machine, so its process id says nothing here.
    Elsewhere {
        /// The machine that wrote the record
        host: String,
    },
    /// Nothing was recorded: a state file written before luxe kept this, or a run marked
    /// resumable by `luxe loop resume`, which flags the state without driving anything.
    Unrecorded,
}

/// A loop run's spend, which no session ledger can hold.
///
/// A [`crate::usage::UsageLedger`] measures one session and is reset and rebuilt whenever a session
/// is loaded — and a run loads several: it starts fresh sessions as it goes, and every phase review
/// runs in an engine and a session of its own. So the run's total lives here, beside the phase
/// counter, and survives a resume for the same reason that does.
///
/// Broken down by model because that is the actionable question. The reviewer is usually the strong,
/// expensive one, and "is it worth what it costs" needs its share, not the sum.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LoopSpend {
    /// Every model's spend added together
    pub total: crate::usage::UsageTotals,
    /// Spend per model id, in display order
    pub by_model: std::collections::BTreeMap<String, crate::usage::UsageTotals>,
}

impl LoopSpend {
    /// One line per model plus a total, for a status readout.
    ///
    /// Empty when the run has spent nothing, so a caller can print the section or leave it out.
    /// Tokens always; money only where the provider priced it, because a subscription's plan
    /// equivalent is not spend the user incurred.
    pub fn summary_lines(&self) -> Vec<String> {
        if self.total.calls == 0 {
            return Vec::new();
        }
        let line = |label: &str, totals: &crate::usage::UsageTotals| {
            let tokens = totals.prompt_tokens + totals.completion_tokens;
            let money = if totals.cost_usd > 0.0 {
                format!(" — ${:.4}", totals.cost_usd)
            } else if totals.plan_usd > 0.0 {
                format!(" — ${:.4} on plan", totals.plan_usd)
            } else {
                String::new()
            };
            format!("{label}: {} calls, {tokens} tokens{money}", totals.calls)
        };

        let mut lines: Vec<String> =
            self.by_model.iter().map(|(model, totals)| line(model, totals)).collect();
        if self.by_model.len() > 1 {
            lines.push(line("all models", &self.total));
        }
        lines
    }

    /// Add one provider call.
    pub fn record(&mut self, model_id: &str, totals: &crate::usage::UsageTotals) {
        self.total.merge(totals);
        self.by_model.entry(model_id.to_string()).or_default().merge(totals);
    }
}

impl Default for LoopState {
    fn default() -> Self {
        Self {
            running: false,
            iteration: 0,
            current_phase: 0,
            phase_baseline: None,
            max_turns_per_run: 100,
            compact_percent: 75,
            pause_percent: 90,
            journal_max_kb: 24,
            confirmations_required: 2,
            context_grace_turns: 2,
            max_no_progress_turns: 12,
            max_stalls: 5,
            rate_limit_backoff_ms: 30_000,
            compact_timeout_ms: 600_000,
            max_consecutive_errors: 5,
            spend: LoopSpend::default(),
            owner: None,
        }
    }
}

impl LoopState {
    /// Load state from a file, or return default if not found.
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        if path.exists() {
            let content = std::fs::read_to_string(path)?;
            let state: LoopState = serde_json::from_str(&content)?;
            Ok(state)
        } else {
            Ok(Self::default())
        }
    }

    /// Save state to a file.
    pub fn save(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        // atomic, so a crash mid-write leaves the old state rather than an unreadable one
        crate::fs_atomic::replace(path, content.as_bytes())?;
        Ok(())
    }

    /// Check if the STOP sentinel file exists.
    pub fn is_stopped(sentinel_path: &Path) -> bool {
        sentinel_path.exists()
    }

    /// Create or update the STOP sentinel file.
    pub fn create_stop_sentinel(sentinel_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = sentinel_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(sentinel_path, "STOP")?;
        Ok(())
    }

    /// Remove the STOP sentinel file.
    pub fn remove_stop_sentinel(sentinel_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if sentinel_path.exists() {
            std::fs::remove_file(sentinel_path)?;
        }
        Ok(())
    }

    /// Check if the loop should continue running.
    pub fn should_continue(&self, sentinel_path: &Path) -> bool {
        self.running && !Self::is_stopped(sentinel_path)
    }

    /// Check if we should pause for compaction.
    pub fn should_pause_for_compaction(&self, effective_percent: f64) -> bool {
        effective_percent >= self.pause_percent as f64
    }

    /// Check if we're in the grace period (first N turns of a run).
    pub fn is_grace_period(&self, iteration: u64) -> bool {
        iteration < self.context_grace_turns as u64
    }

    /// What can be said about the process behind this state's `running` flag.
    pub fn liveness(&self) -> LoopLiveness {
        self.owner.as_ref().map_or(LoopLiveness::Unrecorded, LoopOwner::liveness)
    }

    /// Whether another live `luxe loop start` already owns this project.
    ///
    /// Two loops in one repository share a phase counter, a journal and a `STOP` sentinel, so the
    /// second one to start silently corrupts the first one's run. Only a *live* owner that is not
    /// this process counts — a crashed run must never lock a project out of starting again.
    pub fn owned_by_another_process(&self) -> Option<u32> {
        let owner = self.owner.as_ref()?;
        match owner.liveness() {
            LoopLiveness::Alive { pid } if pid != std::process::id() => Some(pid),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod owner_tests {
    use super::*;

    fn owner(pid: u32, host: &str) -> LoopOwner {
        LoopOwner { pid, host: host.to_string(), started_at: 0 }
    }

    /// A run records the process driving it, so a later reader can ask whether it still is.
    #[test]
    fn the_current_process_is_alive_and_is_us() {
        let current = LoopOwner::current();
        assert_eq!(current.pid, std::process::id());
        assert_eq!(current.liveness(), LoopLiveness::Alive { pid: std::process::id() });

        let mut state = LoopState { running: true, owner: Some(current), ..Default::default() };
        assert_eq!(
            state.owned_by_another_process(),
            None,
            "a process does not lock itself out of its own run"
        );

        state.owner = Some(owner(u32::MAX - 1, &crate::program::hostname()));
        assert!(matches!(state.liveness(), LoopLiveness::Gone { .. }));
        assert_eq!(state.owned_by_another_process(), None, "and a dead one locks out nobody");
    }

    /// A record from another machine is not evidence about this one, either way.
    ///
    /// A project directory travels — a synced folder, a network share, a container mount — and a
    /// process id only means something on the box that issued it. Reading one as local is how a
    /// stranger's process gets reported as your loop.
    #[test]
    fn a_record_from_another_machine_says_so() {
        let state = LoopState {
            running: true,
            owner: Some(owner(std::process::id(), "some-other-box")),
            ..Default::default()
        };
        assert_eq!(
            state.liveness(),
            LoopLiveness::Elsewhere { host: "some-other-box".into() },
            "even though that pid is alive here"
        );
        assert_eq!(state.owned_by_another_process(), None);
    }

    /// A state file written before luxe recorded an owner still loads, and claims nothing.
    #[test]
    fn a_state_without_an_owner_makes_no_claim() {
        let older = r#"{"running":true,"iteration":3,"current_phase":1,"max_turns_per_run":50,
            "compact_percent":75,"pause_percent":90,"journal_max_kb":24,"confirmations_required":2,
            "context_grace_turns":2,"max_no_progress_turns":12,"max_stalls":5,
            "rate_limit_backoff_ms":30000,"compact_timeout_ms":600000,"max_consecutive_errors":5}"#;
        let state: LoopState =
            serde_json::from_str(older).expect("an older state file still loads");
        assert_eq!(state.iteration, 3);
        assert_eq!(state.liveness(), LoopLiveness::Unrecorded);
        assert_eq!(state.owned_by_another_process(), None);
    }
}
