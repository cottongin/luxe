//! Tool card bodies — what a card shows under its header, and what it admits to hiding.
//!
//! The string budgeted for the *model* is the wrong thing to render: a `write` would be a wall of
//! green `+` rows, capped at twenty-odd lines, with no language to highlight and no honest account
//! of what was cut. The payload the reader wants is the call's own arguments, which the front-end
//! already has ([`luxe_core::tool_card_body`]), so a `write` shows the file it wrote and a `bash`
//! shows the command it ran. An `edit` keeps rendering its result, because only the tool knows where
//! the change landed.
//!
//! Every body goes through one policy: render logical rows, keep the first
//! [`CARD_PREVIEW_LINES`] *visual* lines, and say exactly what that left out. Counting visual lines is
//! what stops a wrapped side-by-side row from quietly turning a ten-line budget into thirty.

mod answer;
mod content;
mod output;
mod rows;
mod split;
mod unified;
mod window;

use crate::app::ToolItem;
use crate::theme::styles::ThemeStyles;
use answer::question_body;
use luxe_core::config::DiffLayout;
use output::{error_body, opened_output, strip_restated_summary, test_body};
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;
use window::{collapsed_budget, diff_window, expanded_budget, finish, Unit, Window};
#[cfg(test)]
use {crate::glyphs::Icon, luxe_core::question::QuestionOutcome};

/// Visual lines a collapsed card body occupies before the rest is folded behind the expand hint.
///
/// Ten, the same as pi and OpenCode: enough to see a change in its context, few enough that a run of
/// cards still reads as a conversation rather than a log.
const CARD_PREVIEW_LINES: usize = 10;

/// Visual lines of context a windowed diff keeps above the change.
///
/// Two, because the budget past the change is worth more: a replacement is legible from the line
/// above it, and everything after it is the rest of the change.
const CHANGE_LEAD_LINES: usize = 2;

/// Visual lines a collapsed list of search queries occupies.
///
/// Shorter than a file's, because a list of phrasings is scanned rather than read and the tool's
/// own schema asks for two to four of them. Five covers what a search actually sends and folds the
/// pathological call rather than letting it take a card's worth of rail.
const CARD_QUERY_PREVIEW_LINES: usize = 5;

/// Visual lines the output of a payload-less card occupies once opened.
///
/// Capped even when open: expanding is transcript-wide, so this multiplies across every card at once.
const OUTPUT_PREVIEW_LINES: usize = 24;

/// The cells a row's code gets once its gutter has taken its share.
///
/// Floored at one so a pathological width still renders something, and never more than the column
/// actually holds. A floor above the column is not generosity, it is a license to overflow: the
/// transcript wraps a body row that outgrew its card at a *word* boundary, which drops the leading
/// whitespace and lands the tail back at the left margin with no gutter under it. Any amount of
/// wrapping the card does itself reads better than that.
fn code_width(available: usize, gutter_width: usize) -> usize {
    available.saturating_sub(gutter_width).max(1)
}

/// One logical row's visual lines. A row may wrap; truncation counts rows, budgets visual lines.
type RowLines = Vec<Line<'static>>;

/// One laid-out row of a diff, and whether the change is in it.
///
/// The flag is set where the row is laid out — by the renderer that decided what a row even is,
/// since the side-by-side layout folds a replacement's two halves into one — so a window that has
/// to contain the change cannot drift from the renderer's idea of where the change is.
pub(super) struct DiffRow {
    pub(crate) lines: RowLines,
    /// Whether this row carries an addition or a removal.
    pub(crate) changed: bool,
}

/// What a card body needs to know about where it is being drawn.
pub(super) struct CardContext<'a> {
    pub(crate) styles: &'a ThemeStyles,
    /// Cells the body may occupy, after the card's own indent.
    pub(crate) width: u16,
    /// The reader's diff-layout preference; the width still has the final say.
    pub(crate) layout: DiffLayout,
    /// Whether this card is showing everything it holds.
    pub(crate) expanded: bool,
    /// The chord that toggles tool output, named in the truncation footer.
    pub(crate) expand_chord: Option<String>,
}

