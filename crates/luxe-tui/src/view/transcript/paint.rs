//! Paint — draws the transcript's visible rows, publishes their hits, and paints their images.

use super::layout::lay_out_transcript;
use super::selection::{highlight_columns, line_text, resolve_selection_copy};
use super::RenderedConversationLine;
use crate::app::App;
use crate::view::{anchors, conversation_canvas_widths_with_limits};
use crate::workspace_layout::gutter_inset;
use luxe_core::config::ConversationGrowth;
use ratatui::{
    layout::{Margin, Rect},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use std::ops::Range;

/// Draws the transcript into `area`: brings its rows up to date, scrolls the viewport, resolves
/// what the pointer asked of the selection, paints the visible rows and publishes their hits.
pub(in crate::view) fn render_transcript(f: &mut Frame, area: Rect, app: &mut App) {
    // Off the rule beside a dock or a preview, never off the terminal's edge
    let area = gutter_inset(area, f.area());
    let canvas = conversation_canvas_widths_with_limits(
        area.width,
        app.config.conversation.prose_max_width(),
        app.config.conversation.wide_block_max_width(),
        app.config.conversation.alignment(),
    );
    let mut inner = area.inner(Margin { horizontal: 1, vertical: 0 });
    inner.width = inner.width.min(canvas.available_width);

    // The rows survive between frames; the layout walk below rebuilds only the units that moved.
    // Taken out of the app because the walk needs the rest of it at the same time.
    let mut layout = std::mem::take(&mut app.transcript);
    let started = std::time::Instant::now();
    lay_out_transcript(&mut layout, app, canvas, inner.height);
    let rendered_lines = layout.rows();
    crate::frame_profile::note_transcript(started, app.convo.len(), rendered_lines.len());

    let window = restore_viewport(app, rendered_lines, inner);
    resolve_selection_copy(app, rendered_lines);
    let lines = visible_rows(app, rendered_lines, window.clone());
    let top_pad = paint_visible_rows(f, app, lines, rendered_lines.len(), inner);
    publish_transcript_hits(app, rendered_lines, window.clone(), inner, top_pad);
    render_transcript_scrollbar(f, app, area, usize::from(inner.height));
    paint_images(f, app, inner, rendered_lines, window.start, window.end, top_pad);
    app.transcript = layout;
}

/// Fits the scroll state to this frame's rows and returns the window of them on screen.
///
/// While not following, the viewport stays on the line it was anchored to, so a reflow above it
/// does not move what the reader is looking at.
fn restore_viewport(
    app: &mut App,
    rendered_lines: &[RenderedConversationLine],
    inner: Rect,
) -> Range<usize> {
    let total_lines = rendered_lines.len();
    let previous_anchor = app.conversation_anchor.clone();
    let viewport_height = usize::from(inner.height);
    app.scroll_state.on_resize(viewport_height);
    if app.scroll_state.total != total_lines {
        app.scroll_state.total = total_lines;
        if app.scroll_state.follow {
            app.scroll_state.to_bottom();
        }
    }
    // Semantic scroll: while not following, keep the anchored line where it was even if the
    // transcript above it reflowed (a tool card expanded, output streamed into an earlier block).
    // The restore alone would clobber a scroll the user made since the anchor was captured, so
    // re-apply that delta on top of the restored position — with unchanged content the two cancel
    // out exactly and the user's offset survives untouched.
    if !app.scroll_state.follow {
        if let Some((anchor, restored)) = previous_anchor.as_ref().and_then(|anchor| {
            anchors::restore_offset(rendered_lines, anchor, inner.y, viewport_height)
                .map(|restored| (anchor, restored))
        }) {
            let scrolled_by = app.scroll_state.offset as isize - anchor.scroll_offset as isize;
            let max_offset = rendered_lines.len().saturating_sub(viewport_height) as isize;
            app.scroll_state.offset =
                (restored as isize + scrolled_by).clamp(0, max_offset.max(0)) as usize;
        }
    }

    let window = app.scroll_state.window();
    let start = window.start.min(total_lines);
    let end = window.end.min(total_lines).max(start);
    app.conversation_anchor = anchors::capture(rendered_lines, start, inner.y);
    start..end
}

/// The window's rows as they are drawn: search matches or the selection painted over them.
fn visible_rows(
    app: &mut App,
    rendered_lines: &[RenderedConversationLine],
    window: Range<usize>,
) -> Vec<Line<'static>> {
    let start = window.start;
    let selection = app.pointer.selection.as_ref().map(|selection| selection.grid);
    // Only the window is materialized. Every frame lays out the whole transcript to know its shape,
    // but cloning all of it to paint forty rows is the kind of cost that only shows up once the
    // frame rate goes up.
    let visible = || rendered_lines[window.clone()].iter().map(|rendered| rendered.line.as_ref());
    if app.search_state.is_active() {
        app.search_state.search(rendered_lines.iter().map(|rendered| rendered.line.as_ref()));
        visible()
            .enumerate()
            .map(|(index, line)| {
                if app.search_state.matches.contains(&(start + index)) {
                    let spans = line
                        .spans
                        .iter()
                        .map(|span| {
                            Span::styled(span.content.clone(), app.theme_styles.selected_bg)
                        })
                        .collect::<Vec<_>>();
                    Line::from(spans)
                } else {
                    line.clone()
                }
            })
            .collect::<Vec<_>>()
    } else {
        match selection {
            Some(selection) => visible()
                .enumerate()
                .map(|(index, line)| {
                    match selection
                        .columns_on(start + index, crate::selection::cell_width(&line_text(line)))
                    {
                        Some(columns) => {
                            highlight_columns(line, columns, app.theme_styles.selected_bg)
                        }
                        None => line.clone(),
                    }
                })
                .collect::<Vec<_>>(),
            None => visible().cloned().collect::<Vec<_>>(),
        }
    }
}

