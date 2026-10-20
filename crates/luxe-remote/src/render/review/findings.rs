//! Findings — the reader's comments, as cards under the hunk or chapter they're about.
//!
//! Each card is the TUI's: its label, what kind of comment it is and where it stands in the TUI's
//! words, the comment, then the thread that followed. The phone has no thread window, so the whole
//! thread is on the card. The buttons carry `data-act` for the app to send the edit; which ones a
//! card has follows what the review desk accepts in that state.

use super::super::{escape, Renderer, Voice};
use luxe_core::guided_review::{
    ChapterId, DiffSide, FindingAnchor, FindingMessageAuthor, FindingSeverity, FindingState,
    HunkId, ReviewFinding,
};

/// Cards for the comments on lines of `hunk`, or on the hunk itself.
pub(super) fn on_hunk(
    renderer: &dyn Renderer,
    findings: &[ReviewFinding],
    hunk: &HunkId,
) -> String {
    let on = |finding: &&ReviewFinding| match &finding.anchor {
        FindingAnchor::Line { hunk_id, .. } | FindingAnchor::Hunk { hunk_id, .. } => {
            hunk_id == hunk
        }
        _ => false,
    };
    findings.iter().filter(on).map(|finding| card(renderer, finding)).collect()
}

/// Cards for the comments on the chapter as a whole.
pub(super) fn on_chapter(
    renderer: &dyn Renderer,
    findings: &[ReviewFinding],
    chapter: &ChapterId,
) -> String {
    let on = |finding: &&ReviewFinding| matches!(&finding.anchor, FindingAnchor::Chapter { chapter_id } if chapter_id == chapter);
    findings.iter().filter(on).map(|finding| card(renderer, finding)).collect()
}

fn card(renderer: &dyn Renderer, finding: &ReviewFinding) -> String {
    let (state, tone) = state_words(finding);
    format!(
        "<article class=\"rv-finding rv-tone-{tone}\" data-finding=\"{}\"><p class=\"rv-finding-head\">\
         <span class=\"rv-label\">{}</span><span>{}</span><span class=\"rv-state\">{state}</span>{}\
         </p><div class=\"rv-finding-body md\">{}</div>{}{}</article>",
        escape(finding.id.as_str()),
        escape(&finding.label),
        kind(finding),
        place(&finding.anchor),
        renderer.markdown(&finding.body, Voice::Answer),
        thread(renderer, finding),
        actions(finding),
    )
}

/// What kind of comment it is, as the TUI's comment window heads it.
fn kind(finding: &ReviewFinding) -> &'static str {
    match finding.severity {
        FindingSeverity::Question => "question",
        _ if finding.github_thread.is_some() => "reviewer comment",
        _ => "comment",
    }
}

/// The line a line comment is on; the old side says so, since its number isn't the file's now.
fn place(anchor: &FindingAnchor) -> String {
    match anchor {
        FindingAnchor::Line { side: DiffSide::Old, line, .. } => {
            format!("<span>removed line {line}</span>")
        }
        FindingAnchor::Line { line, .. } => format!("<span>line {line}</span>"),
        _ => String::new(),
    }
}

/// Where it stands, in the TUI's words without its key hints, and the tone it's drawn in.
fn state_words(finding: &ReviewFinding) -> (&'static str, &'static str) {
    let last = finding.thread.last().map(|message| message.author);
    let question = finding.severity == FindingSeverity::Question;
    match finding.state {
        FindingState::Draft | FindingState::Open if finding.posted.is_some() => {
            ("on GitHub", "dim")
        }
        FindingState::Draft => ("unsent", "warning"),
        FindingState::Open if question && last == Some(FindingMessageAuthor::Agent) => {
            ("answered", "success")
        }
        FindingState::Open if last == Some(FindingMessageAuthor::System) => {
            ("no word from the agent", "warning")
        }
        FindingState::Open if last == Some(FindingMessageAuthor::Agent) => {
            ("the agent replied", "info")
        }
        FindingState::Open => ("sent \u{00b7} waiting on the agent", "dim"),
        FindingState::NeedsVerification => ("to verify", "info"),
        FindingState::Resolved => ("verified", "success"),
        FindingState::Stale => ("the code moved", "dim"),
    }
}

/// Every word after the comment, each said by who said it. The reader's opening message is the
/// comment itself, so it isn't said twice.
fn thread(renderer: &dyn Renderer, finding: &ReviewFinding) -> String {
    let said: String = finding
        .thread
        .iter()
        .skip_while(|message| {
            message.author == FindingMessageAuthor::Reviewer && message.body == finding.body
        })
        .map(|message| {
            let who = match message.author {
                FindingMessageAuthor::Reviewer => "you",
                FindingMessageAuthor::Agent => "agent",
                FindingMessageAuthor::System => "luxe",
            };
            let words = renderer.markdown(&message.body, Voice::Answer);
            format!("<div class=\"rv-said\"><p class=\"rv-who\">{who}</p><div class=\"md\">{words}</div></div>")
        })
        .collect();
    said
}

/// The edits the review desk takes in this state: a draft is sent or deleted, an open comment
/// answered or resolved, a resolved one reopened. A moved one is the TUI's to re-anchor.
fn acts(finding: &ReviewFinding) -> &'static [(&'static str, &'static str)] {
    match finding.state {
        _ if finding.posted.is_some() => &[],
        FindingState::Draft => &[("send", "Send"), ("delete", "Delete")],
        FindingState::Open | FindingState::NeedsVerification => {
            &[("reply", "Reply"), ("resolve", "Resolve")]
        }
        FindingState::Resolved => &[("reopen", "Reopen")],
        FindingState::Stale => &[],
    }
}

fn actions(finding: &ReviewFinding) -> String {
    let acts = acts(finding);
    if acts.is_empty() {
        return String::new();
    }
    let buttons: String = acts
        .iter()
        .map(|(act, words)| {
            format!("<button type=\"button\" class=\"mention\" data-act=\"{act}\">{words}</button>")
        })
        .collect();
    format!("<p class=\"rv-finding-acts\">{buttons}</p>")
}

#[cfg(test)]
mod tests;
