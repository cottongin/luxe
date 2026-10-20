//! Review summaries — the short strings about the source, its checks, progress, staleness, the
//! agent's work and the feedback tally.

use super::plural;
use crate::app::App;
use crate::guided_review::GuidedReviewLoadState;

/// The `Checks …` label for a review carrying real gate results, or `None`.
///
/// Only genuine review-gate results reach here. Promoting every shell command to a check and every
/// non-zero exit to a failure would turn a `grep` that matched nothing red and report a healthy
/// session as broken. The harness cannot know what an exit code meant, so it does not guess: an
/// interactive review has no gate results and says nothing, and what verification actually happened
/// is described in the Brief by the narrator, which can see the whole turn.
pub(super) fn checks_label(
    review: &luxe_core::guided_review::GuidedReview,
) -> Option<(ChecksTone, String)> {
    use luxe_core::guided_review::ReviewCheckStatus;
    let checks = &review.checks.checks;
    if checks.is_empty() {
        return None;
    }
    Some(if checks.iter().any(|check| matches!(check.status, ReviewCheckStatus::Failed)) {
        (ChecksTone::Failed, "Checks failed".to_string())
    } else if checks.iter().any(|check| matches!(check.status, ReviewCheckStatus::Pending)) {
        (ChecksTone::Unsettled, "Checks pending".to_string())
    } else if checks.iter().any(|check| matches!(check.status, ReviewCheckStatus::Unavailable)) {
        // A run that predates the final edit is the only thing that lands here today.
        (ChecksTone::Unsettled, "Verified before the last edit".to_string())
    } else {
        (ChecksTone::Passed, format!("Checks {}", crate::glyphs::Mark::Yes))
    })
}

/// How the checks line should read, decided with the status rather than re-derived from the words.
///
/// Matching the rendered label to pick a color would turn the passing line amber the moment its
/// tick became a themed glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChecksTone {
    Passed,
    Failed,
    Unsettled,
}

/// The `{n} files · +a −d · {n} hunks` change-size summary for the review.
///
/// The hunk count joined it when the overview's own "Scale" section went: that section restated
/// this line and added the one figure it was missing.
pub(super) fn review_stats_summary(review: &luxe_core::guided_review::GuidedReview) -> String {
    let mut stats = format!(
        "{} {}",
        review.stats.files,
        if review.stats.files == 1 { "file" } else { "files" }
    );
    if let (Some(additions), Some(deletions)) = (review.stats.additions, review.stats.deletions) {
        stats.push_str(&format!(" · +{additions} −{deletions}"));
    }
    if review.stats.hunks > 0 {
        stats.push_str(&format!(
            " · {} {}",
            review.stats.hunks,
            if review.stats.hunks == 1 { "hunk" } else { "hunks" }
        ));
    }
    stats
}

/// The in-flight status for the sidebar, or `None` when the review is settled.
///
/// Owned as a `String` so an active remediation can name the comments it is addressing: after
/// submitting, a silent review gives no sign the agent is working at all.
pub(super) fn review_activity_status(app: &App) -> Option<String> {
    if let Some(working) = agent_working_label(app) {
        return Some(working);
    }
    if app.review_edit_pending(|purpose| *purpose == crate::app::ReviewEditPurpose::Send) {
        Some("Saving feedback snapshot".into())
    } else if app.review_edit_pending(|_| true) {
        Some("Saving review".into())
    } else {
        // Kept short enough to fit the 40-cell rail on one line (no mid-phrase wrap).
        match app.guided_review_workspace.load_state {
            GuidedReviewLoadState::Loading { .. } => Some("Refreshing · revision retained".into()),
            GuidedReviewLoadState::Failed { .. } => {
                Some("Refresh failed · revision retained".into())
            }
            GuidedReviewLoadState::Idle | GuidedReviewLoadState::Ready => None,
        }
    }
}

