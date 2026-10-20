//! View — ratatui rendering for luxe's TUI surfaces.
//!
//! [`render`] routes a frame to the surface the app is on. The chat frame is measured first
//! (`frame_layout`), then drawn back to front: the dock and the conversation, the composer and
//! the strips above it, and whatever is open over them.

mod activity;
mod anchors;
mod blocks;
mod canvas;
pub(crate) mod changes;
mod context_report;
mod diff_lines;
mod divider;
mod dock;
pub(crate) use dock::{external_place_folded, status_mark};
mod emphasis;
mod frame_layout;
pub(crate) mod guided_review;
mod input;
mod pending;
mod surfaces;
mod tool_card;
pub(crate) mod transcript;
pub(crate) mod wrapping;

pub use canvas::{
    conversation_canvas_widths, conversation_canvas_widths_with_limits, peek_diff_column,
    peek_diff_width, peek_width_bounds, ConversationCanvasWidths,
};
use canvas::{BLOCK_INSET, SAFE_GUTTER_WIDTH};
use frame_layout::ChatRegions;

use crate::app::{App, ViewMode};
use crate::workspace_dock::DockPresentation;
use crate::workspace_layout::{self, gutter_inset};
use luxe_core::config::DockPosition;
use ratatui::{
    layout::Rect,
    widgets::{Block, Borders},
    Frame,
};

/// Draws one frame and hands the terminal whatever that frame owed it.
///
/// The single entry point every renderer uses, because the debt is easy to forget and invisible
/// when you do: the transcript writes placeholder cells for an image the terminal has never been
/// sent, and a correct-looking layout comes up empty.
pub fn draw(terminal: &mut crate::terminal::Tui, app: &mut App) -> std::io::Result<()> {
    crate::terminal::draw_synchronized(terminal, |frame| render(frame, app))?;
    crate::terminal::write_graphics(&app.images.drain_control())
}

/// Renders the current application surface.
pub fn render(f: &mut Frame, app: &mut App) {
    let size = f.area();
    // Start each frame with an empty mouse hit registry; surfaces republish their regions below.
    app.hit.borrow_mut().clear();
    f.render_widget(Block::default().style(app.theme_styles.background), size);

    if app.current_view == ViewMode::Dashboard {
        surfaces::render_dashboard(f, size, app);
        // Pickers a command or the prompt opens sit over the start screen and hand it back, the
        // draft still in the prompt
        if let Some(ref picker) = app.overlays.model_picker {
            picker.view(f, size, &app.theme_styles, &app.hit);
        }
        if let Some(ref picker) = app.overlays.thinking_picker {
            picker.view(f, size, &app.theme_styles, &app.hit);
        }
        if let Some(ref picker) = app.overlays.theme_picker {
            picker.view(f, size, &app.theme_styles, &app.hit);
        }
        if let Some(ref files) = app.overlays.context_files {
            crate::context_files::render(f, size, files, &app.theme_styles, &app.hit);
        }
        return;
    }

    if app.current_view == ViewMode::Configuration {
        surfaces::render_configuration(f, size, app);
        return;
    }

    if app.current_view == ViewMode::SessionPicker && app.dashboard.is_some() {
        surfaces::render_dashboard(f, size, app);
        if let Some(ref picker) = app.overlays.session_picker {
            picker.view(f, size, &app.theme_styles, &app.hit);
        }
        return;
    }

    if app.current_view == ViewMode::GuidedReview {
        guided_review::render(f, size, app);
        // The palette is the one overlay a review draws over itself: it is reachable from anywhere,
        // and it lists only what a review can run
        if let Some(ref palette) = app.overlays.palette {
            crate::palette::render_palette(f, size, palette, &app.theme_styles, &app.hit);
        }
        return;
    }

    let regions = frame_layout::build_chat_regions(app, size);
    render_dock_and_conversation(f, app, &regions, size);
    let composer_area = render_composer_surfaces(f, app, &regions, size);
    // `/sessions` mid-session: over the conversation, and under any prompt the turn raises, which
    // takes the keys first. It is modal, so nothing the conversation published answers the pointer
    if app.current_view == ViewMode::SessionPicker {
        if let Some(ref picker) = app.overlays.session_picker {
            app.hit.borrow_mut().clear();
            picker.view(f, size, &app.theme_styles, &app.hit);
        }
    }
    render_chat_overlays(f, app, &regions, composer_area);
}

