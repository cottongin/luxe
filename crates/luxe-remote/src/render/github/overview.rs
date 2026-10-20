//! The detail's header and its Overview: what stands between a pull request and merged, or who
//! is on an issue, then the start of the description.
//!
//! The merge checks are core's (`merge_checks`), so the phone and the TUI agree on what blocks a
//! merge. Their actions are GitHub writes or agent work, which a joined session doesn't take, so a
//! row shows its evidence and no key; only "open #N", which reads, is offered.

use luxe_core::github::overview::{
    blocking, merge_checks, CheckAction, CheckMark, IssueOverview, Label, LinkedPullRequest,
    MergeCheck, Overview, PullRequestOverview, Reviewer, ReviewerKind, Verdict,
};
use luxe_core::github::{Author, PullRequestState};

use super::{counts, login, state_word, title, Context, BODY_MAX_CHARS, DOT};
use crate::render::escape;

/// Lines of the description the Overview quotes before pointing at Conversation.
const EXCERPT_LINES: usize = 5;

/// Labels the header shows, as the TUI's does.
const HEADER_LABELS: usize = 4;

/// `#182 Title`, then who, where and how big, then the state and labels as chips.
pub(in crate::render) fn header(cx: &Context, overview: &Overview) -> String {
    let (head, meta) = match overview {
        Overview::PullRequest(pr) => (heading_facts(pr), pull_request_meta(pr)),
        Overview::Issue(issue) => (issue_facts(issue), String::new()),
    };
    format!(
        "<header class=\"gh-head\"><h2 class=\"gh-heading\"><span class=\"dim\">#{} </span>{}</h2>\
         <p class=\"gh-meta\">{}{meta}{DOT}<span class=\"dim\">{}</span></p>\
         <p class=\"gh-chips\">{}</p></header>",
        overview.number(),
        title(head.title),
        login(&head.author.login, head.author.bot),
        cx.ago(head.updated_at),
        chips(&head)
    )
}

/// What the header says about either kind.
struct HeadFacts<'a> {
    title: &'a str,
    author: &'a Author,
    state: PullRequestState,
    draft: bool,
    issue: bool,
    labels: &'a [Label],
    updated_at: u64,
}

fn heading_facts(pr: &PullRequestOverview) -> HeadFacts<'_> {
    let (title, author, labels) = (&pr.title, &pr.author, &pr.labels);
    HeadFacts {
        title,
        author,
        state: pr.state,
        draft: pr.draft,
        issue: false,
        labels,
        updated_at: pr.updated_at,
    }
}

fn issue_facts(issue: &IssueOverview) -> HeadFacts<'_> {
    let (title, author, labels) = (&issue.title, &issue.author, &issue.labels);
    HeadFacts {
        title,
        author,
        state: issue.state,
        draft: false,
        issue: true,
        labels,
        updated_at: issue.updated_at,
    }
}

/// ` · master ← feature · +8 −8 in 4 files`.
fn pull_request_meta(pr: &PullRequestOverview) -> String {
    let files = if pr.changed_files == 1 { "file" } else { "files" };
    format!(
        "{DOT}{} <span class=\"hue-muted\">\u{2190}</span> {}{DOT}{} \
         <span class=\"hue-muted\">in {} {files}</span>",
        escape(&pr.base_ref),
        escape(&pr.head_ref),
        counts(pr.additions, pr.deletions),
        pr.changed_files
    )
}

/// The state as a filled chip cut from its own color, then up to four labels on the chip surface.
fn chips(head: &HeadFacts) -> String {
    let (word, class) = state_word(head.state, head.draft, head.issue);
    let mut out = format!("<span class=\"gh-state {class}\">{word}</span>");
    for label in head.labels.iter().take(HEADER_LABELS) {
        let name = escape(&luxe_core::github::untrusted(&label.name, 40));
        out += &format!(" <span class=\"gh-label-chip\"><span class=\"hue-muted\">\u{25cf}</span> {name}</span>");
    }
    out
}

