//! Layout — brings the transcript's cached rows up to date with the conversation.

use super::reveal::{rendered_chars, revealed_lines};
use super::rows::{append_banded_rows, folded_run_rows, Banded};
use super::{LayoutContext, TranscriptLayout, UnitSignature};
use crate::app::{App, ToolRunFold};
use crate::view::{blocks, wrapping, ConversationCanvasWidths};
use ratatui::text::Line;
use std::rc::Rc;

/// Rows an image is never scaled below, however short the terminal is.
///
/// Under this it stops being a picture and becomes a smear, at which point the reader is better
/// served by scrolling to a bigger one than by a thumbnail that says nothing.
pub(in crate::view) const MIN_IMAGE_ROWS: u16 = 6;

/// How much of the transcript's height one image may take.
///
/// Half, so there is always as much conversation on screen as picture. The configured cap is a
/// ceiling rather than a size: it is right for a full-height window and would swallow a short one,
/// and a terminal split four ways is exactly where a screenshot is most likely to be reviewed.
pub(in crate::view) fn image_row_cap(configured: u16, viewport_height: u16) -> u16 {
    configured.min((viewport_height / 2).max(MIN_IMAGE_ROWS)).max(1)
}

/// Brings the transcript's cached rows up to date with the conversation.
///
/// Walks the items in order against [`TranscriptLayout`]'s existing units, reusing every unit
/// whose [`UnitSignature`] still matches and laying out only the ones that moved. On an idle frame
/// that is nothing at all; while a turn streams it is the tail.
pub(in crate::view) fn lay_out_transcript(
    layout: &mut TranscriptLayout,
    app: &mut App,
    canvas: ConversationCanvasWidths,
    viewport_height: u16,
) {
    let context = layout_context(app, canvas, viewport_height);
    layout.begin(&context);

    let group_runs = app.tool_run_folds();
    // The reveal is consulted per item while the render path still holds borrows of the app, so it
    // travels through the frame as a copy and goes back at the end.
    let mut walk = RevealWalk {
        reveal: app.reveal,
        revealing: app.reveal_enabled(),
        growing_tail: app.tail_still_streaming(),
        items: app.convo.len(),
        now: app.frame_now(),
    };
    // Scrolled off the bottom, the animation isn't even on screen — holding text back there would
    // just be text the user cannot see, and returning to the bottom would then jump.
    if !app.scroll_state.follow {
        walk.reveal.catch_up();
    }

    let mut fold_index = 0;
    let mut block_index = 0;
    while block_index < app.convo.len() {
        // The folds arrive in transcript order and so does this walk, so the next one to consider
        // is always the next one along — searching the list per item made a session with many runs
        // quadratic in its own history.
        while group_runs.get(fold_index).is_some_and(|fold| fold.range.start < block_index) {
            fold_index += 1;
        }
        let group = group_runs
            .get(fold_index)
            .filter(|fold| fold.range.start == block_index)
            .filter(|fold| !app.tool_group_is_expanded(&fold.range));
        let next = match group {
            Some(fold) => layout_folded_run(layout, app, &context, &mut walk, fold),
            None => layout_item(layout, app, &context, &mut walk, block_index),
        };
        // Nothing from a hidden unit on has had its turn yet
        let Some(next) = next else { break };
        block_index = next;
    }
    layout.finish();
    app.reveal = walk.reveal;
}

/// The reveal's state for one walk of the transcript.
struct RevealWalk {
    reveal: crate::reveal::Reveal,
    revealing: bool,
    growing_tail: bool,
    items: usize,
    now: std::time::Instant,
}

/// The frame-wide inputs every unit is laid out against.
fn layout_context(
    app: &App,
    canvas: ConversationCanvasWidths,
    viewport_height: u16,
) -> LayoutContext {
    // A tool card's body is laid out against a real width — two diff columns are only worth it if
    // the card can seat them — so the canvas travels into the block renderer rather than being
    // applied to its output afterwards. The wide canvas minus the body's own indent is what a card
    // body has.
    let card_width =
        canvas.wide_width.saturating_sub(u16::try_from(blocks::BODY_INDENT.len()).unwrap_or(0));
    let image_max_rows = image_row_cap(app.config.conversation.image_max_rows(), viewport_height);
    LayoutContext {
        canvas,
        card_width,
        image_bounds: (canvas.wide_width, image_max_rows),
        diff_layout: app.config.diff.layout(),
        tool_output_expanded: app.tool_output_expanded,
        expand_chord: app
            .keymap
            .chords_for(luxe_core::keymap::UiAction::ToggleToolOutput)
            .and_then(|chords| chords.first())
            .map(ToString::to_string),
        styles: app.theme_styles.clone(),
        glyphs: crate::glyphs::tier(),
    }
}

