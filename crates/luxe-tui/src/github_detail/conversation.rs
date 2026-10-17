//! The Conversation: everything said, oldest first, each entry a card of its own.
//!
//! The cards are Guided Review's ([`crate::widgets::card`]): a comment looks like a comment
//! wherever it turns up. The tone says what kind of thing was said — an open thread in the info
//! color, changes requested in the error color, a bot quieter than a person — and the entry under
//! the cursor wears the accent on its frame.

use luxe_core::github::overview::{ConversationEntry, Verdict};
use luxe_core::github::{rfc3339_seconds, Author, ReviewThread};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::header::ago;
use super::parts::{fit, indented, markdown};
use super::{Draft, GithubDetail};
use crate::glyphs::Mark;
use crate::theme::styles::ThemeStyles;

/// Rendered lines a comment shows before it folds. Agents write long, and a timeline of whole
/// comments is a wall.
const FOLD_LINES: usize = 6;

/// Cells a reply to a thread steps in from the thread it answers.
const NESTED: usize = 4;

/// The Conversation's lines, and which one the cursor is on.
pub(super) fn conversation_lines(
    detail: &GithubDetail,
    width: usize,
    focused: bool,
    now: u64,
    styles: &ThemeStyles,
) -> (Vec<Line<'static>>, Option<usize>) {
    let Some(entries) = &detail.conversation else {
        let said = match &detail.conversation_error {
            Some(reason) => Line::from(crate::widgets::code_spans(reason, styles.error, styles)),
            None => Line::from(Span::styled("Reading the conversation…", styles.muted)),
        };
        return (vec![indented(said)], None);
    };
    let opened_by_you =
        matches!(&detail.subject, super::Subject::PullRequest(pr) if pr.viewer.author);
    let mut lines = Vec::new();
    let mut cursor_line = None;
    let mut stop = 0;
    let mut hidden = 0;
    let mut chosen = |lines: &Vec<Line<'static>>, stop: usize| {
        let selected = focused && stop == detail.cursor;
        if selected {
            cursor_line = Some(lines.len());
        }
        selected
    };
    for (index, entry) in entries.iter().enumerate() {
        if detail.hide_bots && entry.by_bot() {
            hidden += 1;
            continue;
        }
        let selected = chosen(&lines, stop);
        let open = detail.open.contains(&index);
        lines.extend(entry_card(entry, opened_by_you, open, selected, width, now, styles));
        stop += 1;
        if let ConversationEntry::Thread(thread) = entry {
            for draft in detail.drafts.iter().filter(|d| d.thread.as_deref() == Some(&thread.id)) {
                let selected = chosen(&lines, stop);
                lines.extend(draft_card(draft, selected, width, styles));
                stop += 1;
            }
        }
        lines.push(Line::default());
    }
    for draft in detail.drafts.iter().filter(|draft| draft.thread.is_none()) {
        let selected = chosen(&lines, stop);
        lines.extend(draft_card(draft, selected, width, styles));
        lines.push(Line::default());
        stop += 1;
    }
    if hidden > 0 {
        let said = match hidden {
            1 => "1 entry by a bot hidden".to_string(),
            count => format!("{count} entries by bots hidden"),
        };
        lines.push(indented(Line::from(vec![
            Span::styled(format!("{said} · "), styles.muted),
            Span::styled("b", styles.accent),
            Span::styled(" shows them", styles.text),
        ])));
    }
    (lines, cursor_line)
}

/// `@mara` as a card's title names a speaker: yours in the accent, a bot's quiet and named as one,
/// anyone else's in the color Guided Review gives a reviewer.
fn speaker(login: &str, bot: bool, mine: bool, styles: &ThemeStyles) -> Vec<Span<'static>> {
    let login = login.trim_end_matches("[bot]");
    if login.is_empty() {
        return vec![Span::styled("a deleted account", styles.muted)];
    }
    let name = format!("@{login}");
    match (mine, bot) {
        (true, _) => vec![Span::styled(name, styles.accent)],
        (false, true) => vec![Span::styled(name, styles.muted), Span::styled(" bot", styles.dim)],
        (false, false) => vec![Span::styled(name, styles.accent_secondary)],
    }
}

