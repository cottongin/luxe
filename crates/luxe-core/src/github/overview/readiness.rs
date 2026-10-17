//! Readiness — the rows between a pull request and merged, each with what can be done about it.

use super::{MergeStatus, PullRequestOverview, Verdict};
use crate::github::{PullRequestState, ReviewDecision};

/// How a merge check reads at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckMark {
    /// Nothing in the way
    Pass,
    /// In the way until someone acts
    Fail,
    /// In the way until something finishes, or someone else acts
    Wait,
    /// Worth knowing, not in the way
    Note,
}

/// What a merge check offers to do about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckAction {
    /// Hand the failing steps to the agent
    FixChecks,
    /// Work through the unresolved review threads in Guided Review: fix, reply, resolve
    ResolveThreads,
    /// Bring the head up to date with its base
    UpdateBranch,
    /// Resolve the conflicts in a worktree session
    ResolveConflicts,
    /// Take it out of draft
    MarkReady,
    /// Open the pull request below it in its stack
    OpenBelow(u64),
    /// Merge it
    Merge,
    /// Have GitHub merge it by itself once nothing stands in the way
    EnableAutoMerge,
    DisableAutoMerge,
}

/// One row of "Before it can merge".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeCheck {
    /// How it reads at a glance
    pub mark: CheckMark,
    /// What is being checked, a word or two
    pub label: &'static str,
    /// The evidence, as a phrase
    pub detail: String,
    /// What can be done about it from here
    pub action: Option<CheckAction>,
}

/// The rows between a pull request and merged, blockers first and the merge itself last.
///
/// Empty for a pull request that is already merged or closed: there is nothing left to stand in
/// its way. An action is offered only where it would work for this account — `f` on someone
/// else's pull request means nothing when a push from here would be refused.
pub fn merge_checks(pr: &PullRequestOverview) -> Vec<MergeCheck> {
    if pr.state != PullRequestState::Open {
        return Vec::new();
    }
    let mut checks = Vec::new();
    let check = |mark, label, detail: String, action| MergeCheck { mark, label, detail, action };

    if pr.draft {
        let action = (pr.viewer.author || pr.viewer.can_push).then_some(CheckAction::MarkReady);
        checks.push(check(CheckMark::Fail, "Draft", "still a draft".into(), action));
    }
    if let Some(below) = pr.stack.as_ref().and_then(|stack| stack.below.last()) {
        checks.push(check(
            CheckMark::Wait,
            "Stack",
            format!("waits on #{} below it", below.number),
            Some(CheckAction::OpenBelow(below.number)),
        ));
    }
    checks.push(checks_row(pr));
    if let Some(review) = review_row(pr) {
        checks.push(review);
    }
    if let Some(waiting) = waiting_row(pr) {
        checks.push(waiting);
    }
    match pr.merge_status {
        MergeStatus::Dirty => checks.push(check(
            CheckMark::Fail,
            "Conflicts",
            format!("conflicts with {}", pr.base_ref),
            pr.viewer.can_push.then_some(CheckAction::ResolveConflicts),
        )),
        MergeStatus::Behind => checks.push(check(
            CheckMark::Fail,
            "Up to date",
            format!("behind {}", pr.base_ref),
            pr.viewer.can_push.then_some(CheckAction::UpdateBranch),
        )),
        MergeStatus::Unknown => {
            checks.push(check(CheckMark::Wait, "Conflicts", "checking".into(), None))
        }
        _ => checks.push(check(
            CheckMark::Pass,
            "Conflicts",
            format!("none with {}", pr.base_ref),
            None,
        )),
    }
    let clear = checks.iter().all(|row| matches!(row.mark, CheckMark::Pass | CheckMark::Note));
    // Said whenever it is on, since it merges by itself the moment the rows above clear, and
    // offered whenever GitHub says this account may: it starts no run of its own, only waits
    let settings = &pr.merge_settings;
    if let Some(method) = settings.auto_merge {
        checks.push(check(
            CheckMark::Note,
            "Auto-merge",
            format!("{} into {} when ready", method.phrase(), pr.base_ref),
            settings.can_cancel_auto_merge.then_some(CheckAction::DisableAutoMerge),
        ));
    } else if settings.can_auto_merge {
        checks.push(check(
            CheckMark::Note,
            "Auto-merge",
            "off".into(),
            Some(CheckAction::EnableAutoMerge),
        ));
    }
    if clear && pr.merge_status.mergeable() {
        // Say how it will merge before anyone asks it to: merging is not easy to take back. The
        // method is the one the confirmation opens on
        let method = pr.default_merge_method();
        checks.push(check(
            CheckMark::Pass,
            "Ready",
            format!("{} into {}", method.phrase(), pr.base_ref),
            pr.viewer.can_merge.then_some(CheckAction::Merge),
        ));
    }
    checks
}