/// Whether the working tree has moved on from the revision the reviewer is reading.
///
/// A pinned review is a feature, not a bug — but reading a diff that no longer matches the files
/// without being told is how a reviewer signs off on code that has since changed underneath them.
pub(super) fn stale_revision_label(
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
) -> Option<String> {
    // Checked first, and for any target, because the path comparison below cannot see it: a fix
    // nearly always edits a file the review already holds, which leaves the set of paths as it was
    if let Some(addressed) = app
        .review
        .session()
        .and_then(|session| fixes_returned_label(session, &review.source.content_digest))
    {
        return Some(addressed);
    }
    if review.target != luxe_core::guided_review::ReviewTarget::WorkingTree {
        return None;
    }
    let changes = &app.workspace.as_ref()?.changes;
    // A bounded list cannot be compared honestly, and neither can raw counts: Git collapses an
    // untracked directory into one dock entry while the review expands it into its files, so
    // `19 != 8` was permanently true and the warning permanently on. Paths compare; counts do not.
    if changes.truncated {
        return None;
    }
    let reviewed: Vec<String> = review
        .files
        .iter()
        .filter_map(|file| file.new_path.as_deref().or(file.old_path.as_deref()))
        .map(|path| path.display().to_string())
        .collect();
    let entries: Vec<String> =
        changes.files.iter().map(|entry| entry.path.display().to_string()).collect();
    let covers = |entry: &str, path: &str| {
        entry == path || (entry.ends_with('/') && path.starts_with(entry))
    };
    let moved = !reviewed.iter().all(|path| entries.iter().any(|entry| covers(entry, path)))
        || !entries.iter().all(|entry| reviewed.iter().any(|path| covers(entry, path)));
    moved.then_some("Workspace changed · ctrl+r refresh".to_owned())
}

/// `Fixes for 2 comments are in · Ctrl+R refresh` once fixes asked for from this revision have
/// come back: the diff on screen is from before them, and verifying a fix against it would sign
/// off on code that is no longer there. A refresh captures a new revision, which no submission
/// names yet.
pub(super) fn fixes_returned_label(
    session: &luxe_core::guided_review::ReviewSession,
    revision: &luxe_core::guided_review::ReviewRevisionId,
) -> Option<String> {
    use luxe_core::guided_review::{ReviewFeedbackMode, SubmissionDispatch};
    let fixed: usize = session
        .submissions
        .iter()
        .filter(|submission| {
            &submission.revision_id == revision
                && submission.mode == ReviewFeedbackMode::RequestFixes
                && submission.dispatch == SubmissionDispatch::Completed
        })
        .map(|submission| submission.findings.len())
        .sum();
    match fixed {
        0 => None,
        1 => Some("The fix is in · ctrl+r refresh".to_owned()),
        n => Some(format!("Fixes for {n} comments are in · ctrl+r refresh")),
    }
}

/// `Agent working on 2 comments`, or `Agent answering your question`, while a dispatched
/// submission is still being worked.
fn agent_working_label(app: &App) -> Option<String> {
    use luxe_core::guided_review::{ReviewFeedbackMode, SubmissionDispatch};
    let session = app.review.session()?;
    let dispatched: Vec<_> = session
        .submissions
        .iter()
        .filter(|submission| submission.dispatch == SubmissionDispatch::Dispatched)
        .collect();
    let comments: usize = dispatched.iter().map(|submission| submission.findings.len()).sum();
    let asking =
        dispatched.iter().all(|submission| submission.mode == ReviewFeedbackMode::Question);
    match (comments, asking) {
        (0, _) => None,
        (1, true) => Some("Agent answering your question".to_owned()),
        (n, true) => Some(format!("Agent answering {n} questions")),
        (1, false) => Some("Agent working on your comment".to_owned()),
        (n, false) => Some(format!("Agent working on {n} comments")),
    }
}