/// The Overview's sections, under the header.
pub(in crate::render) fn overview(cx: &Context, overview: &Overview) -> String {
    match overview {
        Overview::PullRequest(pr) => {
            status_section(cx, pr) + &reviewers_section(cx, pr) + &excerpt(cx, &pr.body)
        }
        Overview::Issue(issue) => issue_section(cx, issue) + &excerpt(cx, &issue.body),
    }
}

/// A section's heading, with a quiet note against the far edge.
fn section_heading(text: &str, note: Option<String>) -> String {
    let note = note.map(|note| format!("<span class=\"gh-right dim\">{}</span>", escape(&note)));
    format!("<h3 class=\"gh-section\">{text}{}</h3>", note.unwrap_or_default())
}

/// One row: its mark, a label in the label column, then the evidence.
fn check_row(cx: &Context, mark: CheckMark, label: &str, detail: &str) -> String {
    let (glyph, class) = match mark {
        CheckMark::Pass => (cx.marks.yes.as_str(), "hue-success"),
        CheckMark::Fail => (cx.marks.no.as_str(), "hue-error"),
        CheckMark::Wait => (cx.marks.waiting.as_str(), "hue-warning"),
        CheckMark::Note => ("\u{2013}", "hue-muted"),
    };
    format!(
        "<p class=\"gh-check\"><span class=\"gh-mark {class}\">{}</span>\
         <span class=\"gh-label\">{}</span><span class=\"gh-evidence\">{detail}</span></p>",
        escape(glyph),
        escape(label)
    )
}

/// The checks between the pull request and merged, blockers first.
fn status_section(cx: &Context, pr: &PullRequestOverview) -> String {
    let checks = merge_checks(pr);
    if checks.is_empty() {
        return String::new();
    }
    let note = match blocking(&checks) {
        0 => None,
        count => Some(format!("{count} incomplete")),
    };
    let rows: String = checks.iter().map(|check| merge_row(cx, check)).collect();
    section_heading("Status", note) + &rows
}

/// A merge check's row: a failure's evidence in body text, the rest quiet, and a way to the pull
/// request below it in a stack, the one action that only reads.
fn merge_row(cx: &Context, check: &MergeCheck) -> String {
    let tone = if check.mark == CheckMark::Fail { "" } else { "hue-muted" };
    let mut detail = format!("<span class=\"{tone}\">{}</span>", escape(&check.detail));
    if let Some(CheckAction::OpenBelow(number)) = check.action {
        detail += &format!(
            " <button type=\"button\" class=\"gh-open\" data-number=\"{number}\" \
             data-pull=\"true\">open #{number}</button>"
        );
    }
    check_row(cx, check.mark, check.label, &detail)
}

/// Each reviewer, with what they said and when.
fn reviewers_section(cx: &Context, pr: &PullRequestOverview) -> String {
    if pr.reviewers.is_empty() {
        return String::new();
    }
    let approved = pr.reviewers.iter().filter(|r| r.verdict == Verdict::Approved).count();
    let note = format!("{approved} of {} approved", pr.reviewers.len());
    let rows: String = pr.reviewers.iter().map(|reviewer| reviewer_row(cx, reviewer)).collect();
    section_heading("Reviewers", Some(note)) + &rows
}

fn reviewer_row(cx: &Context, reviewer: &Reviewer) -> String {
    let (mark, said, class) = match reviewer.verdict {
        Verdict::Approved => (CheckMark::Pass, "approved", "hue-success"),
        Verdict::ChangesRequested => (CheckMark::Fail, "changes requested", "hue-error"),
        Verdict::Commented => (CheckMark::Note, "commented", "hue-muted"),
        Verdict::Dismissed => (CheckMark::Note, "dismissed", "hue-muted"),
        Verdict::Requested => (CheckMark::Wait, "requested", "hue-warning"),
    };
    let kind = match reviewer.kind {
        ReviewerKind::Team => " \u{b7} team",
        ReviewerKind::Bot => " \u{b7} bot",
        ReviewerKind::User => "",
    };
    let when = reviewer.at.map(|at| format!(" \u{b7} {}", cx.ago(at))).unwrap_or_default();
    let detail = format!(
        "<span class=\"{class}\">{said}</span><span class=\"dim\">{kind}</span>\
         <span class=\"hue-muted\">{when}</span>"
    );
    check_row(cx, mark, &reviewer.login, &detail)
}