/// A card's frame and wash: what kind of thing was said, unless the cursor is on it.
fn tone(selected: bool, kind: (Style, Style), styles: &ThemeStyles) -> (Style, Style) {
    match selected {
        true => (styles.accent, kind.1),
        false => kind,
    }
}

/// One entry as a card. Args: `(entry, opened_by_you, open, selected, width, now, styles)` —
/// `opened_by_you` names the description as yours when the reader wrote it.
fn entry_card(
    entry: &ConversationEntry,
    opened_by_you: bool,
    open: bool,
    selected: bool,
    width: usize,
    now: u64,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let inner = crate::widgets::card_inner_width(width, 0);
    let dot = || Span::styled(" · ", styles.dim);
    let person = (styles.dim, styles.user_msg_bg);
    let quiet = (styles.dim, Style::default());
    let (kind, title, body) = match entry {
        ConversationEntry::Opening { author, body, at } => {
            let Author { login, bot } = author;
            let mut title = speaker(login, *bot, opened_by_you, styles);
            title.extend([dot(), Span::styled("opened", styles.muted)]);
            title.extend([dot(), Span::styled(ago(*at, now), styles.muted)]);
            let kind = if *bot { quiet } else { person };
            (kind, title, folded(markdown(body, inner, styles), open, styles))
        }
        ConversationEntry::Comment(comment) => {
            let mut title = speaker(&comment.author, comment.bot, comment.mine, styles);
            title.extend([dot(), Span::styled("commented", styles.muted)]);
            if let Some(at) = rfc3339_seconds(&comment.created_at) {
                title.extend([dot(), Span::styled(ago(at, now), styles.muted)]);
            }
            let kind = if comment.bot { quiet } else { person };
            (kind, title, folded(markdown(&comment.body, inner, styles), open, styles))
        }
        ConversationEntry::Review { comment, verdict, line_comments } => {
            let (said, style, kind) = match verdict {
                Verdict::Approved => {
                    ("approved", styles.success, (styles.success, styles.user_msg_bg))
                }
                Verdict::ChangesRequested => {
                    ("requested changes", styles.error, (styles.error, styles.card_error_bg))
                }
                Verdict::Commented => ("reviewed", styles.muted, person),
                Verdict::Dismissed => ("review dismissed", styles.muted, person),
                Verdict::Requested => ("asked to review", styles.warning, person),
            };
            let kind = if comment.bot { quiet } else { kind };
            let mut title = speaker(&comment.author, comment.bot, comment.mine, styles);
            title.extend([dot(), Span::styled(said, style)]);
            match line_comments {
                0 => {}
                1 => title.extend([dot(), Span::styled("1 line comment", styles.muted)]),
                n => {
                    title.extend([dot(), Span::styled(format!("{n} line comments"), styles.muted)])
                }
            }
            if let Some(at) = rfc3339_seconds(&comment.created_at) {
                title.extend([dot(), Span::styled(ago(at, now), styles.muted)]);
            }
            (kind, title, folded(markdown(&comment.body, inner, styles), open, styles))
        }
        ConversationEntry::Thread(thread) => thread_parts(thread, open, inner, now, styles),
    };
    let (frame, fill) = tone(selected, kind, styles);
    crate::widgets::card(frame, fill, title, body, width, 0)
}

