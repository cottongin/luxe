//! The list side of `/github`: how many a tab's search found, then each pull request or issue on
//! the TUI's two lines.
//!
//! Line one is what it is: a state mark, the number, the title, how long ago it changed. Line two
//! is where it stands, quiet: who opened it, how big it is, and on the right its status in words.
//! A row is a button whose data says which item to open; the app owns the tap.

use luxe_core::github::overview::RowFacts;
use luxe_core::github::{
    rfc3339_seconds, ListItem, ListedSection, PullRequestState, ReviewDecision,
};

use super::{counts, grouped, login, marked, state_word, title, Context, DOT};
use crate::render::escape;

/// A tab's rows, or why it has none. `repository` is the checkout's, which a row from elsewhere
/// names its own beside.
pub(in crate::render) fn list(
    cx: &Context,
    repository: Option<&str>,
    section: &ListedSection,
) -> String {
    if let Some(error) = &section.error {
        return format!("<p class=\"gh-note hue-error\">{}</p>", escape(error));
    }
    if section.items.is_empty() {
        return "<p class=\"gh-note dim\">Nothing matches this search</p>".to_owned();
    }
    let rows: String = section.items.iter().map(|item| row(cx, item, repository)).collect();
    format!("<div class=\"gh-list\"><p class=\"gh-count dim\">{}</p>{rows}</div>", shown(section))
}

/// How many are here of how many there are. The rest are GitHub's to show: paging needs the
/// section's query, which the listing doesn't carry.
fn shown(section: &ListedSection) -> String {
    let loaded = section.items.len() as u64;
    match section.total {
        Some(total) if total > loaded => format!("{} of {}", grouped(loaded), grouped(total)),
        _ => format!("{} shown", grouped(loaded)),
    }
}

fn row(cx: &Context, item: &ListItem, repository: Option<&str>) -> String {
    let age = rfc3339_seconds(&item.updated_at).map(|at| cx.ago(at)).unwrap_or_default();
    format!(
        "<button type=\"button\" class=\"gh-row\" data-number=\"{}\" data-pull=\"{}\" \
         data-repository=\"{}\" data-url=\"{}\"><span class=\"gh-line\">{}\
         <span class=\"dim\">#{} </span><span class=\"gh-title\">{}</span>\
         <span class=\"gh-right dim\">{}</span></span>{}</button>",
        item.number,
        item.pull_request,
        escape(&item.repository),
        escape(&item.url),
        row_mark(cx, item),
        item.number,
        title(&item.title),
        age.trim_end_matches(" ago"),
        second_line(cx, item, repository.is_some_and(|here| here != item.repository))
    )
}

/// A pull request's or an issue's mark, in its state's color.
fn row_mark(cx: &Context, item: &ListItem) -> String {
    let issue = !item.pull_request;
    let glyph = if issue { &cx.marks.issue } else { &cx.marks.pull_request };
    if glyph.is_empty() {
        return String::new();
    }
    let (_, class) = state_word(item.state, item.draft, issue);
    format!("<span class=\"gh-mark {class}\">{}</span>", escape(glyph))
}

/// Who opened it, from where when it's another repository, and how big it is, then its status.
fn second_line(cx: &Context, item: &ListItem, elsewhere: bool) -> String {
    let facts = item.facts.as_ref();
    let mut left = login(&item.author, facts.is_some_and(|facts| facts.author_bot));
    if elsewhere {
        left += &format!("{DOT}<span class=\"hue-muted\">{}</span>", escape(&item.repository));
    }
    if let Some(facts) = facts {
        left += &size(facts, item.pull_request);
    }
    format!(
        "<span class=\"gh-line gh-sub\"><span class=\"gh-who\">{left}</span>\
         <span class=\"gh-right\">{}</span></span>",
        status(cx, item, facts).join(DOT)
    )
}

/// A pull request's diff counts, or an issue's labels, after a separator; nothing for an issue
/// without labels.
fn size(facts: &RowFacts, pull_request: bool) -> String {
    if pull_request {
        return format!("{DOT}{}", counts(facts.additions, facts.deletions));
    }
    if facts.labels.is_empty() {
        return String::new();
    }
    let names: Vec<&str> = facts.labels.iter().map(|label| label.name.as_str()).collect();
    format!("{DOT}<span class=\"hue-muted\">{}</span>", escape(&names.join(", ")))
}

/// Where it stands, in words: `✗ 1 failing · changes requested`.
fn status(cx: &Context, item: &ListItem, facts: Option<&RowFacts>) -> Vec<String> {
    let mut parts = Vec::new();
    let (word, class) = state_word(item.state, item.draft, !item.pull_request);
    if item.draft || item.state != PullRequestState::Open {
        parts.push(marked("", word, class));
    }
    let Some(facts) = facts else {
        return parts;
    };
    match item.pull_request {
        true => parts.extend(pull_request_status(cx, facts)),
        false => parts.extend(issue_status(cx, facts)),
    }
    parts
}

fn issue_status(cx: &Context, facts: &RowFacts) -> Option<String> {
    if let Some(number) = facts.linked_pull_request {
        return Some(marked(&cx.marks.waiting, &format!("PR #{number}"), "hue-warning"));
    }
    let noun = if facts.comments == 1 { "comment" } else { "comments" };
    (facts.comments > 0).then(|| marked("", &format!("{} {noun}", facts.comments), "hue-muted"))
}

fn pull_request_status(cx: &Context, facts: &RowFacts) -> Vec<String> {
    let checks = &facts.checks;
    let mut parts = Vec::new();
    if checks.failed > 0 {
        parts.push(marked(&cx.marks.no, &format!("{} failing", checks.failed), "hue-error"));
    } else if checks.pending > 0 {
        parts.push(marked(&cx.marks.waiting, "running", "hue-warning"));
    } else if checks.passed > 0 {
        parts.push(marked(&cx.marks.yes, "checks", "hue-success"));
    }
    let review = match facts.review_decision {
        Some(ReviewDecision::ChangesRequested) => Some(("changes requested", "hue-error")),
        Some(ReviewDecision::Approved) => Some(("approved", "hue-success")),
        Some(ReviewDecision::ReviewRequired) => Some(("needs review", "hue-muted")),
        None => None,
    };
    parts.extend(review.map(|(word, class)| marked("", word, class)));
    parts
}