/// What the review asks of the reviewer: `14 to judge` / `all accounted for`.
///
/// Deliberately not a completion ratio. `Reviewed 0/92` framed review as an inspection with a target
/// nobody can hit on an agent-scale change, and made reading the core sections closely and judging
/// the rest look like failure.
pub(super) fn review_summary(app: &App) -> String {
    let budget = &app.review.budget;
    if budget.total() == 0 {
        return "Nothing to review".into();
    }
    if budget.needs_review == 0 {
        return "All accounted for".into();
    }
    // The accounted-for tail rides here rather than in a block of its own: what each exclusion is
    // *for* is printed on the hunk header and the file row it accounts for, so the only thing left
    // to say at review scale is how much of the change it covers.
    let mut summary = format!("{} to review", budget.needs_review);
    if budget.accounted > 0 {
        summary.push_str(&format!(" · {} accounted for", budget.accounted));
    }
    summary
}

/// `Chapters 2/7` — the only progress the review keeps.
pub(super) fn chapter_progress_summary(
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
) -> Option<String> {
    let (walked, total) = app.guided_review_workspace.chapters_walked(review);
    (total > 0).then(|| format!("Chapters {walked}/{total}"))
}

/// Whether `finding` went in a send the agent has not finished: queued, or its turn running.
///
/// Sent is not the same thing. Once its turn is over a comment is back with the reader, to verify
/// or to read, whatever its state says.
pub(in crate::view::guided_review) fn with_the_agent(
    session: &luxe_core::guided_review::ReviewSession,
    finding: &luxe_core::guided_review::FindingId,
) -> bool {
    use luxe_core::guided_review::SubmissionDispatch;
    session.submissions.iter().any(|submission| {
        matches!(submission.dispatch, SubmissionDispatch::Queued | SubmissionDispatch::Dispatched)
            && submission.findings.iter().any(|sent| &sent.finding_id == finding)
    })
}

pub(in crate::view::guided_review) fn submission_dispatch_label(
    dispatch: luxe_core::guided_review::SubmissionDispatch,
) -> &'static str {
    match dispatch {
        luxe_core::guided_review::SubmissionDispatch::Queued => "queued",
        luxe_core::guided_review::SubmissionDispatch::Dispatched => "dispatched",
        luxe_core::guided_review::SubmissionDispatch::FailedRetryable => "retry available",
        luxe_core::guided_review::SubmissionDispatch::Completed => "completed",
    }
}

pub(super) fn review_outcome_label(
    session: Option<&luxe_core::guided_review::ReviewSession>,
) -> Option<&'static str> {
    match session.and_then(|session| session.outcome) {
        Some(luxe_core::guided_review::ReviewOutcome::Accepted) => Some("accepted"),
        Some(luxe_core::guided_review::ReviewOutcome::ChangesRequested) => {
            Some("changes requested")
        }
        Some(luxe_core::guided_review::ReviewOutcome::Commented) => Some("commented"),
        None => None,
    }
}

/// The `Feedback N · M unsent` roll-up shown beside the review, or an empty state.
///
/// *Feedback*, not *findings*: a question and a comment are the same artifact here — text anchored
/// to a place, with a severity saying what it asks for — and "finding" is inspection language on a
/// surface that spent a whole pass leaving the inspection frame behind. The unit is a comment, the
/// collective is feedback. The durable type stays `ReviewFinding` and the binding stays
/// `tui.review.findings`, because both are written down where a rename breaks something: a session
/// file on disk, and a keymap someone has already edited.
pub(super) fn feedback_summary(
    session: Option<&luxe_core::guided_review::ReviewSession>,
    pull_request: bool,
) -> String {
    feedback_tally(session, pull_request)
        .map_or_else(|| "No feedback yet".into(), |tally| format!("Feedback {tally}"))
}

