//! Card answers — a question card's answers, laid out in columns.

use super::window::{expanded_budget, finish, per_visual_line, Unit, Window};
use super::{code_width, rows, CardBody, CardContext, RowLines, CARD_PREVIEW_LINES};
use crate::glyphs::Icon;
use luxe_core::question::QuestionOutcome;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

/// What the reader decided, beside the question that asked them.
///
/// The modal is gone by the time anyone scrolls back to this card, so without the answer here the
/// only trace of the decision is a green band and the question it was an answer to. It is the other
/// half of an exchange rather than content the card is previewing, so it is laid out as a *peer* of
/// the header: `answer` under the tool's name, what was answered under the question. Labeled and
/// marked rather than merely listed, because a bare list under a question reads as the options it
/// offered.
pub(super) fn question_body(output: &str, ctx: &CardContext<'_>) -> CardBody {
    let Some(outcome) = QuestionOutcome::parse(output) else {
        return CardBody::empty();
    };
    let column = super::super::blocks::header_subject_column("question");
    let answers = outcome.answers();
    if answers.is_empty() {
        let note = match outcome {
            QuestionOutcome::Dismissed => "dismissed without answering",
            _ => "submitted with no answer",
        };
        return CardBody {
            lines: unanswered_note(note, column, ctx),
            truncated: false,
            flush: true,
        };
    }
    let mut rendered: Vec<RowLines> = Vec::new();
    // Nobody was there to be asked, so the card must not credit the reader with the choice.
    if matches!(outcome, QuestionOutcome::Unattended { .. }) {
        let note = "nobody to ask · answered from the default";
        rendered.extend(per_visual_line(unanswered_note(note, column, ctx)));
    }
    for answer in answers {
        rendered.extend(per_visual_line(answer_lines(answer, column, ctx)));
    }
    let total = rendered.len();
    let budget = expanded_budget(ctx, CARD_PREVIEW_LINES);
    let mut body = finish(rendered, total, total, Unit::Line, Window::top(budget), ctx).flushed();
    // The footer is card chrome rather than part of the exchange, so it sits where every other
    // card's footer sits instead of following the rows it is counting.
    if body.truncated {
        if let Some(footer) = body.lines.last_mut() {
            footer.spans.insert(0, Span::raw(super::super::blocks::BODY_INDENT));
        }
    }
    body
}

/// A card's own note about an ending, under the question it is about.
///
/// The label column stays empty on purpose: there is no answer, so there is nothing to label, and a
/// tick beside a dismissal would credit the reader with a choice they declined to make.
fn unanswered_note(note: &str, column: usize, ctx: &CardContext<'_>) -> RowLines {
    column_rows(Vec::new(), note, ctx.styles.muted, column, ctx)
}

/// One answer, labeled and ticked in the header's name column and wrapped under its question.
///
/// In the accent a user turn is drawn in, because that is what it is: the reader's own words, in the
/// transcript's voice for them, set against the model's question in the line above. The label beside
/// it carries the meaning, so a theme whose accent sits close to its own success wash costs the row
/// shine rather than sense.
fn answer_lines(answer: &str, column: usize, ctx: &CardContext<'_>) -> RowLines {
    let label = Span::styled(
        crate::glyphs::icon_label(Icon::Answer.glyph(), "answer"),
        ctx.styles.success.add_modifier(Modifier::BOLD),
    );
    column_rows(vec![label], answer, ctx.styles.accent, column, ctx)
}

/// A row laid out on the header's columns: `lead` in the name column, `text` under the subject.
///
/// Wrapping keeps the text column, so a long answer reads as one block rather than stepping back
/// to the margin on its second line.
fn column_rows(
    lead: Vec<Span<'static>>,
    text: &str,
    style: Style,
    column: usize,
    ctx: &CardContext<'_>,
) -> RowLines {
    let lead_width: usize = lead.iter().map(|span| span.content.width()).sum();
    let width = code_width(usize::from(ctx.width), column);
    let wrapped =
        crate::widgets::wrap_line(&Line::from(Span::styled(rows::sanitize(text), style)), width);
    wrapped
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let mut spans = if index == 0 {
                let mut spans = lead.clone();
                spans.push(Span::raw(" ".repeat(column.saturating_sub(lead_width))));
                spans
            } else {
                vec![Span::raw(" ".repeat(column))]
            };
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}