/// Draws the dock in whichever presentation it has, and the conversation beside or under it, and
/// publishes the transcript's wheel region.
fn render_dock_and_conversation(f: &mut Frame, app: &mut App, regions: &ChatRegions, size: Rect) {
    let &ChatRegions {
        presentation: dock_presentation,
        statusline_visible,
        separator,
        dock_column,
        stack,
        diff_column,
        conversation_area,
        ..
    } = regions;
    // Overlay/replacement docks fill everything above the statusline row.
    let dock_content = stack;

    // The full-height sidebar always carries its own MODEL/telemetry footer — its visibility is
    // decoupled from the statusline, so a forced-on statusline (`/statusline show`) can sit
    // beside the sidebar without emptying its footer. Overlay/replacement docks coexist with the
    // statusline (which keeps carrying telemetry), so they defer to it to avoid showing the same
    // values twice at once. In `auto` the statusline hides beside the sidebar, so you still see
    // exactly one copy by default.
    let show_telemetry = match dock_presentation {
        DockPresentation::SideBySide => true,
        _ => !statusline_visible,
    };
    // Where the transcript actually landed, which is what the wheel scrolls. With a diff taking a
    // column of its own the transcript is only part of the main column, and with the diff alone it
    // is nowhere.
    let mut transcript_area = None;
    match dock_presentation {
        DockPresentation::SideBySide => {
            transcript_area = render_conversation(f, conversation_area, diff_column, app);
            if let (Some(separator), Some(dock_rect)) = (separator, dock_column) {
                let on_left = app.dock.position != DockPosition::Right;
                let border = if on_left { Borders::RIGHT } else { Borders::LEFT };
                let (min, max) = workspace_layout::dock_width_bounds(size.width);
                let handle = crate::mouse::DividerHandle {
                    pane: workspace_layout::Pane::Dock,
                    anchor: if on_left { dock_rect.x } else { dock_rect.right() },
                    pane_on_left: on_left,
                    min,
                    max,
                };
                divider::render(f, separator, border, separator, handle, app);
                dock::render(f, dock_rect, app, show_telemetry);
            }
        }
        DockPresentation::Replacement => {
            dock::render(f, dock_content, app, show_telemetry);
        }
        DockPresentation::Overlay => {
            transcript_area = render_conversation(f, conversation_area, diff_column, app);
            f.buffer_mut().set_style(stack, app.theme_styles.dim);
            let dock_width = workspace_layout::resolve_workspace_layout_with(
                size.width,
                app.panes.width(workspace_layout::Pane::Dock),
            )
            .dock_width;
            let overlay =
                frame_layout::overlay_rect(conversation_area, app.dock.position, dock_width);
            dock::render(f, overlay, app, show_telemetry);
        }
        DockPresentation::Hidden => {
            transcript_area = render_conversation(f, conversation_area, diff_column, app);
        }
    }
    // The transcript is the wheel-scroll target over the rows it occupies, and only those: pushed
    // over the whole main column it swallowed a peeked diff's own region, so the wheel over a diff
    // scrolled the transcript behind it.
    if let Some(rect) =
        transcript_area.filter(|_| dock_presentation != DockPresentation::Replacement)
    {
        app.hit.borrow_mut().push_scroll(rect, crate::mouse::ScrollRegionId::Conversation);
    }
}