/// Lays out a collapsed run of tool calls as one unit, or reuses it, and returns the index after
/// it — `None` when the reveal has not reached it yet.
fn layout_folded_run(
    layout: &mut TranscriptLayout,
    app: &App,
    context: &LayoutContext,
    walk: &mut RevealWalk,
    fold: &ToolRunFold,
) -> Option<usize> {
    let range = fold.range.clone();
    // A collapsed run draws as one line but is still several items, so the cursor has to be
    // walked across all of them or it would be left pointing inside a run already drawn.
    if walk.revealing {
        let mut hidden = false;
        for index in range.clone() {
            let item = crate::reveal::Item {
                index,
                items: walk.items,
                chars: 0,
                animated: false,
                growing: false,
            };
            hidden |= walk.reveal.show(walk.now, item) == crate::reveal::Show::Hidden;
        }
        if hidden {
            return None;
        }
    }
    let signature = UnitSignature {
        first: range.start,
        len: range.len(),
        revision: newest_revision(app, range.clone()),
        leading_gap: !layout.at_start(),
        folded: true,
        arriving: fold.arriving,
        selected: app.selected_block.is_some_and(|index| range.contains(&index)),
        never_sent: false,
        expanded: false,
    };
    // A run that is still working spins, so its line is different every tick and
    // there is nothing about it a later frame could reuse.
    let volatile = blocks::run_is_working(&app.convo[range.clone()], fold.arriving);
    if (!volatile).then(|| layout.reusable(&signature)).flatten().is_some() {
        layout.keep();
        return Some(range.end);
    }
    let rows = folded_run_rows(app, context, fold, &signature, volatile);
    layout.replace(signature, rows, 0, !volatile);
    Some(range.end)
}

/// Lays out one conversation item, or reuses it, and returns the index after it — `None` when the
/// reveal has not reached it yet.
fn layout_item(
    layout: &mut TranscriptLayout,
    app: &App,
    context: &LayoutContext,
    walk: &mut RevealWalk,
    block_index: usize,
) -> Option<usize> {
    let styles = &app.theme_styles;
    let item = &app.convo[block_index];
    let item_id = &item.id;
    let signature = UnitSignature {
        first: block_index,
        len: 1,
        revision: item.revision(),
        leading_gap: !layout.at_start(),
        folded: false,
        arriving: false,
        selected: app.selected_block == Some(block_index),
        never_sent: app.was_never_sent(item_id),
        expanded: app.message_is_expanded(item_id),
    };
    // Anything that redraws on a clock of its own — a spinner, an image the store is still
    // sizing — has nothing worth keeping between frames.
    let volatile = draws_by_itself(app, item);
    let cached_chars = (!volatile).then(|| layout.reusable(&signature)).flatten();

    // The reveal decides how much of an item to draw from its *rendered* character count, and
    // wants that before the layout. A cached unit remembers its own; anything else has to be
    // laid out to be counted, which is the work the memo exists to skip.
    let mut laid = cached_chars.is_none().then(|| lay_out_block(app, block_index, context));
    let chars = match (cached_chars, &laid) {
        (Some(chars), _) => chars,
        (None, Some((content, _))) => rendered_chars(content),
        // Unreachable: the layout above runs exactly when the memo had no count.
        (None, None) => 0,
    };
    let show = if walk.revealing {
        walk.reveal.show(
            walk.now,
            crate::reveal::Item {
                index: block_index,
                items: walk.items,
                chars,
                animated: blocks::animates_reveal(item.block()),
                growing: block_index + 1 == walk.items && walk.growing_tail,
            },
        )
    } else {
        crate::reveal::Show::Whole
    };
    // Nothing from here on has had its turn yet.
    if show == crate::reveal::Show::Hidden {
        return None;
    }
    let partial = matches!(show, crate::reveal::Show::Partial(_));
    if cached_chars.is_some() && !partial {
        layout.keep();
        return Some(block_index + 1);
    }
    // A cached unit the reveal has started typing out is the one case that has to be laid out
    // after all: the memo holds the finished item, and only the layout can produce a prefix.
    let (mut content, mut annotations) =
        laid.take().unwrap_or_else(|| lay_out_block(app, block_index, context));

    let background = blocks::background(app.convo[block_index].block(), styles);
    if let crate::reveal::Show::Partial(shown) = show {
        let band = background.unwrap_or(styles.background);
        content = revealed_lines(content, shown, styles, band, walk.reveal.settle());
        annotations.truncate(content.len());
    }
    // Skip whitespace-only blocks (e.g. an assistant text block that held only "\n\n" before a
    // tool call) so they don't leave a large empty gap. An image's rows are deliberately blank
    // — they are reserved for pixels painted after the paragraph — so they are not that.
    if content.iter().all(line_is_blank) && annotations.images.iter().all(Option::is_none) {
        layout.replace(signature, Vec::new(), chars, !volatile && !partial);
        return Some(block_index + 1);
    }
    let banded = Banded { content, annotations, background };
    let rows = append_banded_rows(app, context.canvas, block_index, &signature, banded);
    layout.replace(signature, rows, chars, !volatile && !partial);
    Some(block_index + 1)
}