/// `3 · 2 unposted`: how many comments, and the one thing the most urgent of them is waiting on.
/// `None` with nothing written.
pub(super) fn feedback_tally(
    session: Option<&luxe_core::guided_review::ReviewSession>,
    pull_request: bool,
) -> Option<String> {
    use luxe_core::guided_review::FindingState;
    let session = session.filter(|session| !session.findings.is_empty())?;
    let total = session.findings.len();
    let count = |predicate: fn(&&luxe_core::guided_review::ReviewFinding) -> bool| {
        session.findings.iter().filter(predicate).count()
    };
    let unsent = session
        .findings
        .iter()
        .filter(|finding| finding.state == FindingState::Draft && finding.posted.is_none())
        // A question on a pull request stays with the agent, so it is not waiting to be posted
        .filter(|finding| {
            !pull_request || finding.severity != luxe_core::guided_review::FindingSeverity::Question
        })
        .count();
    let posted = count(|finding| finding.posted.is_some());
    let sent = |finding: &&luxe_core::guided_review::ReviewFinding| {
        matches!(finding.state, FindingState::Open | FindingState::NeedsVerification)
            && finding.posted.is_none()
    };
    let (waiting, back): (Vec<_>, Vec<_>) = session
        .findings
        .iter()
        .filter(sent)
        .partition(|finding| with_the_agent(session, &finding.id));
    let to_verify =
        back.iter().filter(|finding| finding.state == FindingState::NeedsVerification).count();
    // Open and back from the agent: it answered, said it could not, or luxe said it said nothing
    let to_read = back
        .iter()
        .filter(|finding| finding.state == FindingState::Open)
        .filter(|finding| {
            finding.thread.last().is_some_and(|message| {
                message.author != luxe_core::guided_review::FindingMessageAuthor::Reviewer
            })
        })
        .count();
    // Unsent leads: a count of what you have written that does not say none of it has left is the
    // wrong half of the fact. The keys that send it are the footer's, not repeated here.
    let tail = if unsent > 0 && pull_request {
        format!(" \u{b7} {unsent} unposted")
    } else if unsent > 0 {
        format!(" \u{b7} {unsent} unsent")
    } else if posted > 0 {
        format!(" \u{b7} {posted} on GitHub")
    } else if !waiting.is_empty() {
        format!(" \u{b7} {} with the agent", waiting.len())
    } else if to_verify > 0 {
        format!(" \u{b7} {to_verify} to verify")
    } else if to_read > 0 {
        format!(" \u{b7} {to_read} to read")
    } else {
        String::new()
    };
    Some(format!("{total}{tail}"))
}

/// The stacked tiers' one-line version, which carries its own affordance because it has no room
/// for a separate hint row.
pub(super) fn hidden_noise_summary(
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
) -> Option<String> {
    let (hunks, lines) = app.guided_review_workspace.hidden_counts(&review.files);
    (hunks > 0).then(|| format!("{} · f opens them", collapsed_banner(hunks, lines)))
}

/// The collapsed count: `12 mechanical changes collapsed · 148 lines`.
///
/// Nothing is withheld — each one is a row in the diff that says what it stands for, and `f` opens
/// every one of them at once.
pub(super) fn collapsed_banner(hunks: usize, lines: usize) -> String {
    format!(
        "{} collapsed · {}",
        plural(hunks, "mechanical change", "mechanical changes"),
        plural(lines, "line", "lines"),
    )
}

/// Folded hunks grouped by *what accounts for them*, most numerous first.
pub(super) fn folded_reasons(
    app: &App,
    review: &luxe_core::guided_review::GuidedReview,
) -> Vec<(&'static str, usize)> {
    use luxe_core::guided_review::NoiseKind;
    let mut counts: std::collections::BTreeMap<&'static str, usize> =
        std::collections::BTreeMap::new();
    for (_, hunk) in review
        .files
        .iter()
        .flat_map(|file| file.hunks.iter().map(move |hunk| (file, hunk)))
        .filter(|(file, hunk)| app.guided_review_workspace.is_hunk_hidden(file, hunk))
    {
        let label = match hunk.noise {
            NoiseKind::FormattingOnly => "whitespace-only, tokens identical",
            NoiseKind::Generated => "generated file",
            NoiseKind::Lockfile => "lockfile",
            NoiseKind::Mechanical => "mechanical rename or move",
            NoiseKind::CommentOnly | NoiseKind::Semantic | NoiseKind::Unknown => continue,
        };
        *counts.entry(label).or_default() += 1;
    }
    let mut reasons: Vec<(&'static str, usize)> = counts.into_iter().collect();
    reasons.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(right.0)));
    reasons
}