/// Draws the composer (or the search field), the tray of pending submissions, the activity row and
/// the statusline, and returns where the composer went.
fn render_composer_surfaces(
    f: &mut Frame,
    app: &mut App,
    regions: &ChatRegions,
    size: Rect,
) -> Rect {
    let &ChatRegions {
        presentation: dock_presentation,
        statusline_visible,
        statusline_column,
        activity_area,
        tray_area,
        composer_column,
        ..
    } = regions;
    let composer_layout = &regions.composer_layout;
    // Inset the composer (and the strips above it) the way the conversation and dock are, so the
    // prompt box lines up with them. The inset lives with `input::height`, which measures the wrap
    // column through the same helper.
    let composer_area = input::composer_area(composer_column, size);
    if dock_presentation != DockPresentation::Replacement {
        match composer_layout {
            Some(layout) => input::composer(f, composer_area, app, layout),
            None => input::search(f, composer_area, &app.search_state, &app.theme_styles),
        }
        pending::render(f, gutter_inset(tray_area, size), app);
    }
    // The live activity indicator belongs to the conversation column: it is the bottom of what the
    // agent is saying, not a sidebar statistic, so it sits in the row directly above the composer
    // in every layout that shows one. Replacement has no conversation column, so its dock footer
    // stays the runtime surface there.
    if dock_presentation != DockPresentation::Replacement {
        let indicator = gutter_inset(
            Rect { y: activity_area.bottom().saturating_sub(1), height: 1, ..activity_area },
            size,
        );
        if !app.search_state.is_active() {
            activity::render_composer_activity(f, indicator, app);
        }
        // The clipboard is invisible, so a copy has to say it happened. It goes at the right end of
        // the same row, where the eye already is on its way to the prompt, and out of the activity
        // bar's way — which grows from the left.
        activity::render_toast(f, indicator, app);
    }
    if statusline_visible {
        // Share the composer's left/right margin, so the flat bar lines up with the prompt box
        let statusline_area = gutter_inset(statusline_column, size);
        crate::statusline::render(f, statusline_area, app);
    }
    composer_area
}

/// Draws what opens over the chat: the context menu, the full-window viewers and panels, the
/// pickers, the login wizard, and whichever panel grows up from the composer.
fn render_chat_overlays(f: &mut Frame, app: &mut App, regions: &ChatRegions, composer_area: Rect) {
    render_covering_overlays(f, app);
    render_composer_panel(f, app, regions, composer_area);
}

/// The overlays that sit over the conversation rather than growing out of the composer.
fn render_covering_overlays(f: &mut Frame, app: &mut App) {
    let size = f.area();
    // Over everything it was opened on, and under nothing: it is answering the click that opened it
    if let Some(menu) = app.overlays.context_menu.as_ref() {
        let hovered = match app.pointer.hover {
            Some(crate::mouse::MouseTarget::ContextMenuItem(index)) => Some(index),
            _ => None,
        };
        crate::context_menu::render(f, menu, hovered, &app.theme_styles, &app.hit);
    }

    // Above the transcript and below every input surface: it is a look at something already on
    // screen, not a thing to interact with.
    if let Some(key) = app.overlays.image_viewer.clone() {
        let styles = app.theme_styles.clone();
        crate::image_viewer::render(f, size, &key, &mut app.images, &styles);
    }
    if app.overlays.keys_card {
        crate::keys_card::render(
            f,
            size,
            crate::keys_card::CHAT_TITLE,
            &crate::keys_card::chat_groups(&app.keymap),
            " esc close ",
            Some(crate::keys_card::DOCS),
            &app.theme_styles,
        );
    }
    // `/github` and `/pr` cover the conversation: each is a place you went, not a strip above
    // the prompt
    if let Some(ref panel) = app.overlays.github {
        crate::github_panel::render(f, size, panel, &app.theme_styles, app.status.spinner_frame);
    }
    if let Some(ref card) = app.overlays.pull_request_card {
        let frame = app.status.spinner_frame;
        crate::github_detail::render_card(f, size, card, frame, &app.theme_styles);
    }
    if let Some(ref palette) = app.overlays.palette {
        crate::palette::render_palette(f, size, palette, &app.theme_styles, &app.hit);
    }
    if let Some(ref picker) = app.overlays.model_picker {
        picker.view(f, size, &app.theme_styles, &app.hit);
    }
    // The login wizard sits above every other overlay: it owns input while it's open.
    surfaces::render_auth_wizard(f, size, app);
    if let Some(ref picker) = app.overlays.thinking_picker {
        picker.view(f, size, &app.theme_styles, &app.hit);
    }
    if let Some(ref picker) = app.overlays.theme_picker {
        picker.view(f, size, &app.theme_styles, &app.hit);
    }
}

