//! Built-in hook that runs acceptance tests for review gates.
//!
//! This hook listens for `ReviewGate` events and runs acceptance commands (e.g.,
//! `cargo test`, `cargo clippy`) to verify the recent work. It returns a verdict
//! (PASS/FAIL/INVALID) based on the command output.

use crate::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint};
use crate::review::{ReviewItem, ReviewItemStatus, ReviewVerdict};
use async_trait::async_trait;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command as TokioCommand;
use tracing::{debug, warn};

/// Env var set on spawned acceptance-command children so a nested review gate can
/// detect it is already running and refuse to recurse. Without this guard, a test
/// that fires the review gate with default `cargo test`/`cargo clippy` commands would
/// re-spawn the entire suite — an unbounded fork bomb that locks the machine.
pub const RECURSION_GUARD_ENV: &str = "LUXE_REVIEW_GATE_ACTIVE";

/// A hook that runs acceptance tests for review gates.
pub struct ReviewGateHook {
    /// Commands to run for the review gate (e.g., vec!["cargo", "test"]).
    commands: Vec<Vec<String>>,
    /// Timeout per acceptance command (default: 10 minutes).
    timeout: Duration,
}

impl ReviewGateHook {
    /// A gate with no acceptance commands, which passes without spawning anything.
    ///
    /// Not `["true"]`: that is a real subprocess on every claim to arrive at "yes", and `true` is a
    /// unix *program* — on Windows the spawn fails, and a gate that cannot spawn its command
    /// reports FAIL.
    pub fn new() -> Self {
        Self { commands: Vec::new(), timeout: Duration::from_secs(600) }
    }

    /// Create a new `ReviewGateHook` with custom commands.
    pub fn with_commands(commands: Vec<Vec<String>>) -> Self {
        Self { commands, timeout: Duration::from_secs(600) }
    }

    /// Set the timeout per acceptance command.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl Default for ReviewGateHook {
    fn default() -> Self {
        Self::new()
    }
}

impl ReviewGateHook {
    /// Runs one acceptance command to its verdict: pass, fail with its stderr, or fail because it
    /// did not spawn, did not run, or ran out of time.
    async fn run_command(&self, cmd: &[String]) -> ReviewItem {
        let program = &cmd[0];
        let args = &cmd[1..];
        let label = cmd.join(" ");
        debug!(program = %program, ?args, "running acceptance command");

        let mut command = TokioCommand::new(program);
        command
            .args(args)
            .env(RECURSION_GUARD_ENV, "1")
            .kill_on_drop(true)
            // Acceptance commands run under the TUI; none of their I/O may touch it.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Own process group so a timeout can kill the whole tree, not just cargo.
        crate::program::detach_from_terminal(&mut command);

        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                warn!(program = %program, error = %error, "could not spawn an acceptance command");
                let evidence = format!("Command failed to spawn: {error}");
                return failure(label, evidence, "Check command path and arguments".into());
            }
        };
        let child_pid = child.id();

        match tokio::time::timeout(self.timeout, child.wait_with_output()).await {
            Ok(Ok(output)) if output.status.success() => {
                ReviewItem { label, status: ReviewItemStatus::Pass, evidence: None, fix: None }
            }
            Ok(Ok(output)) => {
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let fix = format!("Fix issues in: {label}");
                failure(label, stderr, fix)
            }
            Ok(Err(error)) => {
                warn!(program = %program, error = %error, "an acceptance command did not run");
                let evidence = format!("Command failed to execute: {error}");
                failure(label, evidence, "Check command path and arguments".into())
            }
            Err(_) => {
                let secs = self.timeout.as_secs();
                warn!(program = %program, timeout_secs = secs, "acceptance command timed out");
                // kill_on_drop reaps the direct child; this reaps its descendants.
                if let Some(pid) = child_pid {
                    crate::program::kill_tree(pid);
                }
                let fix = format!("Increase timeout or optimize command: {label}");
                failure(label, format!("Command timed out after {secs}s"), fix)
            }
        }
    }
}

/// A failed acceptance item with what happened and what to try.
fn failure(label: String, evidence: String, fix: String) -> ReviewItem {
    ReviewItem { label, status: ReviewItemStatus::Fail, evidence: Some(evidence), fix: Some(fix) }
}

#[async_trait]
impl Hook for ReviewGateHook {
    fn name(&self) -> &'static str {
        "review-gate"
    }

    fn interest(&self) -> &[HookPoint] {
        &[HookPoint::ReviewGate]
    }

    async fn on(&self, _point: HookPoint, ev: &HookEvent, _cx: &HookCtx) -> HookOutcome {
        let HookEvent::Other(_) = ev else {
            return HookOutcome::Continue;
        };

        // Recursion guard: if we're already inside a review gate (env set by a parent
        // acceptance command), skip — otherwise a `cargo test` gate re-runs the suite.
        if std::env::var_os(RECURSION_GUARD_ENV).is_some() {
            warn!("already running inside a review gate; skipping acceptance commands to avoid recursion");
            return HookOutcome::Continue;
        }

        debug!("running acceptance commands");

        let mut items = Vec::new();

        for cmd in &self.commands {
            if !cmd.is_empty() {
                items.push(self.run_command(cmd).await);
            }
        }

        let verdict = ReviewVerdict::new(items);

        let verdict_str = format!(
            "{}\n\nItems:\n{}",
            verdict.overall,
            verdict
                .items
                .iter()
                .map(|item| format!(
                    "  - [{}] {}: {}",
                    item.status,
                    item.label,
                    item.evidence.as_deref().unwrap_or("no evidence")
                ))
                .collect::<Vec<_>>()
                .join("\n")
        );

        HookOutcome::InjectContext(verdict_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook::{HookCtx, HookEvent, HookOutcome, HookPoint, HookRegistry};
    use tokio::sync::mpsc;

    fn test_ctx() -> HookCtx {
        let (tx, _rx) = mpsc::channel(1);
        HookCtx { session_id: "test".into(), event_tx: tx, hook_registry: HookRegistry::new() }
    }

    /// The recursion guard must prevent acceptance commands from running when we're
    /// already inside a review gate — this is what stops the `cargo test` fork bomb.
    #[tokio::test]
    async fn test_recursion_guard_skips_commands() {
        let dir = std::env::temp_dir().join("luxe-review-gate-guard-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sentinel = dir.join("ran");

        // A command that would create a sentinel file if it ever executed.
        let hook = ReviewGateHook::with_commands(vec![vec![
            "touch".into(),
            sentinel.to_string_lossy().into_owned(),
        ]]);

        // Simulate being inside a parent review gate.
        std::env::set_var(RECURSION_GUARD_ENV, "1");
        let outcome = hook
            .on(HookPoint::ReviewGate, &HookEvent::Other(serde_json::Value::Null), &test_ctx())
            .await;
        std::env::remove_var(RECURSION_GUARD_ENV);

        assert!(matches!(outcome, HookOutcome::Continue), "guard should skip and Continue");
        assert!(!sentinel.exists(), "acceptance command must not run under the guard");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_gate_with_no_commands_spawns_nothing() {
        // The break this catches: a default of `["true"]`, which spawned a real process on every
        // claim to answer "yes" — and, since `true` is a unix program, refused every claim on
        // Windows because a command that cannot be spawned is reported as a failure.
        assert!(ReviewGateHook::new().commands.is_empty());
    }

    #[test]
    fn test_review_gate_hook_custom_commands() {
        let commands = vec![vec!["echo".into(), "test".into()]];
        let hook = ReviewGateHook::with_commands(commands);
        assert_eq!(hook.commands.len(), 1);
    }
}