/// Paints the rows into `inner`, padded at the top when the transcript grows from the bottom, and
/// returns that padding.
fn paint_visible_rows(
    f: &mut Frame,
    app: &App,
    visible_lines: Vec<Line<'static>>,
    total_lines: usize,
    inner: Rect,
) -> usize {
    let viewport_height = usize::from(inner.height);
    // Bottom-anchored growth: when the transcript is shorter than the viewport, pad the top so
    // content sits just above the input and grows upward (chat-style). Top growth renders as-is.
    let top_pad = match app.config.conversation.growth() {
        ConversationGrowth::Bottom => viewport_height.saturating_sub(total_lines),
        ConversationGrowth::Top | ConversationGrowth::Unknown => 0,
    };
    let visible_lines = if top_pad > 0 {
        let mut padded = Vec::with_capacity(top_pad + visible_lines.len());
        padded.extend(std::iter::repeat_with(Line::default).take(top_pad));
        padded.extend(visible_lines);
        padded
    } else {
        visible_lines
    };

    f.render_widget(Paragraph::new(visible_lines), inner);
    top_pad
}

/// Publishes each visible row's hits: its block's handle, its text, and the copy, open and image
/// affordances on top of them.
fn publish_transcript_hits(
    app: &App,
    rendered_lines: &[RenderedConversationLine],
    window: Range<usize>,
    inner: Rect,
    top_pad: usize,
) {
    let viewport_height = usize::from(inner.height);
    let start = window.start;
    // Publish a click region per visible row mapping it to its block (rendered_lines already carry
    // item_index per wrapped row, and the paragraph doesn't re-wrap, so each row is one line). A
    // click opens whatever that block is holding back; the ScrollRegionId::Conversation wheel
    // region is separate. An overlay drawn on top takes the transcript out of reach, so a press on
    // its padding can't select the message behind it.
    if !app.chat_overlay_open() {
        let mut regions = app.hit.borrow_mut();
        for (offset, rendered) in rendered_lines[window].iter().enumerate() {
            let row = top_pad + offset;
            if row >= viewport_height {
                break;
            }
            let rect = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
            if rendered.expands {
                regions
                    .push(rect, crate::mouse::MouseTarget::ConversationBlock(rendered.item_index));
            }
            // The same row is selectable text: a drag resolves cells to (line, column) here, and
            // the pin travels with it so the selection survives the next reflow.
            regions.push_text(
                rect,
                crate::mouse::TextRow {
                    surface: crate::mouse::TextSurface::Conversation,
                    line: start + offset,
                    x: inner.x,
                    first_column: 0,
                    pin: Some(crate::mouse::TextPin {
                        item: (*rendered.item_id).clone(),
                        visual_line_within_item: rendered.visual_line_within_item,
                    }),
                },
            );
            // A code block's copy affordance sits on top of its block region so the row copies
            // rather than selecting (topmost hit wins).
            if let Some(code) = &rendered.code_copy {
                regions.push(rect, crate::mouse::MouseTarget::CodeBlockCopy(code.to_string()));
            }
            // Same rule for the row that offers to open an image the terminal cannot draw.
            if let Some(key) = &rendered.opens_image {
                regions.push(rect, crate::mouse::MouseTarget::OpenImage(key.clone()));
            }
            // And for the image's own rows, which a double-click opens full-window.
            if let Some((image, _)) = &rendered.image {
                regions.push(rect, crate::mouse::MouseTarget::ConversationImage(image.key.clone()));
            }
        }
    }
}