/// Who's on an issue, which pull requests close it, and its milestone.
fn issue_section(cx: &Context, issue: &IssueOverview) -> String {
    let assignees = match issue.assignees.as_slice() {
        [] => "<span class=\"hue-muted\">unassigned</span>".to_owned(),
        logins => format!("<span class=\"hue-secondary\">{}</span>", escape(&logins.join(", "))),
    };
    let mut rows = check_row(cx, CheckMark::Note, "Assignees", &assignees);
    rows += &closing_rows(cx, issue);
    if let Some(milestone) = &issue.milestone {
        rows += &check_row(cx, CheckMark::Note, "Milestone", &escape(milestone));
    }
    section_heading("Status", None) + &rows
}

/// The pull requests that close the issue when they merge, each a way to open it.
fn closing_rows(cx: &Context, issue: &IssueOverview) -> String {
    let closing: Vec<_> = issue.linked.iter().filter(|linked| linked.closes).collect();
    if closing.is_empty() {
        let none = "<span class=\"hue-muted\">none yet</span>";
        return check_row(cx, CheckMark::Note, "Pull requests", none);
    }
    let label = |index| if index == 0 { "Pull requests" } else { "" };
    closing
        .iter()
        .enumerate()
        .map(|(index, linked)| closing_row(cx, label(index), linked))
        .collect()
}

/// Which pull request, how far along, and whose: the question the row answers is whether
/// somebody is already on it.
fn closing_row(cx: &Context, label: &str, linked: &LinkedPullRequest) -> String {
    let (word, class) = state_word(linked.state, linked.draft, false);
    let mark = match linked.state {
        PullRequestState::Open => CheckMark::Wait,
        PullRequestState::Merged => CheckMark::Pass,
        PullRequestState::Closed => CheckMark::Note,
    };
    let detail = format!(
        "<button type=\"button\" class=\"gh-open\" data-number=\"{0}\" data-pull=\"true\">#{0}\
         </button>{DOT}<span class=\"{class}\">{word}</span>{DOT}{1}",
        linked.number,
        login(&linked.author, false)
    );
    check_row(cx, mark, label, &detail)
}

/// A heading and the first lines of the description, pointing at Conversation for the rest.
fn excerpt(cx: &Context, body: &str) -> String {
    let heading = section_heading("Description", None);
    let readable = cx.renderer.github_readable(&luxe_core::github::untrusted(body, BODY_MAX_CHARS));
    let (kept, hidden) = first_lines(&readable, EXCERPT_LINES);
    if kept.trim().is_empty() {
        return heading + "<p class=\"gh-note dim\">No description</p>";
    }
    let rendered = cx.renderer.markdown(&kept, crate::render::Voice::Answer);
    let more = match hidden {
        0 => String::new(),
        hidden => format!(
            "<button type=\"button\" class=\"gh-more\" data-tab=\"conversation\">{} {hidden} more \
             lines in Conversation</button>",
            escape(&cx.marks.elision)
        ),
    };
    format!("{heading}<div class=\"gh-body md\">{rendered}</div>{more}")
}

/// The first `keep` lines of `text` that say something, with the blank lines between them, and
/// how many lines that say something are left: a paragraph break isn't worth one of the five.
fn first_lines(text: &str, keep: usize) -> (String, usize) {
    let mut said = 0;
    let mut kept = String::new();
    let mut lines = text.lines();
    for line in lines.by_ref() {
        if !line.trim().is_empty() {
            if said == keep {
                let hidden = 1 + lines.filter(|line| !line.trim().is_empty()).count();
                return (kept, hidden);
            }
            said += 1;
        }
        kept += line;
        kept.push('\n');
    }
    (kept, 0)
}