/// The summary the header shows after the duration, for tools whose result says something a green
/// wash does not. `None` leaves the header as it is.
pub(super) fn header_summary(activity: &luxe_core::ToolActivity) -> Option<String> {
    let output = activity.output_preview.as_deref()?;
    match activity.name.as_str() {
        "write" => content::write_outcome(output),
        // A `todo update`/`complete` has only an id in its arguments; the item's name is in the
        // result, and naming it is the whole point of the card.
        "todo" => content::todo_outcome(output),
        _ => None,
    }
}

/// A card body: its rows, and whether the last of them is the footer that accounts for what the
/// budget hid.
///
/// That footer offers the expand in so many words, so a click has to land on it — which means the
/// caller has to know which row it is.
pub(super) struct CardBody {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) truncated: bool,
    /// Whether the rows carry their own alignment, so the card must not inset them.
    pub(crate) flush: bool,
}

impl CardBody {
    /// A body with nothing to say.
    fn empty() -> Self {
        Self { lines: Vec::new(), truncated: false, flush: false }
    }

    /// Marks a body that lays itself out on the header's columns rather than under them.
    fn flushed(mut self) -> Self {
        self.flush = true;
        self
    }
}

/// The body of one tool card, already wrapped to `ctx.width` and ready to indent.
///
/// Empty for a collapsed card with no payload of its own, which is most of them: a `read` or a `grep`
/// says everything on its header until the reader asks for its output.
pub(super) fn body(item: &ToolItem, ctx: &CardContext<'_>) -> CardBody {
    let activity = &item.activity;
    // Opened by the reader, as opposed to opened for them because it failed
    let asked = ctx.expanded || (item.details_toggled && item.expanded);
    let expanded = ctx.expanded || item.expanded;
    let ctx = CardContext { expanded, ..borrow(ctx) };

    // A suite run through the shell reads like one run through `test`: a failing run shows its
    // verdict and what failed rather than the command, which is one keystroke away
    if activity.name == "bash" && !asked {
        let summary = activity
            .output_preview
            .as_deref()
            .and_then(luxe_core::compression::TestSummary::parse_rendered);
        if let Some(summary) = summary.filter(|summary| summary.failed > 0) {
            return test_body(&summary, &ctx);
        }
    }

    // The arguments are the payload for the tools that have one, and they are not capped by the
    // model's budget — so these are the cards that can count honestly and expand for real.
    if let Some(card_body) = &activity.card_body {
        let (language, gutter, unit) = match activity.name.as_str() {
            "write" => {
                (rows::language_for(&activity.label), content::Gutter::LineNumbers, Unit::Line)
            }
            // Queries are prose, not code, and each row is one search rather than one line — so
            // the gutter numbers them, which is also what the header's `+2 more` is counting.
            "web_search" | "websearch" => (None, content::Gutter::LineNumbers, Unit::Query),
            // A command is not a file: no numbers, and shell is the only language it can be.
            _ => (Some("sh".to_string()), content::Gutter::None, Unit::Line),
        };
        let collapsed = card_body.collapsed_lines.unwrap_or(collapsed_budget(&activity.name));
        let budget = expanded_budget(&ctx, collapsed);
        // One row past the budget, so the footer can still tell "there is more of this here" from
        // "that is all the front-end was given" without laying out a payload nobody will see.
        let window = (!ctx.expanded).then(|| budget.saturating_add(1));
        let rendered = content::render(&card_body.text, language.as_deref(), gutter, window, &ctx);
        let held = card_body.text.lines().count();
        return finish(rendered, card_body.total_lines, held, unit, Window::top(budget), &ctx);
    }

    let is_diff_tool = matches!(activity.name.as_str(), "edit" | "write");
    let succeeded = activity.state == luxe_core::ToolActivityState::Succeeded;
    let Some(output) = activity.output_preview.as_deref() else {
        return CardBody::empty();
    };
    // A question's payload is the answer, which only exists once the call has ended — so unlike
    // every other body it comes off the result rather than the arguments.
    if activity.name == "question" {
        return question_body(output, &ctx);
    }
    // A failing test run reads as its verdict and what failed, however much the runner printed; the
    // raw reply is one keystroke away for the reader who wants it. A green one stays a header.
    if activity.name == "test" && !asked {
        if let Some(summary) = luxe_core::compression::TestSummary::parse_rendered(output) {
            if summary.failed == 0 {
                return CardBody::empty();
            }
            return test_body(&summary, &ctx);
        }
    }
    if is_diff_tool && succeeded {
        return diff_body(output, &activity.label, &ctx);
    }
    // A rejected edit produced no diff, so without this the card is a bare header and the reader
    // never learns why: an exact-match miss, a stale file, a guard. Show what the tool said.
    if is_diff_tool && expanded {
        return error_body(output, activity.summary.as_deref(), &ctx);
    }
    if expanded {
        return opened_output(activity, output, &ctx);
    }
    CardBody::empty()
}