/// The scrollbar and its track's hit, when the transcript is taller than the viewport.
fn render_transcript_scrollbar(f: &mut Frame, app: &App, area: Rect, viewport_height: usize) {
    if app.scroll_state.total > viewport_height {
        // The track is a real affordance, not decoration: publish it so a press jumps to that
        // position and a drag keeps steering (see `main_loop::route_mouse`). Pushed after the
        // per-row regions above so the one-cell lane wins over the block underneath it.
        if area.width > 0 {
            let track = Rect::new(area.right() - 1, area.y, 1, area.height);
            app.hit.borrow_mut().push(
                track,
                crate::mouse::MouseTarget::Scrollbar {
                    region: crate::mouse::ScrollRegionId::Conversation,
                    y: track.y,
                    height: track.height,
                },
            );
        }
        crate::widgets::scrollbar(
            f,
            area,
            app.scroll_state.offset,
            app.scroll_state.total,
            viewport_height,
            &app.theme_styles,
        );
    }
}

/// Paints the rows the transcript reserved for images, for whatever part of them is on screen.
///
/// After the paragraph, because these cells are pixels rather than text and nothing else should
/// draw over them; and per row, because that is what makes a half-scrolled image correct — the
/// rows above the viewport are simply not painted, and both fidelities crop to what they are
/// given rather than being told where to cut.
fn paint_images(
    frame: &mut Frame,
    app: &mut App,
    inner: Rect,
    rendered_lines: &[RenderedConversationLine],
    start: usize,
    end: usize,
    top_pad: usize,
) {
    if !app.images.tier().draws_pixels() {
        return;
    }
    // Nothing under a full-window overlay is visible, and drawing it anyway is not merely wasted:
    // the viewer wants the same image at a different size, so preparing both every frame made the
    // store release and re-transmit it twice per frame, which the terminal shows as a flicker on
    // the animation tick's cadence.
    if app.overlays.image_viewer.is_some() {
        return;
    }
    let viewport_height = usize::from(inner.height);
    let canvas = app.theme_styles.background.bg.unwrap_or(ratatui::style::Color::Reset);
    for (offset, rendered) in rendered_lines[start..end].iter().enumerate() {
        let Some((image, x)) = &rendered.image else {
            continue;
        };
        let row = top_pad + offset;
        if row >= viewport_height {
            break;
        }
        let at = (inner.x + x, inner.y + row as u16);
        app.images.paint_row(frame.buffer_mut(), at, &image.key, image.fit, image.row, canvas);
    }
    // After painting, so everything just drawn counts as seen and is never what gets evicted.
    app.images.sweep();
}
