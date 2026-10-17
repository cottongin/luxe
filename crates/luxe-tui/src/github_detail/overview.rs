//! The Overview: what stands between a pull request and merged, or who is on an issue.

use luxe_core::github::overview::{
    blocking, merge_checks, CheckMark, IssueOverview, PullRequestOverview, ReviewerKind, Verdict,
};
use luxe_core::github::PullRequestState;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::header::{ago, state_word};
use super::parts::{
    action_hint, heading, indented, key_spans, label_cell, mark, prose, row, LABEL_CELLS,
};
use super::{GithubDetail, Subject};
use crate::glyphs::Mark;
use crate::theme::styles::ThemeStyles;

/// Lines of the description the Overview quotes before pointing at Conversation.
const EXCERPT_LINES: usize = 5;

/// The Overview's lines, and which one the cursor is on.
pub(super) fn overview(
    detail: &GithubDetail,
    width: usize,
    focused: bool,
    now: u64,
    styles: &ThemeStyles,
) -> (Vec<Line<'static>>, Option<usize>) {
    match &detail.subject {
        Subject::PullRequest(pr) => pull_request_overview(detail, pr, width, focused, now, styles),
        Subject::Issue(issue) => issue_overview(detail, issue, width, focused, now, styles),
    }
}

/// The Status section of a pull request: its checks, with the cursor's row when one is selected.
/// Returns the line the cursor is on.
///
/// Args: `(lines, detail, pr, (width, focused), styles)`.
fn status_section(
    lines: &mut Vec<Line<'static>>,
    detail: &GithubDetail,
    pr: &PullRequestOverview,
    (width, focused): (usize, bool),
    styles: &ThemeStyles,
) -> Option<usize> {
    let mut cursor_line = None;
    let checks = merge_checks(pr);
    if !checks.is_empty() {
        let note = match blocking(&checks) {
            0 => None,
            count => Some(format!("{count} incomplete")),
        };
        lines.push(heading("Status", note, width, styles));
        let mut stop = 0;
        for check in &checks {
            let acting = check.action.filter(|action| detail.acts(*action));
            let selected = focused && acting.is_some() && stop == detail.cursor;
            if selected {
                cursor_line = Some(lines.len());
            }
            let right = acting
                .map(|action| {
                    let (key, verb) = action_hint(action);
                    key_spans(&key, &verb, styles)
                })
                .unwrap_or_default();
            let detail_style = match check.mark {
                CheckMark::Fail => styles.text,
                _ => styles.muted,
            };
            lines.push(row(
                vec![
                    mark(check.mark, styles),
                    label_cell(check.label, styles),
                    Span::styled(check.detail.clone(), detail_style),
                ],
                right,
                width,
                selected,
                styles,
            ));
            stop += usize::from(acting.is_some());
        }
        lines.push(Line::default());
    }
    cursor_line
}

/// The Reviewers section: one row per reviewer, with what they said and when.
fn reviewers_section(
    lines: &mut Vec<Line<'static>>,
    pr: &PullRequestOverview,
    width: usize,
    now: u64,
    styles: &ThemeStyles,
) {
    if !pr.reviewers.is_empty() {
        let approved =
            pr.reviewers.iter().filter(|reviewer| reviewer.verdict == Verdict::Approved).count();
        let note = format!("{approved} of {} approved", pr.reviewers.len());
        lines.push(heading("Reviewers", Some(note), width, styles));
        let login_cells = pr
            .reviewers
            .iter()
            .map(|reviewer| reviewer.login.width())
            .max()
            .unwrap_or(0)
            .clamp(LABEL_CELLS, 20);
        for reviewer in &pr.reviewers {
            let (check, said, style) = match reviewer.verdict {
                Verdict::Approved => (CheckMark::Pass, "approved", styles.success),
                Verdict::ChangesRequested => (CheckMark::Fail, "changes requested", styles.error),
                Verdict::Commented => (CheckMark::Note, "commented", styles.muted),
                Verdict::Dismissed => (CheckMark::Note, "dismissed", styles.muted),
                Verdict::Requested => (CheckMark::Wait, "requested", styles.warning),
            };
            let login = crate::widgets::truncate_end(&reviewer.login, login_cells);
            let pad = " ".repeat(login_cells.saturating_sub(login.width()) + 1);
            let login_style = match reviewer.kind {
                ReviewerKind::User => styles.accent_secondary,
                _ => styles.muted,
            };
            let mut left = vec![
                mark(check, styles),
                Span::styled(login, login_style),
                Span::raw(pad),
                Span::styled(said, style),
            ];
            match reviewer.kind {
                ReviewerKind::Team => left.push(Span::styled(" · team", styles.dim)),
                ReviewerKind::Bot => left.push(Span::styled(" · bot", styles.dim)),
                ReviewerKind::User => {}
            }
            if let Some(at) = reviewer.at {
                left.push(Span::styled(format!(" · {}", ago(at, now)), styles.muted));
            }
            lines.push(row(left, Vec::new(), width, false, styles));
        }
        lines.push(Line::default());
    }
}

