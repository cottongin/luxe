//! The Conversation: everything said, oldest first, each entry a card of its own.
//!
//! The cards are the TUI's: the tone says what kind of thing was said (an open thread in the info
//! color, changes requested in the error color, a bot quieter than a person), a long comment folds
//! to its first lines, and a thread shows its last word until it's opened.

use luxe_core::github::overview::{by_bot_login, ConversationEntry, Verdict};
use luxe_core::github::{rfc3339_seconds, ConversationComment, ReviewThread, ThreadComment};

use super::{Context, BODY_MAX_CHARS, DOT};
use crate::render::escape;

/// Lines of a comment's source past which it folds. Agents write long, and a timeline of whole
/// comments is a wall; a fold that hides one line costs the line it saves.
const FOLD_LINES: usize = 7;

/// Characters of a thread's last word shown while it's folded, as the TUI shows it.
const LAST_WORD_CHARS: usize = 400;

/// The conversation's cards. `opened_by_you` names the description as yours when you wrote it.
pub(in crate::render) fn conversation(
    cx: &Context,
    entries: &[ConversationEntry],
    opened_by_you: bool,
) -> String {
    let cards: String = entries.iter().map(|entry| card(cx, entry, opened_by_you)).collect();
    format!("<div class=\"gh-conversation\">{cards}</div>")
}

fn card(cx: &Context, entry: &ConversationEntry, opened_by_you: bool) -> String {
    match entry {
        ConversationEntry::Opening { author, body, at } => {
            let title = speaker(&author.login, author.bot, opened_by_you)
                + &said("opened", "hue-muted")
                + &said(&cx.ago(*at), "hue-muted");
            said_card(cx, if author.bot { "quiet" } else { "person" }, &title, body)
        }
        ConversationEntry::Comment(comment) => {
            let title = speaker(&comment.author, comment.bot, comment.mine)
                + &said("commented", "hue-muted")
                + &when(cx, &comment.created_at);
            said_card(cx, if comment.bot { "quiet" } else { "person" }, &title, &comment.body)
        }
        ConversationEntry::Review { comment, verdict, line_comments } => {
            review_card(cx, comment, *verdict, *line_comments)
        }
        ConversationEntry::Thread(thread) => thread_card(cx, thread),
    }
}

/// `@mara` naming a speaker: yours in the accent, a bot's quiet and named as one, anyone else's
/// in the color a reviewer wears.
fn speaker(login: &str, bot: bool, mine: bool) -> String {
    let login = login.trim_end_matches("[bot]");
    if login.is_empty() {
        return "<span class=\"hue-muted\">@ghost</span>".to_owned();
    }
    let name = escape(&format!("@{login}"));
    match (mine, bot) {
        (true, _) => format!("<span class=\"gh-mine\">{name}</span>"),
        (false, true) => {
            format!("<span class=\"hue-muted\">{name}</span><span class=\"dim\"> bot</span>")
        }
        (false, false) => format!("<span class=\"hue-secondary\">{name}</span>"),
    }
}

/// ` · words` in `class`.
fn said(words: &str, class: &str) -> String {
    format!("{DOT}<span class=\"{class}\">{}</span>", escape(words))
}

/// ` · 3h ago` from GitHub's timestamp, or nothing when it can't be read.
fn when(cx: &Context, created_at: &str) -> String {
    rfc3339_seconds(created_at).map(|at| said(&cx.ago(at), "hue-muted")).unwrap_or_default()
}

/// A review's summary card, toned by its verdict unless a bot gave it.
fn review_card(
    cx: &Context,
    comment: &ConversationComment,
    verdict: Verdict,
    lines: usize,
) -> String {
    let (words, class, tone) = match verdict {
        Verdict::Approved => ("approved", "hue-success", "approved"),
        Verdict::ChangesRequested => ("requested changes", "hue-error", "changes"),
        Verdict::Commented => ("reviewed", "hue-muted", "person"),
        Verdict::Dismissed => ("review dismissed", "hue-muted", "person"),
        Verdict::Requested => ("asked to review", "hue-warning", "person"),
    };
    let title = speaker(&comment.author, comment.bot, comment.mine)
        + &said(words, class)
        + &line_comments(lines)
        + &when(cx, &comment.created_at);
    said_card(cx, if comment.bot { "quiet" } else { tone }, &title, &comment.body)
}

