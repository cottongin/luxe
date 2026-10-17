//! Rows — how a unit's laid-out lines become transcript rows: a leading gap, a band, and the
//! annotations each row carries.

use super::layout::RowAnnotations;
use super::{LayoutContext, RenderedConversationLine, UnitSignature};
use crate::app::{App, ConversationItemId, ToolRunFold};
use crate::view::{blocks, wrapping, ConversationCanvasWidths, BLOCK_INSET, SAFE_GUTTER_WIDTH};
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use std::rc::Rc;

/// A collapsed run's rows: its lines wrapped and banded, with the header and footer as handles.
pub(super) fn folded_run_rows(
    app: &App,
    context: &LayoutContext,
    fold: &ToolRunFold,
    signature: &UnitSignature,
    volatile: bool,
) -> Vec<RenderedConversationLine> {
    let (styles, canvas, range) = (&app.theme_styles, context.canvas, fold.range.clone());
    let items_in_run = &app.convo[range.clone()];
    let group = blocks::tool_group_lines(
        items_in_run,
        fold.arriving,
        app.status.spinner_frame,
        canvas.wide_width,
        styles,
    );
    // A run still working has not succeeded yet, so it keeps the neutral band.
    let band = if volatile { styles.tool_call_bg } else { styles.tool_call_success_bg };
    let item_id = Rc::new(app.convo[range.start].id.clone());
    let mut content = Vec::new();
    let mut handles = Vec::new();
    for row in group {
        let wrapped = wrapping::wide_line(row.line, canvas);
        handles.extend(std::iter::repeat_n(row.handle, wrapped.len()));
        content.extend(wrapped);
    }
    let mut rows = Vec::new();
    let base = push_leading_gap(&mut rows, signature.leading_gap, range.start, &item_id);
    let banded = render_banded(
        content,
        Some(band),
        signature.selected,
        styles.selected_bg,
        canvas.available_width,
    );
    // The band adds a blank row above the content, which is part of the header's handle
    let offset_of_content = 1;
    for (offset, line) in banded.into_iter().enumerate() {
        let handle = match offset.checked_sub(offset_of_content) {
            Some(index) => handles.get(index).copied().unwrap_or(false),
            None => true,
        };
        rows.push(RenderedConversationLine {
            item_index: range.start,
            item_id: Rc::clone(&item_id),
            visual_line_within_item: base + offset,
            line: Rc::new(line),
            code_copy: None,
            image: None,
            opens_image: None,
            // The header and the footer that offers the rest open the run; the list of
            // calls between them is text, the way a card's body is.
            expands: handle,
        });
    }
    rows
}

/// One item's laid-out lines, their annotations, and the band they sit on.
pub(super) struct Banded {
    pub(super) content: Vec<Line<'static>>,
    pub(super) annotations: RowAnnotations,
    pub(super) background: Option<Style>,
}

/// An item's rows: its leading gap, then its lines banded, each carrying the annotations of the
/// content row it came from.
pub(super) fn append_banded_rows(
    app: &App,
    canvas: ConversationCanvasWidths,
    block_index: usize,
    signature: &UnitSignature,
    Banded { content, annotations, background }: Banded,
) -> Vec<RenderedConversationLine> {
    let item_id = Rc::new(app.convo[block_index].id.clone());
    let mut rows = Vec::new();
    let base = push_leading_gap(&mut rows, signature.leading_gap, block_index, &item_id);
    let banded = render_banded(
        content,
        background,
        signature.selected,
        app.theme_styles.selected_bg,
        canvas.available_width,
    );
    // render_banded prepends a blank only when a background band is drawn (never for assistant
    // prose, where code blocks live), so shift the copy flags by that offset.
    let banded_offset = usize::from(background.is_some());
    // Where an image's own cells begin: the wide canvas's indent, plus the band's inset when
    // the block draws one. The pixels are painted straight into the buffer, so this is the one
    // place that has to agree with what wrapping and banding did to the blank row beneath.
    let image_x = canvas.wide_x.saturating_sub(SAFE_GUTTER_WIDTH / 2)
        + if background.is_some() { BLOCK_INSET } else { 0 };
    for (offset, line) in banded.into_iter().enumerate() {
        let within = offset.checked_sub(banded_offset);
        let code_copy = within.and_then(|index| annotations.copies.get(index).cloned()).flatten();
        let image = within
            .and_then(|index| annotations.images.get(index).cloned())
            .flatten()
            .map(|image| (image, image_x));
        let opens_image = within.and_then(|index| annotations.opens.get(index).cloned()).flatten();
        let expands = within.is_some_and(|index| annotations.expands.get(index) == Some(&true));
        rows.push(RenderedConversationLine {
            item_index: block_index,
            item_id: Rc::clone(&item_id),
            visual_line_within_item: base + offset,
            line: Rc::new(line),
            code_copy,
            image,
            opens_image,
            expands,
        });
    }
    rows
}

