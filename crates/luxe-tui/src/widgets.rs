//! TUI widgets and shared styling for UI components.

mod activity;
mod backdrop;
mod card;
mod cells;
mod fit;
mod gauge;
mod language_glyph;
mod panel;
mod provider_glyph;
mod scrollbar;
mod slab;
pub(crate) mod text_field;
mod tool_glyph;
mod wordmark;
mod wrap;

pub use activity::{activity_bar, ActivityGlyphs};
pub use backdrop::dim_backdrop;
pub use card::{card, card_inner_width};
pub(crate) use fit::{elide_path_components, truncate_middle};
pub use fit::{truncate_end, truncate_start};
pub use gauge::fill_gauge;
pub use language_glyph::{
    language_hue, language_hue_role, language_icon, LanguageHue, GENERIC_LANGUAGE,
};
pub use panel::{
    code_spans, footed_body, key_hint, key_hint_row, panel_block, panel_title, panel_title_width,
    render_footed, render_footer, split_footer, titled_panel, FOOTER_ROWS, PANEL_BORDER_TYPE,
};
pub(crate) use provider_glyph::provider_icon;
pub use scrollbar::{drag_offset, list_rows_width, list_scrollbar, scrollbar, steered_scrollbar};
pub use slab::{
    badge, fade, idle_badge, ramp, selection_block, selection_row, selection_row_on, slab, Cap,
    Fade, FADE_CELLS, SELECTION_FRAME, SELECTION_INSET, SLAB_FRAME,
};
pub use tool_glyph::{tool_glyph, tool_icon, GENERIC_TOOL};
pub use wordmark::{
    banner_frame_due, render_wordmark, smoothstep, tile_png, BannerClock, Wordmark, WordmarkInk,
    BAND, DRIFT_DEPTH, DRIFT_FRAME_TICKS, DRIFT_PERIOD_TICKS, LEAN, PEAK_DARK, PEAK_LIGHT,
    SHINE_CYCLE_TICKS, SHINE_SWEEP_TICKS, STILL_TICK,
};
pub use wrap::{wrap_cells, wrap_line, wrap_lines, wrapped_row_starts};

use ratatui::layout::Rect;

/// The fewest rows a preview can be drawn in: its border, its footer, and one line of what it says.
pub(crate) const PREVIEW_MIN_ROWS: u16 = 2 + FOOTER_ROWS + 1;

/// Where a review preview `width` × `rows` sits in `area`, a row clear above and below; or, when
/// not even [`PREVIEW_MIN_ROWS`] fit, `None` with a bounded notice drawn in its place.
///
/// `min_rows` is a preference, raised to where there is room and lowered to fit where there is
/// not. The review records which happened, because a preview its notice stands in for must take
/// no key but esc: a confirm the reader cannot see is not one.
pub(crate) fn fit_review_preview(
    frame: &mut ratatui::Frame,
    area: Rect,
    (width, rows, min_rows): (u16, u16, u16),
    app: &crate::app::App,
) -> Option<Rect> {
    let fit = fit_preview(frame, area, (width, rows, min_rows), false, &app.theme_styles);
    app.guided_review_workspace.settle_preview(fit.is_some());
    fit
}

/// [`fit_review_preview`] for a dialog whose rows are all choices: it is drawn whole or not at
/// all, since a card cut short hides options the keys still act on.
pub(crate) fn fit_whole_review_preview(
    frame: &mut ratatui::Frame,
    area: Rect,
    (width, rows): (u16, u16),
    app: &crate::app::App,
) -> Option<Rect> {
    let fit = fit_preview(frame, area, (width, rows, rows), true, &app.theme_styles);
    app.guided_review_workspace.settle_preview(fit.is_some());
    fit
}

/// [`fit_review_preview`]'s layout and notice, without the review's guard.
fn fit_preview(
    frame: &mut ratatui::Frame,
    area: Rect,
    (width, rows, min_rows): (u16, u16, u16),
    whole: bool,
    styles: &crate::theme::styles::ThemeStyles,
) -> Option<Rect> {
    let room = area.height.saturating_sub(2);
    if room >= PREVIEW_MIN_ROWS && !(whole && rows > room) {
        return Some(centered_rect(width, rows.max(min_rows).min(room), area));
    }
    use ratatui::text::{Line, Span};
    let notice = centered_rect(40, 2, area);
    frame.render_widget(ratatui::widgets::Clear, notice);
    frame.render_widget(
        ratatui::widgets::Paragraph::new(vec![
            Line::from(Span::styled("Make the window bigger to see this", styles.error)),
            Line::from(Span::styled("Only esc acts until it fits", styles.muted)),
        ])
        .alignment(ratatui::layout::Alignment::Center),
        notice,
    );
    None
}

/// A `width` × `height` box in the middle of `area`, clamped to it.
///
/// Every centered surface measures its content and then asks for this, so "in the middle" means the
/// same thing on the dashboard, the Configuration surface, and anything that joins them.
pub fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}
