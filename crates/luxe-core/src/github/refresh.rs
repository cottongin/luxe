//! Refresh — when to ask GitHub about the branch's pull request again.
//!
//! GitHub has no push channel a local program can subscribe to, so the status row is kept current
//! two ways: right away after anything Luxe saw happen, and on an interval otherwise. This module
//! is the interval, and only the arithmetic of it — the workspace runtime tells it what happened
//! and asks when to look next, so every rule here is testable without a clock or a network.
//!
//! # The rules
//!
//! - Every 15 seconds while something is in flight: checks pending, or a push or a new pull
//!   request in the last 10 minutes.
//! - Every `github.refresh_secs` (60 by default) once settled, and never slower than that while
//!   in flight: a setting of 10 means every 10 seconds, not every 15.
//! - Every 5 minutes once the terminal has been out of focus for 2, or when the pull request is
//!   closed or merged and so has nothing left to report.
//! - The settled rate while under a fifth of the hourly budget, which every tool on the same
//!   sign-in shares; after a refusal for rate, nothing until the budget refills.
//! - Nothing at all after an answer that retrying cannot change (no `gh`, signed out, not GitHub),
//!   or with `github.refresh_secs` at `0`. Refreshes that follow something the reader did still
//!   happen; they are the caller's, not this schedule's.

use crate::permission::wildcard::matches_command;
use std::time::{Duration, Instant};

/// How often to look while checks are running or a push is fresh.
pub const ACTIVE: Duration = Duration::from_secs(15);

/// The settled interval when `github.refresh_secs` is not set.
pub const DEFAULT_REFRESH_SECS: u64 = 60;

/// How often to look while nobody is watching, or when nothing is left to change.
pub const IDLE: Duration = Duration::from_secs(300);

/// How long out of focus before the terminal counts as unwatched.
pub const AWAY_AFTER: Duration = Duration::from_secs(120);

/// How long after a push or a new pull request the checks are presumed to be starting.
pub const FRESH_FOR: Duration = Duration::from_secs(600);

/// Points left below which the schedule stops hurrying: a fifth of GitHub's 5,000 an hour.
pub const LOW_BUDGET: u32 = 1_000;

/// What the last refresh found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// Not asked yet
    Unknown,
    /// No `gh`, signed out, or not a GitHub repository: asking again changes nothing
    Unavailable,
    /// The question did not get an answer this time: a timeout, a network error
    Failed,
    /// The branch has no pull request
    NoPullRequest,
    /// The branch's pull request is closed or merged
    Finished,
    /// The branch's pull request is open
    Open {
        /// Whether any check on its head is still queued or running
        checks_pending: bool,
    },
}

/// When the next background refresh is due, from what the runtime has told it.
#[derive(Debug, Clone)]
pub struct Schedule {
    /// The settled interval, or `None` with background refresh off
    settled: Option<Duration>,
    last_refresh: Option<Instant>,
    standing: Standing,
    remaining: Option<u32>,
    limited_until: Option<Instant>,
    last_activity: Option<Instant>,
    unfocused_since: Option<Instant>,
}

impl Schedule {
    /// A schedule settling at `refresh_secs`; `0` turns background refresh off.
    pub fn new(refresh_secs: u64) -> Self {
        Self {
            settled: (refresh_secs > 0).then(|| Duration::from_secs(refresh_secs)),
            last_refresh: None,
            standing: Standing::Unknown,
            remaining: None,
            limited_until: None,
            last_activity: None,
            unfocused_since: None,
        }
    }

    /// A refresh finished at `at`, finding `standing`, with `remaining` points left when known.
    pub fn refreshed(&mut self, at: Instant, standing: Standing, remaining: Option<u32>) {
        self.last_refresh = Some(at);
        self.standing = standing;
        if remaining.is_some() {
            self.remaining = remaining;
        }
    }

    /// GitHub refused for rate at `at`, or answered with nothing left; nothing may ask again
    /// before `until`.
    pub fn rate_limited(&mut self, at: Instant, until: Instant) {
        self.last_refresh = Some(at);
        self.limited_until = Some(until.max(at));
    }

    /// Something that starts checks happened at `at`: a push, or a new pull request.
    pub fn activity(&mut self, at: Instant) {
        self.last_activity = Some(at);
    }

    /// The terminal gained or lost focus at `at`.
    pub fn focus(&mut self, at: Instant, focused: bool) {
        match (focused, self.unfocused_since) {
            (true, _) => self.unfocused_since = None,
            (false, None) => self.unfocused_since = Some(at),
            (false, Some(_)) => {}
        }
    }

    /// Whether asking at `now` is allowed at all. Only a rate refusal forbids it, and it forbids
    /// event refreshes too, since a refused request still counts against the budget.
    pub fn may_ask(&self, now: Instant) -> bool {
        self.limited_until.is_none_or(|until| now >= until)
    }

    /// When the next background refresh is due, or `None` when there is none to schedule.
    pub fn next(&self, now: Instant) -> Option<Instant> {
        let settled = self.settled?;
        if let Some(until) = self.limited_until.filter(|until| *until > now) {
            return Some(until);
        }
        if self.standing == Standing::Unavailable {
            return None;
        }
        let Some(last) = self.last_refresh else {
            return Some(now);
        };
        Some(last + self.interval(settled, now))
    }

    fn interval(&self, settled: Duration, now: Instant) -> Duration {
        let idle = IDLE.max(settled);
        let away = self
            .unfocused_since
            .is_some_and(|since| now.saturating_duration_since(since) >= AWAY_AFTER);
        if away {
            return idle;
        }
        if self.remaining.is_some_and(|left| left < LOW_BUDGET) {
            return settled;
        }
        let fresh =
            self.last_activity.is_some_and(|at| now.saturating_duration_since(at) < FRESH_FOR);
        let pending = self.standing == Standing::Open { checks_pending: true };
        if fresh || pending {
            return ACTIVE.min(settled);
        }
        if self.standing == Standing::Finished {
            return idle;
        }
        settled
    }
}

/// What a tool call means for the branch's pull request, once it has finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Touch {
    /// Nothing GitHub would know about
    None,
    /// It read or changed the pull request, so what the row shows may be stale
    Changed,
    /// It started checks: a push, a new pull request, a re-run
    Started,
}

/// What `tool` called with `arguments` did to the branch's pull request.
///
/// `bash` commands are matched segment by segment with the permission engine's own splitter, so
/// `cargo fmt && git push` counts the same as `git push`.
pub fn touch(tool: &str, arguments: &serde_json::Value) -> Touch {
    match tool {
        "git" if arguments.get("action").and_then(|action| action.as_str()) == Some("pr") => {
            let first = arguments
                .get("args")
                .and_then(|args| args.as_array())
                .and_then(|args| args.first())
                .and_then(|arg| arg.as_str());
            if first == Some("create") {
                Touch::Started
            } else {
                Touch::Changed
            }
        }
        "bash" => {
            let Some(command) = arguments.get("command").and_then(|command| command.as_str())
            else {
                return Touch::None;
            };
            let any = |patterns: &[&str]| {
                patterns.iter().any(|pattern| matches_command(command, pattern))
            };
            if any(&["git push *", "gh pr create *", "gh run rerun *", "gh workflow run *"]) {
                Touch::Started
            } else if any(&["gh pr *", "gh issue *", "gh run *"]) {
                Touch::Changed
            } else {
                Touch::None
            }
        }
        _ => Touch::None,
    }
}

#[cfg(test)]
mod tests;