fn line_comments(count: usize) -> String {
    match count {
        0 => String::new(),
        1 => said("1 line comment", "hue-muted"),
        n => said(&format!("{n} line comments"), "hue-muted"),
    }
}

/// A card for something said: its title, then its body, folded when it runs long.
fn said_card(cx: &Context, tone: &str, title: &str, body: &str) -> String {
    let rendered = cx.body(body);
    let fold = folds(body);
    let (class, unfold) = match fold {
        true => (" folded", unfold_button(cx)),
        false => ("", String::new()),
    };
    format!(
        "<section class=\"gh-card tone-{tone}{class}\"><p class=\"gh-card-title\">{title}</p>\
         <div class=\"gh-body md\">{rendered}</div>{unfold}</section>"
    )
}

/// Whether a body says more lines than a card shows before it folds.
fn folds(body: &str) -> bool {
    let clean = luxe_core::github::untrusted(body, BODY_MAX_CHARS);
    clean.lines().filter(|line| !line.trim().is_empty()).count() > FOLD_LINES
}

fn unfold_button(cx: &Context) -> String {
    let lead = match cx.marks.elision.is_empty() {
        true => String::new(),
        false => format!("{} ", escape(&cx.marks.elision)),
    };
    format!("<button type=\"button\" class=\"gh-unfold\">{lead}Show all</button>")
}

/// A review thread: who started it, where, whether it's settled, then its last word folded or
/// every comment open, as a native disclosure so it opens without the app.
fn thread_card(cx: &Context, thread: &ReviewThread) -> String {
    let first = thread.comments.first();
    let starter = first.map(|comment| comment.author.as_str()).unwrap_or_default();
    let (state, class, tone) = match (thread.resolved, thread.outdated) {
        (true, _) => ("resolved", "dim", "person"),
        (false, true) => ("outdated", "dim", "person"),
        (false, false) => ("unresolved", "hue-info", "unresolved"),
    };
    let title = speaker(starter, first.is_some_and(by_bot_login), false)
        + &format!("{DOT}{}", escape(&place(thread)))
        + &said(state, class)
        + &replies(thread.total_comments);
    let comments: String =
        thread.comments.iter().map(|comment| thread_comment(cx, comment)).collect();
    format!(
        "<details class=\"gh-card gh-thread tone-{tone}\"><summary><span class=\"gh-card-title\">\
         {title}</span>{}</summary>{comments}</details>",
        last_word(thread)
    )
}

/// `src/api/webhook.rs:42`, the path cut to its last two parts when it's long.
fn place(thread: &ReviewThread) -> String {
    let parts: Vec<&str> = thread.path.split('/').collect();
    let path = match parts.len() {
        0..=3 => thread.path.clone(),
        n => format!("\u{2026}/{}", parts[n - 2..].join("/")),
    };
    match thread.line {
        Some(line) => format!("{path}:{line}"),
        None => path,
    }
}

fn replies(total: usize) -> String {
    match total.saturating_sub(1) {
        0 => String::new(),
        1 => said("1 reply", "hue-muted"),
        n => said(&format!("{n} replies"), "hue-muted"),
    }
}

/// What a folded thread shows: the last comment, on one line.
fn last_word(thread: &ReviewThread) -> String {
    let Some(comment) = thread.comments.last() else {
        return String::new();
    };
    let words = luxe_core::github::untrusted(&comment.body, LAST_WORD_CHARS);
    let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
    let who = format!("@{}: ", comment.author.trim_end_matches("[bot]"));
    format!(
        "<span class=\"gh-last\"><span class=\"hue-muted\">{}</span>{}</span>",
        escape(&who),
        escape(&words)
    )
}

/// One comment in an opened thread: who and when, then what they said.
fn thread_comment(cx: &Context, comment: &ThreadComment) -> String {
    let head =
        speaker(&comment.author, by_bot_login(comment), false) + &when(cx, &comment.created_at);
    format!(
        "<div class=\"gh-reply\"><p class=\"gh-card-title\">{head}</p><div class=\"gh-body \
         md\">{}</div></div>",
        cx.body(&comment.body)
    )
}
