//! The review overview — what the change is, and everything it touches.
//!
//! The review's front page. Read before the diff, not after it: a reviewer who reaches the first
//! hunk already knowing the intent spends their attention on judgement instead of reconstruction —
//! the part of review that does not automate away. It ends with the whole change as a contents
//! page, chapter by chapter and file by file, so "where is everything" is answered here rather
//! than in a third place.
//!
//! **Short on purpose.** Labeled sections between the intent and the contents would only repeat
//! what the reader has been told: the counts are on the rail, the accounted-for evidence is printed
//! on the item it accounts for, and the harness signals ride on the file they belong to. What is
//! here is what nothing else says. Facts that are worth raising only when they are *bad* — nothing
//! ran after the last edit, a source with no record behind it, files excluded from the capture —
//! are one line each in [`notice_lines`], and silent otherwise.

mod coverage;
mod files;
mod notices;
mod pull_request;

use crate::app::App;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

pub(super) use files::file_line;
pub(crate) use files::FileOwners;
use luxe_core::guided_review::GuidedReview;
pub(super) use notices::notice_lines;
#[cfg(test)]
use pull_request::description_opening;
#[cfg(test)]
use pull_request::is_closing_reference;
pub(super) use pull_request::pull_request_facts;
use pull_request::{earlier_conversation_lines, pull_request_account};

/// Judgement items past which a review is honestly more than one sitting.
///
/// Roughly an hour at the 200-lines-an-hour rate the inspection literature settles on — the point
/// where the useful advice is "review a smaller unit", not "keep going".
const LARGE_REVIEW_JUDGEMENT_ITEMS: usize = 40;

/// The change's own intent, as the guide wrote it — or the raw record of the turn, labeled, when
/// the guide pass did not land.
///
/// The document's opening, before any chapter. Everything a reviewer wants before they read a line
/// of code and nothing they have already been told: the counts are in the head above it, and what
/// accounts for an exclusion is printed on the item it accounts for.
pub(super) fn intent_lines(app: &App, review: &GuidedReview, width: usize) -> Vec<Line<'static>> {
    let styles = &app.theme_styles;
    let mut lines: Vec<Line<'static>> = Vec::new();
    let provenance = review.provenance.as_ref();
    let pull_request = review.context.pull_request.as_ref();

    match review.brief.as_ref() {
        Some(brief) => {
            lines.extend(prose_lines(&brief.intent, app, width, " "));
            if !brief.account.is_empty() {
                lines.push(Line::default());
                for claim in &brief.account {
                    lines.extend(bullet_lines(claim, app, width));
                }
            }
        }
        None => {
            if let Some(requests) =
                provenance.map(|provenance| &provenance.requests).filter(|it| !it.is_empty())
            {
                for request in requests {
                    lines.extend(prose_lines(request, app, width, " "));
                }
                lines.push(Line::default());
            }
            if let Some(account) = provenance.and_then(|provenance| provenance.account.as_deref()) {
                lines.extend(prose_lines(account, app, width, " "));
                lines.push(Line::from(Span::styled(
                    " the agent\u{2019}s own words \u{2014} no summary was written for this review",
                    styles.dim,
                )));
            }
        }
    }

    // The author's own account: a claim to weigh against the chapters, which is why it follows the
    // narrator's reading rather than leading it
    if let Some(pr) = pull_request {
        lines.extend(pull_request_account(app, pr, width));
    }
    if let Some(conversation) = review.conversation.as_ref() {
        lines.extend(earlier_conversation_lines(app, conversation, width));
    }

    // What the reviewed commits say. Finished work has no turn to account for it, so its own
    // messages are the intent — without this a `base`/`commit` review opens on nothing. The heading
    // counts them because "the commits" invites the question of *which*: these are the ones in the
    // reviewed range, never the project's history.
    //
    // Not for a pull request: its description is the author's account, and for a one-commit pull
    // request GitHub prefills that description from the commit message, so the section repeats it
    // nearly word for word. The guide still reads both.
    if let Some(intent) = review
        .context
        .source_intent
        .as_deref()
        .filter(|it| !it.is_empty() && review.context.pull_request.is_none())
    {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            match review.context.source_commits {
                0 | 1 => "What this commit says".to_string(),
                count => format!("What these {count} commits say"),
            },
            styles.accent,
        )));
        lines.extend(prose_lines(intent, app, width, " "));
    }
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    lines
}

/// Model-written prose as wrapped, indented markdown lines.
///
/// Rendered, not printed: a summary arrives with headings, emphasis and bullets in it, and one
/// multi-line string in a single `Line` loses its newlines, so `## What was happening` runs into
/// the sentence before it.
///
/// Wrapped here rather than by the widget, because the caller counts these rows to decide what is
/// on screen: a line the paragraph wraps behind its back is a row the scroll offset does not know
/// about, and the selection drifts by one for every one of them.
pub(super) fn prose_lines(text: &str, app: &App, width: usize, indent: &str) -> Vec<Line<'static>> {
    let budget = width.saturating_sub(indent.width()).max(20);
    crate::markdown::render_markdown(text, &app.theme_styles)
        .into_iter()
        .flat_map(|line| crate::widgets::wrap_line(&line, budget))
        .map(|line| {
            let mut spans = vec![Span::raw(indent.to_owned())];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

/// One claim, bulleted, with its wrapped tail indented under the text rather than the bullet.
fn bullet_lines(text: &str, app: &App, width: usize) -> Vec<Line<'static>> {
    let mut rendered = prose_lines(text, app, width, "   ");
    if rendered.is_empty() {
        return Vec::new();
    }
    let first = rendered.remove(0);
    let mut spans = vec![Span::styled(" \u{2022} ", app.theme_styles.dim)];
    spans.extend(first.spans.into_iter().skip(1));
    let mut lines = vec![Line::from(spans)];
    lines.extend(rendered);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A long description opens with its first paragraphs and says there is more; a template
    /// heading and a bare `Fixes #n` are left for the sections that show them better.
    #[test]
    fn a_description_opens_with_its_first_paragraphs() {
        let body = format!(
            "Fixes #14404\n\n### Description\n\nThe owner casing differed.\n\n{}\n\n{}",
            "a".repeat(300),
            "b".repeat(400)
        );
        let (opening, more) = description_opening(&body);
        assert!(opening.starts_with("The owner casing differed."), "{opening}");
        assert!(opening.contains(&"a".repeat(300)));
        assert!(!opening.contains('b'), "past the budget");
        assert!(!opening.contains("Fixes") && !opening.contains("###"), "{opening}");
        assert!(more);

        assert_eq!(
            description_opening("Short and complete."),
            ("Short and complete.".into(), false)
        );
        assert_eq!(description_opening("Fixes #3"), (String::new(), false));
        let lone = "x".repeat(900);
        assert_eq!(description_opening(&lone), (lone.clone(), false), "one paragraph is never cut");
    }

    #[test]
    fn a_closing_reference_is_one_verb_and_one_issue() {
        for line in ["Fixes #12", "closes acme/app#3", "Resolves #9"] {
            assert!(is_closing_reference(line), "{line}");
        }
        for line in ["Fixes the parser", "Fixes #12 and #13", "#12", "See #12"] {
            assert!(!is_closing_reference(line), "{line}");
        }
    }
}
