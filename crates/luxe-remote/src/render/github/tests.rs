use super::{Context, GithubMarks};
use crate::render::{escape, Renderer, StatusFacts, StatusSegment, Voice};

mod conversation;
mod list;
mod overview;

/// The marks as words and ages in seconds, so a test reads which mark and which clock landed
/// where. Bodies come back marked twice, `<md>R:…</md>`, to show they were made readable first.
struct Worded;

impl Renderer for Worded {
    fn markdown(&self, text: &str, _voice: Voice) -> String {
        format!("<md>{}</md>", escape(text))
    }
    fn tool_glyph(&self, _tool: &str) -> String {
        String::new()
    }
    fn status(&self, _facts: &StatusFacts) -> Vec<StatusSegment> {
        Vec::new()
    }
    fn github_marks(&self) -> GithubMarks {
        let word = |word: &str| word.to_owned();
        GithubMarks {
            pull_request: word("PR"),
            issue: word("IS"),
            yes: word("Y"),
            no: word("N"),
            waiting: word("W"),
            elision: word("…"),
        }
    }
    fn ago(&self, at: u64, now: u64) -> String {
        format!("{}s ago", now.saturating_sub(at))
    }
    fn github_readable(&self, body: &str) -> String {
        match body.trim().is_empty() {
            true => body.to_owned(),
            false => format!("R:{body}"),
        }
    }
}

/// An hour after `at`, by the app's clock.
fn an_hour_after(at: u64) -> Context<'static> {
    Context::new(&Worded, at + 3600)
}
