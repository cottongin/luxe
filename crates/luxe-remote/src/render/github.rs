//! GitHub — `/github`'s list, overview and conversation, drawn as the TUI draws them.
//!
//! The engine fetched everything and the app hands it back as pieces, so the bridge stays
//! stateless. Every fact is a word beside its mark, as in the TUI, so a missing glyph loses polish
//! and nothing else. Titles and bodies are strangers' words: sanitized by core, reduced from
//! GitHub's HTML by the renderer, and rendered escaped. Nothing here offers a write: a joined
//! session runs with GitHub writes off.

mod conversation;
mod list;
mod overview;

pub(super) use conversation::conversation;
pub(super) use list::list;
pub(super) use overview::{header, overview};

use luxe_core::github::PullRequestState;

use super::{escape, Renderer, Voice};

/// Characters of a body rendered before it's cut, as the TUI's detail cuts it.
const BODY_MAX_CHARS: usize = 20_000;

/// Characters of a title shown, as the TUI's header shows it.
const TITLE_MAX_CHARS: usize = 300;

/// The separator between facts on a line.
const DOT: &str = "<span class=\"dim\"> \u{b7} </span>";

/// The marks `/github` draws, as plain text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GithubMarks {
    /// Before a pull request's row
    pub pull_request: String,
    /// Before an issue's row
    pub issue: String,
    /// Passed or approved
    pub yes: String,
    /// Failed or refused
    pub no: String,
    /// Running or waiting on someone
    pub waiting: String,
    /// Before a count of lines left out
    pub elision: String,
}

/// What every GitHub view draws with: the renderer, its marks, and the app's clock, which the app
/// sends so a piece renders the same whenever it's asked.
pub(super) struct Context<'a> {
    renderer: &'a dyn Renderer,
    marks: GithubMarks,
    now: u64,
}

impl<'a> Context<'a> {
    pub(super) fn new(renderer: &'a dyn Renderer, now: u64) -> Self {
        Self { renderer, marks: renderer.github_marks(), now }
    }

    /// How long ago `at` was, in the TUI's words, escaped; empty from a renderer without them.
    fn ago(&self, at: u64) -> String {
        escape(&self.renderer.ago(at, self.now))
    }

    /// A stranger's markdown as HTML: sanitized, its HTML reduced, rendered escaped.
    fn body(&self, text: &str) -> String {
        let clean = luxe_core::github::untrusted(text, BODY_MAX_CHARS);
        let readable = self.renderer.github_readable(&clean);
        if readable.trim().is_empty() {
            return String::new();
        }
        self.renderer.markdown(&readable, Voice::Answer)
    }
}

/// A title, sanitized and escaped.
fn title(text: &str) -> String {
    escape(&luxe_core::github::untrusted(text, TITLE_MAX_CHARS))
}

/// The state's word and the class that colors it, shared by a row's mark and the state chip.
fn state_word(state: PullRequestState, draft: bool, issue: bool) -> (&'static str, &'static str) {
    match (state, draft) {
        (PullRequestState::Open, true) => ("draft", "hue-muted"),
        (PullRequestState::Open, false) => ("open", "hue-success"),
        (PullRequestState::Merged, _) => ("merged", "hue-secondary"),
        (PullRequestState::Closed, _) if issue => ("closed", "hue-secondary"),
        (PullRequestState::Closed, _) => ("closed", "hue-error"),
    }
}

/// `mara`, or `dependabot bot` with the word quiet: a bot is named as one. `ghost` for a
/// deleted account.
fn login(login: &str, bot: bool) -> String {
    let login = login.trim_end_matches("[bot]");
    match (login.is_empty(), bot) {
        (true, _) => "<span class=\"hue-muted\">ghost</span>".to_owned(),
        (false, true) => format!(
            "<span class=\"hue-muted\">{}</span><span class=\"dim\"> bot</span>",
            escape(login)
        ),
        (false, false) => format!("<span class=\"hue-secondary\">{}</span>", escape(login)),
    }
}

/// `+50 −12` in the diff colors.
fn counts(additions: u64, deletions: u64) -> String {
    format!(
        "<span class=\"add\">+{additions}</span> <span class=\"del\">\u{2212}{deletions}</span>"
    )
}

/// `words` after `glyph` in `class`, the glyph left out when the renderer has none.
fn marked(glyph: &str, words: &str, class: &str) -> String {
    let lead = if glyph.is_empty() { String::new() } else { format!("{} ", escape(glyph)) };
    format!("<span class=\"{class}\">{lead}{}</span>", escape(words))
}

/// `count` with its thousands set apart: `3,250`.
fn grouped(count: u64) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests;
