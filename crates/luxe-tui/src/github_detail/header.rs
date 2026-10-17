//! The detail's header: `#182 Title`, who and where, and the state and labels as chips.

use luxe_core::github::overview::Label;
use luxe_core::github::{Author, PullRequestState};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::parts::fit;
use super::{GithubDetail, Subject};
use crate::theme::styles::ThemeStyles;

/// `#182 Title`, then who, where and how big, then the state and labels as chips.
pub(super) fn header(
    detail: &GithubDetail,
    width: usize,
    now: u64,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let (title, author, state, draft, labels, updated) = match &detail.subject {
        Subject::PullRequest(pr) => {
            (&pr.title, &pr.author, pr.state, pr.draft, &pr.labels, pr.updated_at)
        }
        Subject::Issue(issue) => {
            (&issue.title, &issue.author, issue.state, false, &issue.labels, issue.updated_at)
        }
    };
    let bold = styles.text.add_modifier(Modifier::BOLD);
    let title = Line::from(vec![
        Span::styled(format!("#{} ", detail.number()), styles.muted),
        Span::styled(luxe_core::github::untrusted(title, 300), bold),
    ]);
    let mut lines: Vec<Line> =
        crate::widgets::wrap_line(&title, width).into_iter().take(2).collect();

    let dot = || Span::styled(" · ", styles.dim);
    let mut meta = author_spans(author, styles);
    if let Subject::PullRequest(pr) = &detail.subject {
        meta.push(dot());
        meta.push(Span::styled(pr.base_ref.clone(), styles.text));
        meta.push(Span::styled(" ← ", styles.muted));
        meta.push(Span::styled(pr.head_ref.clone(), styles.text));
        meta.push(dot());
        meta.extend(diff_counts(pr.additions, pr.deletions, styles));
        let files = if pr.changed_files == 1 { "file" } else { "files" };
        meta.push(Span::styled(format!(" in {} {files}", pr.changed_files), styles.muted));
    }
    meta.push(dot());
    meta.push(Span::styled(ago(updated, now), styles.muted));
    lines.push(fit(meta, width));

    let mut chips = state_chip(state, draft, detail_is_issue(detail), styles);
    for label in labels.iter().take(4) {
        chips.push(Span::raw(" "));
        chips.extend(label_chip(label, styles));
    }
    lines.push(Line::default());
    lines.push(fit(chips, width));
    lines
}

fn detail_is_issue(detail: &GithubDetail) -> bool {
    matches!(detail.subject, Subject::Issue(_))
}

/// The state as a filled chip: `open`, `draft`, `merged`, `closed`, cut out of its own color.
fn state_chip(
    state: PullRequestState,
    draft: bool,
    issue: bool,
    styles: &ThemeStyles,
) -> Vec<Span<'static>> {
    let (word, color) = state_word(state, draft, issue, styles);
    let surface = color.fg.unwrap_or(Color::Reset);
    crate::widgets::slab(
        [Span::styled(word, Style::new().fg(styles.slab.ink))],
        surface,
        styles.slab.canvas,
        Style::new().add_modifier(Modifier::BOLD),
    )
}

/// The state's word and the color that goes with it, shared by the chip and the list.
pub fn state_word(
    state: PullRequestState,
    draft: bool,
    issue: bool,
    styles: &ThemeStyles,
) -> (&'static str, Style) {
    match (state, draft) {
        (PullRequestState::Open, true) => ("draft", styles.muted),
        (PullRequestState::Open, false) => ("open", styles.success),
        (PullRequestState::Merged, _) => ("merged", styles.accent_secondary),
        (PullRequestState::Closed, _) if issue => ("closed", styles.accent_secondary),
        (PullRequestState::Closed, _) => ("closed", styles.error),
    }
}

/// A label on the theme's chip surface, its own color only in the mark before its name: the
/// repository chose that color against GitHub's white page, not against this theme.
fn label_chip(label: &Label, styles: &ThemeStyles) -> Vec<Span<'static>> {
    let surface = styles.selected_bg.bg.unwrap_or(styles.slab.canvas);
    let tint = hex(&label.color).unwrap_or(styles.muted.fg.unwrap_or(Color::Reset));
    crate::widgets::slab(
        [
            Span::styled("● ", Style::new().fg(tint)),
            Span::styled(luxe_core::github::untrusted(&label.name, 40), styles.text),
        ],
        surface,
        styles.slab.canvas,
        Style::new(),
    )
}

fn hex(color: &str) -> Option<Color> {
    let color = color.trim_start_matches('#');
    if color.len() != 6 {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(color.get(at..at + 2)?, 16).ok();
    Some(Color::Rgb(channel(0)?, channel(2)?, channel(4)?))
}

/// `mara`, or `copilot-swe-agent bot` with the word quiet: a bot is named as one. A person's name
/// is the secondary accent, the color a speaker wears in the Conversation and in Guided Review.
pub fn author_spans(author: &Author, styles: &ThemeStyles) -> Vec<Span<'static>> {
    login_spans(&author.login, author.bot, styles)
}

pub(super) fn login_spans(login: &str, bot: bool, styles: &ThemeStyles) -> Vec<Span<'static>> {
    let login = login.trim_end_matches("[bot]");
    match (login.is_empty(), bot) {
        (true, _) => vec![Span::styled("ghost", styles.muted)],
        (false, true) => {
            vec![Span::styled(login.to_owned(), styles.muted), Span::styled(" bot", styles.dim)]
        }
        (false, false) => vec![Span::styled(login.to_owned(), styles.accent_secondary)],
    }
}

/// `+50 −12` in the diff colors.
pub fn diff_counts(additions: u64, deletions: u64, styles: &ThemeStyles) -> Vec<Span<'static>> {
    vec![
        Span::styled(format!("+{additions}"), styles.success),
        Span::styled(format!(" −{deletions}"), styles.error),
    ]
}

/// How long ago, from one clock: `3h ago`.
pub fn ago(at: u64, now: u64) -> String {
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(at);
    let now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(now);
    crate::dashboard::relative_age(at, now)
}
