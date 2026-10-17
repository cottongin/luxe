//! Runaway guards hook — max turns/no-progress/stalls + rate-limit backoff.
//!
//! Prevents the loop from running away by enforcing:
//! - Max turns per run (soft backstop)
//! - Max no-progress turns (circling detection)
//! - Max stalls before pausing for a human
//! - Rate-limit backoff (429/TPM) distinct from hard errors

use crate::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint};
use crate::loop_controller::LoopController;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info};

struct HookState {
    turns_since_last_progress: u32,
    stalls_since_last_success: u32,
    consecutive_errors: u32,
    /// Whether the turn now ending failed.
    ///
    /// `Error` fires before `AgentEnd`, so this is what lets "consecutive" mean consecutive: a turn
    /// that ends without having failed clears the count. Without it, five failures spread across a
    /// long run would stop the loop as though they had happened back to back.
    turn_failed: bool,
    last_progress_event: Option<String>,
}

/// How long to wait before the loop's next iteration.
///
/// A trait so self-continuation can be paced without knowing what is counting the failures, and so
/// a test can pace it without waiting.
#[async_trait]
pub trait IterationPacing: Send + Sync + std::fmt::Debug {
    /// The pause to take before enqueuing the next iteration.
    async fn pause_before_next_iteration(&self) -> std::time::Duration;
}

/// A hook that prevents runaway loops with guards against max turns, no-progress,
/// stalls, and rate-limit backoff.
pub struct RunawayGuardsHook {
    /// Loop controller for checking state.
    loop_controller: Arc<LoopController>,
    /// Mutable hook state.
    state: Arc<Mutex<HookState>>,
}

impl std::fmt::Debug for RunawayGuardsHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunawayGuardsHook").finish()
    }
}

impl RunawayGuardsHook {
    /// Create a new runaway guards hook.
    pub fn new(loop_controller: Arc<LoopController>) -> Self {
        Self {
            loop_controller,
            state: Arc::new(Mutex::new(HookState {
                turns_since_last_progress: 0,
                stalls_since_last_success: 0,
                consecutive_errors: 0,
                turn_failed: false,
                last_progress_event: None,
            })),
        }
    }

    /// Categorize a tool name as "building" (progress) vs "reading" (no progress).
    ///
    /// Building tools modify the workspace (edit, write, bash, git). Reading tools
    /// only observe (read, grep, glob, web_search, web_fetch, etc.). Neutrals like
    /// question/skill/task/todo/astgrep/loop_control don't count as progress either —
    /// they're part of the thinking loop, not active building.
    fn is_building_tool(tool_name: &str) -> bool {
        matches!(tool_name, "edit" | "write" | "bash" | "git")
    }

    /// Check if we should stop due to max turns.
    pub async fn check_max_turns(&self) -> Option<String> {
        let state = self.loop_controller.get_state().await;
        if state.iteration >= state.max_turns_per_run as u64 {
            Some("Max turns reached. Ending loop.".to_string())
        } else {
            None
        }
    }

    /// Check if we should stop due to no-progress (circling).
    pub async fn check_no_progress(&self) -> Option<String> {
        let state = self.state.lock().await;
        let config = self.loop_controller.get_state().await;
        if state.turns_since_last_progress >= config.max_no_progress_turns {
            Some("No progress detected (circling). Ending loop.".to_string())
        } else {
            None
        }
    }

    /// Check if we should stop due to max stalls.
    pub async fn check_max_stalls(&self) -> Option<String> {
        let state = self.state.lock().await;
        let config = self.loop_controller.get_state().await;
        if state.stalls_since_last_success >= config.max_stalls {
            Some("Max stalls reached. Pausing for human.".to_string())
        } else {
            None
        }
    }

    /// Check if we should stop due to max consecutive errors.
    pub async fn check_max_errors(&self) -> Option<String> {
        let state = self.state.lock().await;
        let config = self.loop_controller.get_state().await;
        if state.consecutive_errors >= config.max_consecutive_errors {
            Some("Max consecutive errors reached. Ending loop.".to_string())
        } else {
            None
        }
    }

    /// Record a progress event.
    pub async fn record_progress(&self, event: &str) {
        let mut state = self.state.lock().await;
        state.turns_since_last_progress = 0;
        state.stalls_since_last_success = 0;
        state.last_progress_event = Some(event.to_string());
    }

    /// Record a stall (no progress).
    pub async fn record_stall(&self) {
        let mut state = self.state.lock().await;
        state.turns_since_last_progress += 1;
        state.stalls_since_last_success += 1;
    }

    /// Record an error, and remember that the turn now ending is a failed one.
    pub async fn record_error(&self) {
        let mut state = self.state.lock().await;
        state.consecutive_errors += 1;
        state.turn_failed = true;
    }