/// The panel that grows up from the composer: a permission ask, the restack panel, a question, or
/// the `/` and `@` flyouts.
fn render_composer_panel(f: &mut Frame, app: &App, regions: &ChatRegions, composer_area: Rect) {
    let &ChatRegions { conversation_area, composer_column, .. } = regions;
    // The slash/@ pickers grow upward from the composer, so they must inherit the composer's
    // x/width — not the full terminal — or they'd spill over the side-by-side dock. `y` marks the
    // top of the region above the input the picker may fill (`height` is unused by the renderers,
    // which size themselves from `input_top`).
    let flyout_region = Rect {
        x: composer_area.x,
        y: conversation_area.y,
        width: composer_area.width,
        height: composer_column.y.saturating_sub(conversation_area.y),
    };
    // A pending permission prompt owns the space above the composer; the `/` and `@` flyouts
    // only appear while composing, so they never coincide, but the prompt wins if they somehow do.
    // What grows out of the composer, and what the agent puts in front of you, wears its mode
    let tinted = crate::modes::styles(&app.status.mode, &app.theme_styles);
    if let Some(ref prompt) = app.overlays.permission {
        crate::permission_prompt::render_permission_prompt(
            f,
            flyout_region,
            composer_column.y,
            prompt,
            &tinted,
            &app.hit,
        );
    } else if let Some(ref panel) = app.overlays.restack {
        let frame = app.status.spinner_frame;
        crate::restack_panel::render(
            f,
            flyout_region,
            composer_column.y,
            panel,
            &app.theme_styles,
            frame,
        );
    } else if let Some(ref prompt) = app.overlays.question {
        crate::question_prompt::render_question_prompt(
            f,
            flyout_region,
            composer_column.y,
            prompt,
            &tinted,
            &app.hit,
        );
    } else if let Some(ref flyout) = app.overlays.slash_flyout {
        crate::slash_flyout::render_slash_flyout(
            f,
            flyout_region,
            crate::picker::Edge::Above(composer_column.y),
            flyout,
            &tinted,
            &app.hit,
        );
    } else if let Some(ref mention) = app.overlays.mention {
        crate::mention_flyout::render_mention_flyout(
            f,
            flyout_region,
            crate::picker::Edge::Above(composer_column.y),
            mention,
            &tinted,
            &app.hit,
            app.status.spinner_frame,
        );
    }
}

/// Draws whatever owns the conversation column: the transcript, or one file's diff.
///
/// The Changes pane stands *here* rather than over the whole screen so the dock keeps listing the
/// files (click another and the pane follows) and the composer keeps the keyboard — the two things
/// a reader does after looking at a diff.
fn render_conversation(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    diff_column: Option<ratatui::layout::Rect>,
    app: &mut App,
) -> Option<ratatui::layout::Rect> {
    let frame = f.area();
    let Some(diff) = diff_column else {
        if app.changes.is_some() {
            changes::render(f, gutter_inset(area, frame), app);
            return None;
        }
        transcript::render_transcript(f, area, app);
        return Some(area);
    };
    changes::render(f, gutter_inset(diff, frame), app);
    // The rule runs the full height of both columns, between whichever of them is on the left.
    let diff_on_left = diff.x < area.x;
    let (border, x) = match diff_on_left {
        true => (Borders::RIGHT, diff.right()),
        false => (Borders::LEFT, diff.x.saturating_sub(1)),
    };
    let rule = ratatui::layout::Rect { x, width: 1, ..diff };
    let stack_width = diff.width.saturating_add(1).saturating_add(area.width);
    if let Some((min, max)) = peek_width_bounds(stack_width) {
        let handle = crate::mouse::DividerHandle {
            pane: workspace_layout::Pane::Peek,
            anchor: if diff_on_left { diff.x } else { diff.right() },
            pane_on_left: diff_on_left,
            min,
            max,
        };
        divider::render(f, rule, border, rule, handle, app);
    }
    transcript::render_transcript(f, area, app);
    Some(area)
}

#[cfg(test)]
mod frame_pins;
#[cfg(test)]
mod memo_tests;
#[cfg(test)]
mod tests;