/// A review thread's tone, title and body: the last word when folded, every comment when open.
fn thread_parts(
    thread: &ReviewThread,
    open: bool,
    inner: usize,
    now: u64,
    styles: &ThemeStyles,
) -> ((Style, Style), Vec<Span<'static>>, Vec<Line<'static>>) {
    let dot = || Span::styled(" · ", styles.dim);
    let fold = if open { Mark::FoldOpen } else { Mark::FoldClosed };
    let first = thread.comments.first();
    let starter = first.map(|comment| comment.author.as_str()).unwrap_or_default();
    let bot = first.is_some_and(luxe_core::github::overview::by_bot_login);
    let mut title = vec![Span::styled(format!("{} ", fold.glyph()), styles.muted)];
    title.extend(speaker(starter, bot, false, styles));
    title.push(Span::styled(" · ", styles.dim));
    let place = match thread.line {
        Some(line) => format!("{}:{line}", short_path(&thread.path)),
        None => short_path(&thread.path),
    };
    title.push(Span::styled(place, styles.text));
    // The same two tones Guided Review gives a reviewer's thread on its line
    let (state, style, kind) = match (thread.resolved, thread.outdated) {
        (true, _) => ("resolved", styles.dim, (styles.dim, styles.user_msg_bg)),
        (false, true) => ("outdated", styles.dim, (styles.dim, styles.user_msg_bg)),
        (false, false) => ("unresolved", styles.info, (styles.info, styles.card_info_bg)),
    };
    title.extend([dot(), Span::styled(state, style)]);
    match thread.total_comments.saturating_sub(1) {
        0 => {}
        1 => title.extend([dot(), Span::styled("1 reply", styles.muted)]),
        n => title.extend([dot(), Span::styled(format!("{n} replies"), styles.muted)]),
    }

    let mut body = Vec::new();
    if !open {
        if let Some(comment) = thread.comments.last() {
            let said = luxe_core::github::untrusted(&comment.body, 400);
            let said = said.split_whitespace().collect::<Vec<_>>().join(" ");
            let who = format!("@{}: ", comment.author.trim_end_matches("[bot]"));
            body.push(fit(
                vec![Span::styled(who, styles.muted), Span::styled(said, styles.text)],
                inner,
            ));
        }
        return (kind, title, body);
    }
    for (index, comment) in thread.comments.iter().enumerate() {
        if index > 0 {
            body.push(Line::default());
        }
        let bot = luxe_core::github::overview::by_bot_login(comment);
        let mut head = speaker(&comment.author, bot, false, styles);
        if let Some(at) = rfc3339_seconds(&comment.created_at) {
            head.extend([dot(), Span::styled(ago(at, now), styles.muted)]);
        }
        body.push(Line::from(head));
        body.extend(markdown(&comment.body, inner, styles));
    }
    (kind, title, body)
}

