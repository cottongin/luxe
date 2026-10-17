//! scrollbar — the one vertical scroll indicator every long surface draws.
//!
//! A thin lane in the last column of whatever rect it is handed: a muted track that recedes into
//! the chrome, and an accent thumb, whose job is to be found at a glance. No arrow caps — they are
//! two more things to draw and nothing clicks them.
//!
//! One owner, because a product with three hand-rolled bars has three of them to keep in step, and
//! the Context Dock's lists sit beside the Changes pane's while both are on screen.

use ratatui::{
    layout::Rect,
    widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame,
};

use crate::theme::styles::ThemeStyles;

/// Draws a proportional scrollbar down the last column of `area`, or nothing when it would lie.
///
/// `offset` is the first visible row, `total` the rows there are, `viewport` the rows on screen.
/// A surface that fits gets no bar: an indicator pinned full-height says "there is more" to a
/// reader who has everything.
pub fn scrollbar(
    frame: &mut Frame,
    area: Rect,
    offset: usize,
    total: usize,
    viewport: usize,
    styles: &ThemeStyles,
) {
    if area.is_empty() || viewport == 0 || total <= viewport {
        return;
    }
    let mut state = ScrollbarState::new(content_length(total, viewport))
        .position(offset.min(total.saturating_sub(viewport)))
        .viewport_content_length(viewport);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("▕"))
            .thumb_symbol("▐")
            .thumb_style(styles.accent)
            .track_style(styles.border_muted),
        area,
        &mut state,
    );
}

/// [`scrollbar`], with its lane published as a drag target so a press jumps there and a drag keeps
/// steering, for whichever surface `region` names.
///
/// Every bar takes the pointer the same way: a bar that only shows where you are is half a bar,
/// and a long file is exactly where the wheel is too slow.
#[allow(clippy::too_many_arguments)]
pub fn steered_scrollbar(
    frame: &mut Frame,
    hit: &mut crate::mouse::HitRegions,
    area: Rect,
    region: crate::mouse::ScrollRegionId,
    offset: usize,
    total: usize,
    viewport: usize,
    styles: &ThemeStyles,
) {
    if area.is_empty() || viewport == 0 || total <= viewport {
        return;
    }
    let lane = Rect::new(area.right() - 1, area.y, 1, area.height);
    hit.push(lane, crate::mouse::MouseTarget::Scrollbar { region, y: lane.y, height: lane.height });
    scrollbar(frame, area, offset, total, viewport, styles);
}

/// What [`ScrollbarState`] has to be told the content is, to park the thumb at the end of the
/// track on the last offset.
///
/// It treats its length as a number of scroll *positions*, so handing it the raw row count strands
/// the thumb a whole viewport short of the bottom — scrolling stops once the final row reaches the
/// bottom of the viewport, not the top. Report the offset range instead, and pair it with
/// `viewport_content_length` so the thumb is still sized against the content.
fn content_length(total: usize, viewport: usize) -> usize {
    total.saturating_sub(viewport) + 1
}

/// Where a thumb dragged to `row` of a `height`-tall track points, in a `0..=max` scroll range.
///
/// The thumb's own extent is deliberately ignored, so the grab point maps straight to a fraction
/// of the range — the pointer stays on the value it is pointing at, which is what makes a drag
/// feel attached.
pub fn drag_offset(row: u16, height: u16, max: usize) -> usize {
    if max == 0 || height == 0 {
        return 0;
    }
    let span = f64::from(height.saturating_sub(1)).max(1.0);
    let fraction = (f64::from(row.min(height.saturating_sub(1))) / span).clamp(0.0, 1.0);
    (fraction * max as f64).round() as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    /// The rendered lane, top to bottom.
    fn lane(offset: usize, total: usize, viewport: usize) -> Vec<String> {
        let registry = luxe_core::theme::ThemeRegistry::new();
        let resolved = registry.resolve("nord").expect("nord resolves");
        let styles = ThemeStyles::from_resolved(&resolved, "nord");
        let area = Rect::new(0, 0, 1, viewport as u16);
        let mut terminal = Terminal::new(TestBackend::new(1, viewport as u16)).expect("a terminal");
        terminal
            .draw(|frame| scrollbar(frame, area, offset, total, viewport, &styles))
            .expect("a frame");
        let buffer = terminal.backend().buffer().clone();
        (0..area.height).map(|y| buffer[(0, y)].symbol().to_string()).collect()
    }

    /// Reproduces the thumb parking short of the bottom: ratatui puts the thumb at the end of the
    /// track on `content_length - 1`, so the reported length has to be the offset range.
    #[test]
    fn the_thumb_reaches_both_ends_of_the_track() {
        let (viewport, total) = (20usize, 200usize);
        let at_bottom = lane(total - viewport, total, viewport);
        assert_eq!(
            at_bottom.last().map(String::as_str),
            Some("▐"),
            "the thumb sits against the bottom of the track at the last offset: {at_bottom:?}"
        );

        let at_top = lane(0, total, viewport);
        assert_eq!(
            at_top.first().map(String::as_str),
            Some("▐"),
            "and against the top at offset zero: {at_top:?}"
        );
        assert_eq!(
            at_top.iter().filter(|cell| *cell == "▐").count(),
            at_bottom.iter().filter(|cell| *cell == "▐").count(),
            "the thumb keeps its size across the range"
        );
    }

    /// A surface that fits gets no bar at all, rather than a full-height thumb claiming there is
    /// somewhere to go.
    #[test]
    fn content_that_fits_draws_nothing() {
        assert!(lane(0, 10, 20).iter().all(|cell| cell == " "), "{:?}", lane(0, 10, 20));
        assert!(lane(0, 20, 20).iter().all(|cell| cell == " "), "exactly full is still full");
    }

    /// An offset past the end is clamped rather than panicking or wrapping the thumb.
    #[test]
    fn an_offset_past_the_end_lands_on_the_end() {
        let stranded = lane(9_000, 200, 20);
        assert_eq!(stranded.last().map(String::as_str), Some("▐"), "{stranded:?}");
    }

    /// Dragging maps the pointer row onto the scroll range: top of the track is the top of the
    /// content, bottom is the bottom, and anything past the track stops there.
    #[test]
    fn a_drag_maps_the_track_onto_the_scroll_range() {
        assert_eq!(drag_offset(0, 20, 200), 0);
        assert_eq!(drag_offset(9, 20, 200), 95, "halfway down is halfway through");
        assert_eq!(drag_offset(19, 20, 200), 200);
        assert_eq!(drag_offset(99, 20, 200), 200, "past the track is the end");
        assert_eq!(drag_offset(5, 20, 0), 0, "content that fits does not move");
        assert_eq!(drag_offset(5, 0, 200), 0, "and neither does a track with no height");
    }
}
