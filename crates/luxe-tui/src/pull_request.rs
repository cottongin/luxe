//! Pull request — the branch's pull request put into words, the same way wherever it is shown.
//!
//! The statusline, the dock and `/pr` all read [`luxe_core::workspace::PullRequestPresence`] from
//! the workspace snapshot, and none of them asks GitHub anything: the workspace runtime keeps it
//! current. Every fact is a word — `2 failing`, not a red cross — so a terminal without the icon
//! font loses nothing but the icon.

use luxe_core::github::{PullRequestState, PullRequestStatus, ReviewDecision};
use luxe_core::workspace::PullRequestPresence;

/// The bar's one-liner: `#138 · 1 failing · 2 threads`, or `#149 · stacked 8 of 10 · draft`.
pub(crate) fn label(pr: &PullRequestStatus) -> String {
    let mut parts = vec![format!("#{}", pr.number)];
    if let Some(stack) = pr.stack.as_ref().filter(|_| pr.state == PullRequestState::Open) {
        parts.push(format!("stacked {} of {}", stack.position(), stack.height()));
    }
    match pr.state {
        PullRequestState::Merged => parts.push("merged".into()),
        PullRequestState::Closed => parts.push("closed".into()),
        PullRequestState::Open => {
            if pr.draft {
                parts.push("draft".into());
            }
            if pr.checks.failed > 0 {
                parts.push(format!("{} failing", pr.checks.failed));
            } else if pr.checks.pending > 0 {
                parts.push("checks running".into());
            } else if pr.checks.passed > 0 {
                parts.push("checks pass".into());
            }
            match pr.review_decision {
                Some(ReviewDecision::ChangesRequested) => parts.push("changes requested".into()),
                Some(ReviewDecision::Approved) => parts.push("approved".into()),
                Some(ReviewDecision::ReviewRequired) | None => {}
            }
            if pr.unresolved_threads > 0 {
                parts.push(threads(pr.unresolved_threads));
            }
        }
    }
    parts.join(" · ")
}

/// Whether something on the pull request is waiting on its author: a failing check, or a reviewer
/// asking for changes.
pub(crate) fn wants_attention(pr: &PullRequestStatus) -> bool {
    pr.state == PullRequestState::Open
        && (pr.checks.failed > 0 || pr.review_decision == Some(ReviewDecision::ChangesRequested))
}

/// What `/pr` says: the pull request in full, or the one sentence saying why there is none.
pub(crate) fn report(presence: &PullRequestPresence) -> String {
    match presence {
        PullRequestPresence::Unknown => {
            "luxe has not heard from GitHub about this branch yet; it is asking now.".into()
        }
        PullRequestPresence::None => "This branch has no pull request on GitHub.".into(),
        PullRequestPresence::Unavailable { reason } => reason.clone(),
        PullRequestPresence::Found(pr) => {
            let state = match (pr.state, pr.draft) {
                (PullRequestState::Merged, _) => "merged",
                (PullRequestState::Closed, _) => "closed",
                (PullRequestState::Open, true) => "open, draft",
                (PullRequestState::Open, false) => "open",
            };
            let mut lines = vec![format!("#{} {} — {state}", pr.number, pr.title)];
            if let Some(stack) = &pr.stack {
                lines.extend(stack_lines(pr.number, &pr.title, stack));
            }
            let checks = &pr.checks;
            let waiting = pr.stack.as_ref().filter(|stack| stack.checks_wait_for_trunk(checks));
            if let Some(stack) = waiting {
                let bottom = stack.below.first().map_or(0, |bottom| bottom.number);
                lines.push(format!(
                    "checks: none yet — this repository checks pull requests into `{trunk}`, as \
                     #{bottom}'s were, so this one's run once it targets `{trunk}`",
                    trunk = stack.trunk
                ));
            } else if checks.total() == 0 {
                lines.push("checks: none reported".into());
            } else {
                let counts = [
                    (checks.failed, "failing"),
                    (checks.pending, "running"),
                    (checks.passed, "passed"),
                    (checks.skipped, "skipped"),
                ];
                let counted: Vec<String> = counts
                    .iter()
                    .filter(|(count, _)| *count > 0)
                    .map(|(count, word)| format!("{count} {word}"))
                    .collect();
                lines.push(format!("checks: {}", counted.join(", ")));
            }
            let decision = match pr.review_decision {
                Some(ReviewDecision::Approved) => "approved",
                Some(ReviewDecision::ChangesRequested) => "changes requested",
                Some(ReviewDecision::ReviewRequired) => "review required",
                None => "no review required",
            };
            let review = match pr.unresolved_threads {
                0 => format!("review: {decision}"),
                count => format!("review: {decision} · {} unresolved", threads(count)),
            };
            lines.push(review);
            lines.push(pr.url.clone());
            lines.join("\n")
        }
    }
}

/// The stack, bottom first, with this pull request marked where it sits.
fn stack_lines(number: u64, title: &str, stack: &luxe_core::github::Stack) -> Vec<String> {
    let into = if stack.trunk_is_default {
        format!("into `{}`", stack.trunk)
    } else {
        format!("into `{}`, a branch with no pull request of its own", stack.trunk)
    };
    let mut lines = vec![format!("stack: {} of {}, {into}", stack.position(), stack.height())];
    let row = |entry: &luxe_core::github::StackEntry| {
        let draft = if entry.draft { " (draft)" } else { "" };
        format!("  #{} {}{draft}", entry.number, entry.title)
    };
    lines.extend(stack.below.iter().map(row));
    lines.push(format!("› #{number} {title} — this one"));
    lines.extend(stack.above.iter().map(row));
    lines
}

fn threads(count: usize) -> String {
    match count {
        1 => "1 thread".into(),
        count if count >= 100 => "100+ threads".into(),
        count => format!("{count} threads"),
    }
}

#[cfg(test)]
mod tests;