/// `lines`, cut at [`FOLD_LINES`] unless opened, saying how much is hidden.
fn folded(lines: Vec<Line<'static>>, open: bool, styles: &ThemeStyles) -> Vec<Line<'static>> {
    // A fold that hides one line costs the line it saves
    if open || lines.len() <= FOLD_LINES + 1 {
        return lines;
    }
    let hidden = lines.len() - FOLD_LINES;
    let mut kept: Vec<Line> = lines.into_iter().take(FOLD_LINES).collect();
    kept.push(Line::from(vec![
        Span::styled(format!("{} {hidden} more lines · ", Mark::Elision.glyph()), styles.muted),
        Span::styled("enter", styles.accent),
        Span::styled(" expand", styles.text),
    ]));
    kept
}

/// A draft as a card of its own, saying that it has not been sent.
///
/// One answering a thread steps in under it and is called a reply — where it goes is the thread
/// above it. A loose one names where it goes.
///
/// Your words, colored as your words are everywhere: the name in the accent and the body on the
/// band your messages wear in the transcript. The frame stays quiet, because an accent frame is the
/// cursor, and on a draft that always wore one the cursor could not be seen. Who wrote the words is
/// the draft card's to say, where they are read before they are sent.
fn draft_card(
    draft: &Draft,
    selected: bool,
    width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let nested = draft.thread.is_some();
    let indent = if nested { NESTED } else { 0 };
    let inner = crate::widgets::card_inner_width(width, indent);
    let name = if nested { "Your reply" } else { "Your draft" };
    let mut title = vec![Span::styled(name, styles.accent.add_modifier(Modifier::BOLD))];
    if !nested {
        title.push(Span::styled(" · ", styles.dim));
        title.extend(crate::widgets::code_spans(&draft.target, styles.text, styles));
    }
    title.extend([Span::styled(" · ", styles.dim), Span::styled("unsent", styles.warning)]);
    let body = folded(markdown(&draft.body, inner, styles), false, styles);
    let (frame, fill) = tone(selected, (styles.dim, styles.user_msg_bg), styles);
    crate::widgets::card(frame, fill, title, body, width, indent)
}

/// `src/api/webhook.rs` kept whole when it is short, else its last two parts.
fn short_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.len() {
        0..=3 => path.to_owned(),
        n => format!("…/{}", parts[n - 2..].join("/")),
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{conversation, pr, styles, text};
    use super::super::Subject;
    use super::*;

    /// A draft is your words: the name in the accent, the body on your messages' band, the frame
    /// quiet until the cursor is on it, and nothing about who drafted it.
    #[test]
    fn a_draft_wears_your_colors() {
        let styles = styles();
        let draft = Draft {
            target: "Reply to @jon".into(),
            thread: Some("t1".into()),
            body: "Fixed in 4c1e2a0.".into(),
            by_agent: true,
            sources: vec!["this session".into()],
            note: None,
        };
        let lines = draft_card(&draft, false, 60, &styles);
        let shown = text(&lines);
        assert!(shown[0].contains("Your reply") && shown[0].contains("unsent"), "{shown:?}");
        assert!(!shown[0].contains("agent"), "{shown:?}");
        let said = lines[1].spans.iter().find(|span| span.content.contains("Fixed")).expect("body");
        assert_eq!(said.style.bg, styles.user_msg_bg.bg);
        let edge = |lines: &[Line]| {
            lines[0].spans.iter().find(|s| s.content.contains('╭')).unwrap().style.fg
        };
        assert_eq!(edge(&lines), styles.dim.fg);
        assert_eq!(edge(&draft_card(&draft, true, 60, &styles)), styles.accent.fg, "the cursor");
    }

    /// A long comment folds and says how much it hides; a bot's entries go when asked.
    #[test]
    fn long_comments_fold_and_bots_can_be_hidden() {
        let mut detail = GithubDetail::new(Subject::PullRequest(Box::new(pr())));
        detail.conversation = Some(conversation());
        let (lines, _) = conversation_lines(&detail, 80, false, 4_600, &styles());
        let shown = text(&lines);
        assert!(shown
            .iter()
            .any(|line| line.contains("@coderabbitai bot") && line.contains("commented")));
        assert!(shown.iter().any(|line| line.contains("more lines · enter expand")), "{shown:#?}");
        detail.hide_bots = true;
        let (lines, _) = conversation_lines(&detail, 80, false, 4_600, &styles());
        let shown = text(&lines);
        assert!(!shown.iter().any(|line| line.contains("coderabbitai")));
        assert!(shown.last().unwrap().contains("1 entry by a bot hidden · b shows them"));
    }

    /// Each entry is its own card, its speaker in the top edge; the one under the cursor wears the
    /// accent on its frame.
    #[test]
    fn each_entry_is_a_card_and_the_chosen_one_is_marked() {
        let mut detail = GithubDetail::new(Subject::PullRequest(Box::new(pr())));
        detail.conversation = Some(conversation());
        let (lines, cursor) = conversation_lines(&detail, 80, true, 4_600, &styles());
        let shown = text(&lines);
        let tops = shown.iter().filter(|line| line.starts_with('╭')).count();
        assert_eq!(tops, 2, "{shown:#?}");
        assert!(shown[0].starts_with("╭─ @jon") && shown[0].ends_with('╮'), "{:?}", shown[0]);
        assert_eq!(cursor, Some(0));
        let accent = styles().accent;
        // The first span is the card's margin; the frame is the next
        assert_eq!(lines[0].spans[1].style.fg, accent.fg, "the chosen card's frame");
        let second = shown.iter().rposition(|line| line.starts_with('╭')).unwrap();
        assert_ne!(lines[second].spans[1].style.fg, accent.fg);
    }

    #[test]
    fn a_long_path_keeps_its_last_two_parts() {
        assert_eq!(short_path("src/api/webhook.rs"), "src/api/webhook.rs");
        assert_eq!(short_path("crates/core/src/api/webhook.rs"), "…/api/webhook.rs");
    }
}