/// A full-width blank line painted with `bg` — the top/bottom padding row of a backgrounded block.
fn bg_blank_line(width: u16, bg: Style) -> Line<'static> {
    Line::from(Span::raw(" ".repeat(usize::from(width)))).style(bg)
}

/// Insets a wrapped line by `inset` cells, pads it to the full width, and paints it with `bg`,
/// producing one body row of a backgrounded block. Existing spans keep their foreground.
fn banded_line(line: Line<'static>, width: u16, bg: Style, inset: u16) -> Line<'static> {
    let inset = usize::from(inset);
    let used = inset + line.width();
    let mut spans = Vec::with_capacity(line.spans.len() + 2);
    if inset > 0 {
        spans.push(Span::raw(" ".repeat(inset)));
    }
    spans.extend(line.spans);
    let target = usize::from(width);
    if target > used {
        spans.push(Span::raw(" ".repeat(target - used)));
    }
    Line::from(spans).style(bg)
}

/// Renders a block's wrapped content, optionally wrapping it in a padded background band.
///
/// A `background` turns the block into a padded band — a bg blank line above and below, each body
/// row inset and filled — so user messages and tool calls stand out from plain assistant prose.
/// Selection tints that band rather than replacing it: a selected tool card that loses its band
/// loses its shape, its inset, and every cue that says it is a card. Band-less prose takes the
/// selection as its own line background, and gains no padding — the transcript must not shift as
/// the selection moves through it.
fn render_banded(
    content: Vec<Line<'static>>,
    background: Option<Style>,
    selected: bool,
    selected_style: Style,
    width: u16,
) -> Vec<Line<'static>> {
    let Some(band) = background else {
        return match selected {
            true => content.into_iter().map(|line| line.style(selected_style)).collect(),
            false => content,
        };
    };
    let bg = if selected { band.patch(selected_style) } else { band };
    let mut out = Vec::with_capacity(content.len() + 2);
    out.push(bg_blank_line(width, bg));
    out.extend(content.into_iter().map(|line| banded_line(line, width, bg, BLOCK_INSET)));
    out.push(bg_blank_line(width, bg));
    out
}

/// Pushes a blank spacer as the leading margin of a transcript item so messages get breathing
/// room. Skipped for the first rendered item (no leading gap); tagged with the item it precedes so
/// a viewport pinned to the gap anchors to the message it introduces. Returns the starting
/// visual-line index for the item's own content (1 when a gap was added, else 0).
fn push_leading_gap(
    rows: &mut Vec<RenderedConversationLine>,
    leading_gap: bool,
    item_index: usize,
    item_id: &Rc<ConversationItemId>,
) -> usize {
    if !leading_gap {
        return 0;
    }
    rows.push(RenderedConversationLine {
        item_index,
        item_id: Rc::clone(item_id),
        visual_line_within_item: 0,
        line: Rc::new(Line::default()),
        code_copy: None,
        image: None,
        opens_image: None,
        expands: false,
    });
    1
}