/// The assignees row of an issue.
fn assignees_row(issue: &IssueOverview, width: usize, styles: &ThemeStyles) -> Line<'static> {
    let assignees = match issue.assignees.as_slice() {
        [] => vec![Span::styled("unassigned", styles.muted)],
        logins => {
            let mut spans = Vec::new();
            for (index, login) in logins.iter().enumerate() {
                if index > 0 {
                    spans.push(Span::styled(", ", styles.muted));
                }
                spans.push(Span::styled(login.clone(), styles.accent_secondary));
            }
            spans
        }
    };
    let mut left = vec![mark(CheckMark::Note, styles), label_cell("Assignees", styles)];
    left.extend(assignees);
    row(left, Vec::new(), width, false, styles)
}

fn pull_request_overview(
    detail: &GithubDetail,
    pr: &PullRequestOverview,
    width: usize,
    focused: bool,
    now: u64,
    styles: &ThemeStyles,
) -> (Vec<Line<'static>>, Option<usize>) {
    let mut lines = Vec::new();
    let cursor_line = status_section(&mut lines, detail, pr, (width, focused), styles);

    reviewers_section(&mut lines, pr, width, now, styles);

    lines.extend(excerpt("Description", &pr.body, width, styles));
    (lines, cursor_line)
}

/// A row with a label and one value, nothing to press.
fn plain_row(
    label: &'static str,
    value: Span<'static>,
    width: usize,
    styles: &ThemeStyles,
) -> Line<'static> {
    let left = vec![mark(CheckMark::Note, styles), label_cell(label, styles), value];
    row(left, Vec::new(), width, false, styles)
}

fn issue_overview(
    detail: &GithubDetail,
    issue: &IssueOverview,
    width: usize,
    focused: bool,
    now: u64,
    styles: &ThemeStyles,
) -> (Vec<Line<'static>>, Option<usize>) {
    let mut lines = Vec::new();
    let mut cursor_line = None;
    lines.push(heading("Status", None, width, styles));

    lines.push(assignees_row(issue, width, styles));

    let mut stop = 0;
    let closing = closing_pull_requests(issue);
    if closing.is_empty() {
        lines.push(plain_row(
            "Pull requests",
            Span::styled("none yet", styles.muted),
            width,
            styles,
        ));
    }
    for (index, linked) in closing.into_iter().enumerate() {
        let selected = focused && stop == detail.cursor;
        if selected {
            cursor_line = Some(lines.len());
        }
        let (word, style) = state_word(linked.state, linked.draft, false, styles);
        let check = match linked.state {
            PullRequestState::Open => CheckMark::Wait,
            PullRequestState::Merged => CheckMark::Pass,
            PullRequestState::Closed => CheckMark::Note,
        };
        let label = if index == 0 { "Pull requests" } else { "" };
        // Which one, how far along, and whose: the title is a keypress away, and the question this
        // row answers is whether somebody is already on it
        lines.push(row(
            vec![
                mark(check, styles),
                label_cell(label, styles),
                Span::styled(format!("#{}", linked.number), styles.text),
                Span::styled(" · ", styles.dim),
                Span::styled(word, style),
                Span::styled(" · ", styles.dim),
                Span::styled(linked.author.clone(), styles.accent_secondary),
            ],
            key_spans("enter", "open", styles),
            width,
            selected,
            styles,
        ));
        stop += 1;
    }
    if let Some(local) = &detail.local {
        let selected = focused && stop == detail.cursor;
        if selected {
            cursor_line = Some(lines.len());
        }
        lines.push(row(
            vec![
                mark(CheckMark::Pass, styles),
                label_cell("Your session", styles),
                match &local.branch {
                    Some(branch) => Span::styled(branch.clone(), styles.accent),
                    None => Span::styled("in this checkout", styles.text),
                },
                Span::styled(format!(" · {}", ago(local.last_used, now)), styles.muted),
            ],
            key_spans("enter", "resume", styles),
            width,
            selected,
            styles,
        ));
    }
    if let Some(milestone) = &issue.milestone {
        lines.push(plain_row(
            "Milestone",
            Span::styled(milestone.clone(), styles.text),
            width,
            styles,
        ));
    }
    lines.push(Line::default());
    lines.extend(excerpt("Description", &issue.body, width, styles));
    (lines, cursor_line)
}