/// A `edit` result rendered as a diff, side by side where the card can seat two columns.
///
/// A collapsed card opens on the change rather than on the top of the preview. Spending the budget
/// from the top spends it on scenery whenever the change sits below it, and the reader is left
/// opening a card to find out what an `edit` edited — which is the one thing its card exists to say.
fn diff_body(output: &str, path: &str, ctx: &CardContext<'_>) -> CardBody {
    let parsed = strip_restated_summary(rows::parse(output));
    let language = rows::language_for(path);
    let rendered = match layout_for(ctx.layout, ctx.width, &parsed) {
        DiffLayout::Split => split::render(&parsed, language.as_deref(), ctx),
        _ => unified::render(&parsed, language.as_deref(), ctx),
    };
    let total = rendered.len();
    let budget = expanded_budget(ctx, CARD_PREVIEW_LINES);
    let opens_at = diff_window(&rendered, budget);
    let lines: Vec<RowLines> = rendered.into_iter().map(|row| row.lines).collect();
    if opens_at == 0 {
        return finish(lines, total, total, Unit::Line, Window::top(budget), ctx);
    }
    // The marker costs a line of the budget, and says the window — not the file — starts here.
    let window = Window { budget: budget.saturating_sub(1), opens_at };
    let mut body = finish(lines, total, total, Unit::Line, window, ctx);
    let gutter = rows::number_width(&parsed) + 2;
    body.lines.insert(
        0,
        Line::from(Span::styled(
            format!("{:>gutter$}", crate::glyphs::Mark::Elision.glyph()),
            ctx.styles.muted,
        )),
    );
    body
}

/// Whether this card draws its diff side by side.
///
/// The card's only job is measuring its own room; [`DiffLayout::splits`] holds the rule, so a card,
/// a peeked file and a review all answer the same question the same way. Measured the way the split
/// renderer lays it out, or the gate and the layout disagree about whether a column fits.
fn layout_for(preference: DiffLayout, width: u16, parsed: &[rows::Row]) -> DiffLayout {
    let code_cells = split::side_width(width).saturating_sub(rows::number_width(parsed) + 2);
    if preference.splits(code_cells, has_two_sides(parsed)) {
        DiffLayout::Split
    } else {
        DiffLayout::Unified
    }
}

/// Whether a change has both an old and a new side worth showing in columns.
fn has_two_sides(parsed: &[rows::Row]) -> bool {
    let mut removed = false;
    let mut added = false;
    for row in parsed {
        if let rows::Row::Source(source) = row {
            match source.kind {
                rows::RowKind::Removed => removed = true,
                rows::RowKind::Added => added = true,
                rows::RowKind::Context => {}
            }
        }
    }
    removed && added
}