/// How many rows are in the way.
pub fn blocking(checks: &[MergeCheck]) -> usize {
    checks.iter().filter(|row| matches!(row.mark, CheckMark::Fail | CheckMark::Wait)).count()
}

fn checks_row(pr: &PullRequestOverview) -> MergeCheck {
    let rollup = &pr.checks;
    let total = rollup.total();
    let (mark, detail, action) = if rollup.failed > 0 {
        let mut names: Vec<&str> = pr.failing.iter().take(2).map(|f| f.name.as_str()).collect();
        let more = rollup.failed.saturating_sub(names.len());
        let mut said = format!("{} of {total} failing", rollup.failed);
        if !names.is_empty() {
            if more > 0 {
                names.push("…");
            }
            said.push_str(&format!(": {}", names.join(", ")));
        }
        let action = pr.viewer.can_push.then_some(CheckAction::FixChecks);
        (CheckMark::Fail, said, action)
    } else if rollup.pending > 0 {
        (CheckMark::Wait, format!("{} of {total} running", rollup.pending), None)
    } else if total == 0 {
        let waiting = pr.stack.as_ref().filter(|stack| stack.checks_wait_for_trunk(rollup));
        match waiting {
            Some(stack) => {
                (CheckMark::Wait, format!("none until it targets {}", stack.trunk), None)
            }
            None => (CheckMark::Note, "none reported".into(), None),
        }
    } else {
        (CheckMark::Pass, format!("{} passed", rollup.passed), None)
    };
    MergeCheck { mark, label: "Checks", detail, action }
}

fn review_row(pr: &PullRequestOverview) -> Option<MergeCheck> {
    let resolve = pr.viewer.author.then_some(CheckAction::ResolveThreads);
    let threads = match pr.unresolved_threads {
        0 => None,
        1 => Some("1 thread open".to_string()),
        count => Some(format!("{count} threads open")),
    };
    let named = |verdict| -> Vec<&str> {
        pr.reviewers
            .iter()
            .filter(|reviewer| reviewer.verdict == verdict)
            .map(|reviewer| reviewer.login.as_str())
            .collect()
    };
    let row = |mark, detail: String, action| MergeCheck { mark, label: "Review", detail, action };
    let with_threads = |said: String| match &threads {
        Some(threads) => format!("{said} · {threads}"),
        None => said,
    };
    match pr.review_decision {
        Some(ReviewDecision::ChangesRequested) => {
            let who = named(Verdict::ChangesRequested);
            let said = match who.as_slice() {
                [] => "changes requested".to_string(),
                [one] => format!("changes requested by {one}"),
                [first, rest @ ..] => {
                    format!("changes requested by {first} and {} more", rest.len())
                }
            };
            Some(row(CheckMark::Fail, with_threads(said), resolve))
        }
        Some(ReviewDecision::Approved) => {
            let who = named(Verdict::Approved);
            let said = match who.as_slice() {
                [] => "approved".to_string(),
                [one] => format!("approved by {one}"),
                [first, rest @ ..] => format!("approved by {first} and {} more", rest.len()),
            };
            match threads {
                Some(_) => Some(row(CheckMark::Wait, with_threads(said), resolve)),
                None => Some(row(CheckMark::Pass, said, None)),
            }
        }
        Some(ReviewDecision::ReviewRequired) => {
            Some(row(CheckMark::Wait, with_threads("approval required".into()), None))
        }
        None => threads.map(|threads| row(CheckMark::Wait, threads, resolve)),
    }
}

fn waiting_row(pr: &PullRequestOverview) -> Option<MergeCheck> {
    let waiting: Vec<&str> = pr
        .reviewers
        .iter()
        .filter(|reviewer| reviewer.verdict == Verdict::Requested)
        .map(|reviewer| reviewer.login.as_str())
        .collect();
    let detail = match waiting.as_slice() {
        [] => return None,
        [one] => format!("awaiting {one}"),
        [first, second] => format!("awaiting {first} and {second}"),
        [first, rest @ ..] => format!("awaiting {first} and {} more", rest.len()),
    };
    Some(MergeCheck { mark: CheckMark::Wait, label: "Reviewers", detail, action: None })
}

#[cfg(test)]
mod tests;