/// The pull requests that close `issue` when they merge, the ones GitHub lists under Development. A
/// pull request that only mentions it is not working on it.
pub(super) fn closing_pull_requests(
    issue: &IssueOverview,
) -> Vec<&luxe_core::github::overview::LinkedPullRequest> {
    issue.linked.iter().filter(|linked| linked.closes).collect()
}

/// A heading and the first lines of `body`, pointing at Conversation for the rest.
fn excerpt(title: &str, body: &str, width: usize, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let mut lines = vec![heading(title, None, width, styles)];
    let rendered = prose(body, width, styles);
    if rendered.is_empty() {
        lines.push(indented(Line::from(Span::styled("No description", styles.muted))));
        return lines;
    }
    // Lines of text, not rows: a paragraph break is not worth one of the five
    let blank = |line: &Line| line.spans.iter().all(|span| span.content.trim().is_empty());
    let mut said = 0;
    let mut kept = 0;
    for line in &rendered {
        if !blank(line) {
            if said == EXCERPT_LINES {
                break;
            }
            said += 1;
        }
        kept += 1;
    }
    let hidden = rendered[kept..].iter().filter(|line| !blank(line)).count();
    lines.extend(rendered.into_iter().take(kept));
    if lines.last().is_some_and(blank) {
        lines.pop();
    }
    if hidden > 0 {
        lines.push(indented(Line::from(Span::styled(
            format!("{} {hidden} more lines in Conversation", Mark::Elision.glyph()),
            styles.muted,
        ))));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{pr, styles, text};
    use super::*;

    /// Each row reads as a word beside its mark. A row whose action can be taken from here carries
    /// its key against the far edge; one that cannot shows its evidence and no key.
    #[test]
    fn the_overview_leads_with_what_is_incomplete() {
        let mut detail = GithubDetail::new(Subject::PullRequest(Box::new(pr())));
        detail.on_branch = true;
        let (_, cursor) = overview(&detail, 80, true, 4_600, &styles());
        assert_eq!(cursor, Some(1), "the first row that acts: Checks");
        // Unfocused, so no row wears the selection slab's caps
        let (lines, _) = overview(&detail, 80, false, 4_600, &styles());
        let text = text(&lines);
        assert!(text[0].contains("Status") && text[0].ends_with("2 incomplete"));
        assert!(
            text[1].contains("1 of 12 failing") && text[1].ends_with("f fix with agent"),
            "{:?}",
            text[1]
        );
        assert!(text[2].ends_with("t resolve threads"), "{:?}", text[2]);
        assert!(text.iter().any(|line| line.contains("jon") && line.contains("changes requested")));
        assert!(text.iter().any(|line| line.contains("Routes each digest")));
        assert_eq!(
            detail.hint("← list"),
            "enter fix with agent · tab conversation · ↑/↓ move · o open · ← list"
        );

        // Not the branch checked out here: its threads cannot be worked against this tree
        detail.on_branch = false;
        let (lines, cursor) = overview(&detail, 80, true, 4_600, &styles());
        assert_eq!(cursor, None);
        let shown = crate::github_detail::test_support::text(&lines);
        assert!(!shown[1].contains("fix with agent") && !shown[2].contains("resolve threads"));
    }
}