/// What a continuation row wears in place of a gutter.
///
/// The mark plus a space, or the wrapped text butts straight against it.
pub(super) fn continuation() -> String {
    format!("{} ", crate::glyphs::Mark::Continuation)
}

/// The cells a wrapped row spends before its code: its gutter, or the continuation marker where
/// there is no gutter to hang one under.
///
/// A numbered gutter is always the wider of the two, so this only bites on a body with none — a
/// `bash` command, which is not a file and has no line numbers. Charging it anyway is what stops a
/// wrapped command from drawing two cells past its own card.
fn row_lead_width(gutter_width: usize) -> usize {
    gutter_width.max(continuation().width())
}

/// A row's gutter plus its code, wrapped so continuations align under the gutter with a `↳`.
fn wrapped_rows(
    gutter: Span<'static>,
    code: Vec<Span<'static>>,
    code_width: usize,
    gutter_width: usize,
    ctx: &CardContext<'_>,
) -> RowLines {
    let segments = crate::widgets::wrap_cells(code, code_width.max(1));
    segments
        .into_iter()
        .enumerate()
        .map(|(index, segment)| {
            let mut spans = if index == 0 {
                vec![gutter.clone()]
            } else if gutter_width == 0 {
                vec![Span::styled(continuation(), ctx.styles.dim)]
            } else {
                vec![Span::styled(format!("{:>gutter_width$}", continuation()), ctx.styles.dim)]
            };
            spans.extend(segment);
            Line::from(spans)
        })
        .collect()
}

/// Source spans, syntax-highlighted when the language is known and plain when it is not.
fn highlighted_spans(
    code: &str,
    language: Option<&str>,
    ctx: &CardContext<'_>,
) -> Vec<Span<'static>> {
    if code.is_empty() {
        return Vec::new();
    }
    let spans = crate::highlight::highlight_code(code, language, ctx.styles)
        .into_iter()
        .next()
        .map(|line| line.spans)
        .unwrap_or_default();
    if spans.is_empty() {
        vec![Span::styled(code.to_owned(), ctx.styles.text)]
    } else {
        spans
    }
}

/// A note row — the tool's own prose, muted and wrapped.
fn note_lines(text: &str, ctx: &CardContext<'_>) -> RowLines {
    crate::widgets::wrap_line(
        &Line::from(Span::styled(text.to_string(), ctx.styles.muted)),
        code_width(usize::from(ctx.width), 0),
    )
}

/// Pads a row to `width` cells, painting the whole run when it carries a wash.
///
/// Ratatui fills trailing cells with the paragraph style rather than the line's, so a wash only reads
/// as a bar if the padding carries it too. A span that brought its own background keeps it: the
/// intra-line emphasis marking what changed inside a line is the more specific signal, and a row wash
/// laid over it would erase exactly the span the reader is meant to land on.
fn pad_to(
    mut spans: Vec<Span<'static>>,
    width: usize,
    background: Option<Style>,
) -> Vec<Span<'static>> {
    if let Some(background) = background {
        for span in spans.iter_mut().filter(|span| span.style.bg.is_none()) {
            span.style = span.style.patch(background);
        }
    }
    let used: usize = spans.iter().map(|span| span.content.width()).sum();
    if used < width {
        let padding = " ".repeat(width - used);
        spans.push(match background {
            Some(background) => Span::styled(padding, background),
            None => Span::raw(padding),
        });
    }
    spans
}

/// Reborrows a context so a body can override one field without cloning the theme.
fn borrow<'a>(ctx: &CardContext<'a>) -> CardContext<'a> {
    CardContext {
        styles: ctx.styles,
        width: ctx.width,
        layout: ctx.layout,
        expanded: ctx.expanded,
        expand_chord: ctx.expand_chord.clone(),
    }
}

#[cfg(test)]
pub(super) mod test_support {
    use super::*;