    /// Reset error counter on success.
    pub async fn reset_errors(&self) {
        let mut state = self.state.lock().await;
        state.consecutive_errors = 0;
    }

    /// Get the rate-limit backoff duration (exponential backoff based on consecutive errors).
    pub async fn get_backoff_ms(&self) -> u64 {
        let state = self.state.lock().await;
        // Exponential backoff: 1s, 2s, 4s, 8s, max 30s
        let base = 1000u64;
        let max = 30000u64;
        let backoff = base * 2u64.pow(state.consecutive_errors.min(5));
        backoff.min(max)
    }

    /// Run all guards and return the first violation, if any.
    pub async fn check_all_guards(&self) -> Option<String> {
        if let Some(msg) = self.check_max_turns().await {
            return Some(msg);
        }
        if let Some(msg) = self.check_no_progress().await {
            return Some(msg);
        }
        if let Some(msg) = self.check_max_stalls().await {
            return Some(msg);
        }
        if let Some(msg) = self.check_max_errors().await {
            return Some(msg);
        }
        None
    }
}

#[async_trait]
impl IterationPacing for RunawayGuardsHook {
    /// Zero while things are working. After a failure it is [`Self::get_backoff_ms`]'s exponential
    /// pause, which is the difference between backing off a rate-limited provider and hammering it
    /// at full speed until an unrelated guard cuts the run off.
    async fn pause_before_next_iteration(&self) -> std::time::Duration {
        if self.state.lock().await.consecutive_errors == 0 {
            return std::time::Duration::ZERO;
        }
        std::time::Duration::from_millis(self.get_backoff_ms().await)
    }
}

#[async_trait]
impl Hook for RunawayGuardsHook {
    fn name(&self) -> &'static str {
        "runaway-guards"
    }

    fn interest(&self) -> &[HookPoint] {
        &[HookPoint::AgentEnd, HookPoint::AfterToolUse, HookPoint::Error]
    }

    async fn on(&self, point: HookPoint, ev: &HookEvent, _cx: &HookCtx) -> HookOutcome {
        match point {
            // A failed turn is an error the loop's budget has to see. Without this, a provider
            // outage never touched `consecutive_errors`, so `max_consecutive_errors` could not fire
            // and the exponential backoff had nothing to back off from.
            HookPoint::Error => {
                self.record_error().await;
                debug!("turn error recorded");
                HookOutcome::Continue
            }
            HookPoint::AfterToolUse => {
                // A failing tool call is the agent's ordinary feedback loop, not the loop breaking:
                // an `edit` whose `oldText` did not match, a `bash` that exited non-zero, a `read`
                // of a file that is not there. Counting them against the error budget ended a
                // productive run after five *scattered* tool failures, which is what
                // `max_consecutive_errors` is emphatically not for. Turn failures are what it
                // counts, and those arrive at `HookPoint::Error`. An unproductive turn is already
                // the no-progress guard's business.
                if let HookEvent::ToolResult { is_error, tool_name, .. } = ev {
                    if *is_error {
                        debug!(tool = %tool_name, "tool call failed (not counted against the error budget)");
                    } else if Self::is_building_tool(tool_name) {
                        self.record_progress(&format!("tool: {}", tool_name)).await;
                        debug!(tool = %tool_name, "building tool progress recorded");
                    } else {
                        debug!(tool = %tool_name, "read-only/neutral tool (no progress)");
                    }
                }
                HookOutcome::Continue
            }
            HookPoint::AgentEnd => {
                debug!("checking all guards");

                // A turn that ended without failing clears the error run. `Error` fires first, so
                // the flag is already set when it is the failed one.
                {
                    let mut state = self.state.lock().await;
                    if state.turn_failed {
                        state.turn_failed = false;
                    } else {
                        state.consecutive_errors = 0;
                    }
                }

                if let Some(msg) = self.check_all_guards().await {
                    // A guard firing is the loop doing what it was configured to do, and whatever
                    // was actually wrong has already logged itself: a failed turn at `error`, a
                    // stall at `warn`. Repeating it here as a warning would double-report the cause
                    // and put a warning on the ordinary case of a run reaching its turn budget.
                    info!(reason = %msg, "ending the run on a guard");
                    let _ = self.loop_controller.stop().await;
                    return HookOutcome::StopLoop { reason: msg };
                }

                // Increment counters.
                {
                    let mut state = self.state.lock().await;
                    state.turns_since_last_progress += 1;
                    state.stalls_since_last_success += 1;
                }

                HookOutcome::Continue
            }
            _ => HookOutcome::Continue,
        }
    }
}

#[cfg(test)]
mod tests;