/// The per-row facts wrapping would otherwise drop: which rows are affordances, and which reserve
/// pixels. Parallel to the laid-out lines, one entry per row.
#[derive(Default)]
pub(super) struct RowAnnotations {
    pub(super) copies: Vec<Option<Rc<str>>>,
    pub(super) expands: Vec<bool>,
    pub(super) images: Vec<Option<crate::markdown::ImageRow>>,
    pub(super) opens: Vec<Option<crate::graphics::ImageKey>>,
}

impl RowAnnotations {
    fn truncate(&mut self, rows: usize) {
        self.copies.truncate(rows);
        self.expands.truncate(rows);
        self.images.truncate(rows);
        self.opens.truncate(rows);
    }
}

/// Renders one conversation block to wrapped lines, with the annotations wrapping would lose.
fn lay_out_block(
    app: &App,
    block_index: usize,
    context: &LayoutContext,
) -> (Vec<Line<'static>>, RowAnnotations) {
    let block_context = blocks::BlockContext {
        styles: &app.theme_styles,
        spinner_frame: app.status.spinner_frame,
        waiting_on_user: app.status.waiting_since.is_some(),
        never_sent: app.was_never_sent(&app.convo[block_index].id),
        expanded: app.message_is_expanded(&app.convo[block_index].id),
        images: &app.images,
        tool_images: &app.tool_images,
        image_bounds: context.image_bounds,
        card_width: context.card_width,
        prose_width: context.canvas.prose_width,
        diff_layout: context.diff_layout,
        tool_output_expanded: context.tool_output_expanded,
        expand_chord: context.expand_chord.clone(),
        running_ms: app
            .status
            .tool_elapsed(app.frame_now())
            .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)),
    };
    let mut content = Vec::new();
    let mut annotations = RowAnnotations::default();
    for layout in blocks::lines(app.convo[block_index].block(), &block_context) {
        let code = layout.code_copy.clone().map(Rc::<str>::from);
        let image = layout.image.clone();
        let opens = layout.opens_image.clone();
        let expands = layout.expands;
        for (index, line) in
            wrapping::conversation_line(layout, context.canvas).into_iter().enumerate()
        {
            content.push(line);
            // The affordance is a single non-wrapping line; tag only its first row. A handle that
            // wraps stays clickable across every row it occupies.
            annotations.copies.push(if index == 0 { code.clone() } else { None });
            // An image row is blank and exactly one row, so it never wraps into a second.
            annotations.images.push(if index == 0 { image.clone() } else { None });
            annotations.opens.push(opens.clone());
            annotations.expands.push(expands);
        }
    }
    (content, annotations)
}

/// The highest revision among the items in `range` — see [`UnitSignature::revision`].
fn newest_revision(app: &App, range: std::ops::Range<usize>) -> crate::app::BlockRevision {
    app.convo[range]
        .iter()
        .map(crate::app::ConversationItem::revision)
        .max()
        .unwrap_or(crate::app::BlockRevision::ZERO)
}

/// Whether a block's own drawing moves without the transcript changing.
///
/// A working card spins, and an image is laid out against a store that may still be loading or
/// resizing it. Those units are rebuilt every frame, because there is nothing about them a later
/// frame could honestly reuse.
///
/// **Working**, not running: a call the model is still writing spins too, and the memo is keyed on
/// the card's revision — so a provider that delivers a tool call's arguments in one lump at the end
/// left the spinner frozen for the whole time it took to arrive, which reads worse than no spinner
/// at all.
fn draws_by_itself(app: &App, item: &crate::app::ConversationItem) -> bool {
    match item.block() {
        crate::app::ConvoBlock::Image { .. } => true,
        crate::app::ConvoBlock::Tool(tool) => {
            tool.activity.state.is_working()
                || app.tool_images.contains_key(&(
                    tool.activity.session_id.clone(),
                    tool.activity.call_id.clone(),
                ))
        }
        _ => false,
    }
}

/// True when a rendered line carries no visible content.
fn line_is_blank(line: &Line<'static>) -> bool {
    line.spans.iter().all(|span| span.content.trim().is_empty())
}