    pub(crate) fn theme() -> ThemeStyles {
        let registry = luxe_core::theme::ThemeRegistry::new();
        let resolved = registry.resolve("nord").expect("a bundled theme");
        ThemeStyles::from_resolved(&resolved, "nord")
    }

    pub(crate) fn context(styles: &ThemeStyles, width: u16) -> CardContext<'_> {
        CardContext {
            styles,
            width,
            layout: DiffLayout::Auto,
            expanded: true,
            expand_chord: Some("ctrl+o".into()),
        }
    }

    pub(crate) fn plain(line: &Line<'static>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod layout_tests {
    use super::*;

    const REPLACEMENT: &str = " 11 fn main() {\n-12     old();\n+12     new();\n 13 }";

    /// Two columns are worth it when each can hold a line of code, and not before.
    #[test]
    fn auto_splits_only_when_each_column_can_hold_code() {
        let parsed = rows::parse(REPLACEMENT);
        assert_eq!(layout_for(DiffLayout::Auto, 152, &parsed), DiffLayout::Split);
        assert_eq!(layout_for(DiffLayout::Auto, 189, &parsed), DiffLayout::Split);
        assert_eq!(layout_for(DiffLayout::Auto, 84, &parsed), DiffLayout::Unified);
        assert_eq!(layout_for(DiffLayout::Auto, 60, &parsed), DiffLayout::Unified);
    }

    /// The three settings differ on how much width they ask for, and on nothing else: `unified`
    /// never pairs, `auto` wants a readable column, `split` takes a cramped one.
    #[test]
    fn each_preference_asks_for_a_different_width() {
        let parsed = rows::parse(REPLACEMENT);
        assert_eq!(layout_for(DiffLayout::Unified, 200, &parsed), DiffLayout::Unified);
        assert_eq!(layout_for(DiffLayout::Split, 200, &parsed), DiffLayout::Split);
        // A card `auto` leaves unified is one `split` still pairs up.
        assert_eq!(layout_for(DiffLayout::Auto, 84, &parsed), DiffLayout::Unified);
        assert_eq!(layout_for(DiffLayout::Split, 84, &parsed), DiffLayout::Split);
        // And below a column's worth, nobody gets two of them.
        assert_eq!(layout_for(DiffLayout::Split, 40, &parsed), DiffLayout::Unified);
    }

    /// A one-sided change in two columns is half a card of empty wash. Not a preference.
    #[test]
    fn a_one_sided_change_stays_unified() {
        let additions = rows::parse(" 11 fn main() {\n+12     added();\n 13 }");
        assert_eq!(layout_for(DiffLayout::Auto, 200, &additions), DiffLayout::Unified);
        assert_eq!(layout_for(DiffLayout::Split, 200, &additions), DiffLayout::Unified);

        let deletions = rows::parse(" 11 fn main() {\n-12     gone();\n 13 }");
        assert_eq!(layout_for(DiffLayout::Auto, 200, &deletions), DiffLayout::Unified);
    }

    /// The gutter comes out of the column, so a file with five-digit line numbers needs more room.
    ///
    /// Three more digits of number field push the cutover out by six cells of card: each side pays
    /// for the gutter, and there are two sides.
    #[test]
    fn a_wide_number_gutter_raises_the_bar() {
        let shallow = rows::parse(REPLACEMENT);
        assert_eq!(layout_for(DiffLayout::Auto, 98, &shallow), DiffLayout::Unified);
        assert_eq!(layout_for(DiffLayout::Auto, 99, &shallow), DiffLayout::Split);

        let deep = rows::parse(" 11998 fn main() {\n-11999     old();\n+11999     new();");
        assert_eq!(layout_for(DiffLayout::Auto, 104, &deep), DiffLayout::Unified);
        assert_eq!(layout_for(DiffLayout::Auto, 105, &deep), DiffLayout::Split);
    }
}
