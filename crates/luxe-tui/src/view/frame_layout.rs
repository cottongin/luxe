//! Frame layout — where the chat frame's dock, transcript, strips, composer and statusline go.

use super::{input, pending};
use crate::app::App;
use crate::editor::TextLayout;
use crate::workspace_dock::DockPresentation;
use crate::workspace_layout::{self, WorkspaceLayoutMetrics};
use luxe_core::config::DockPosition;
use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Where each part of the chat frame goes, measured once per frame before anything draws.
pub(super) struct ChatRegions {
    pub(super) presentation: DockPresentation,
    pub(super) statusline_visible: bool,
    /// The rule and the full-height dock column, when the dock sits beside the conversation
    pub(super) separator: Option<Rect>,
    pub(super) dock_column: Option<Rect>,
    /// The main column above the statusline row
    pub(super) stack: Rect,
    pub(super) statusline_column: Rect,
    pub(super) diff_column: Option<Rect>,
    /// The draft wrapped once for the frame; `None` while searching or under a replacement dock
    pub(super) composer_layout: Option<TextLayout>,
    pub(super) conversation_area: Rect,
    pub(super) activity_area: Rect,
    pub(super) tray_area: Rect,
    pub(super) composer_column: Rect,
}

/// Measures the chat frame: the dock column, the main column's stack and statusline, a peeked
/// diff's column, and the conversation column's rows top to bottom.
pub(super) fn build_chat_regions(app: &App, size: Rect) -> ChatRegions {
    let dock_presentation = app.dock.presentation(app.workspace_layout);
    let statusline_visible = app.statusline_state.visible(dock_presentation);

    // Carve the full-height dock column first so the sidebar spans the entire viewport height;
    // the conversation, composer, and (optional) statusline share the remaining main column.
    // `body` is the main column; when there is no side-by-side dock it is the whole terminal.
    let (body, separator, dock_column) = if dock_presentation == DockPresentation::SideBySide {
        let metrics = workspace_layout::resolve_workspace_layout_with(
            size.width,
            app.panes.width(workspace_layout::Pane::Dock),
        );
        let (main, sep, dock_rect) = side_by_side_columns(size, app.dock.position, metrics);
        (main, Some(sep), Some(dock_rect))
    } else {
        (size, None, None)
    };

    let statusline_height = u16::from(statusline_visible);
    // The statusline spans the whole main column: it is session telemetry, and it belongs to
    // neither of the columns above it.
    let stack =
        ratatui::layout::Rect { height: body.height.saturating_sub(statusline_height), ..body };
    let statusline_column =
        ratatui::layout::Rect { y: stack.bottom(), height: statusline_height, ..body };
    // A peeked diff takes a column of its own, full height beside the conversation's whole stack —
    // composer included. Under both columns the prompt would say the text goes to both, and the
    // diff would stop short of the screen for no reason.
    let (diff_column, chat_column) = super::canvas::peek_columns(app, stack);

    // One wrap of the draft for the whole frame. The composer's height, its rows and its caret all
    // read off the same layout, and asking three times walks the draft three times per keystroke.
    let composer_layout = (dock_presentation != DockPresentation::Replacement
        && !app.search_state.is_active())
    .then(|| {
        // A slash command lights as it is typed: the trigger, and the words of it the draft spells
        let marks = crate::slash_args::marks(
            app.editor.current_input(),
            crate::commands::CommandRegistry::shared(),
        );
        let room = input::composer_area(chat_column, size).width;
        app.editor.layout_marked(input::text_width(room), &marks)
    });
    let input_height = match &composer_layout {
        Some(layout) => input::height(layout, chat_column.height),
        None if app.search_state.is_active() => 3,
        None => 0,
    };
    // Mid-turn submissions sit between the transcript and the composer, so they hold still while
    // the conversation scrolls underneath them.
    let tray_height =
        if dock_presentation == DockPresentation::Replacement { 0 } else { pending::height(app) };
    // Everything between the top of the conversation column and the composer — the room a modal
    // that grows up from the prompt has to grow into.
    let above_composer = chat_column.height.saturating_sub(input_height);
    let modal_rows = if dock_presentation == DockPresentation::Replacement {
        0
    } else {
        reserved_modal_rows(
            app,
            input::composer_area(chat_column, size).width,
            above_composer,
            ACTIVITY_STRIP_HEIGHT + tray_height,
        )
    };
    // Top to bottom the conversation column reads chronologically: what was said, what the agent is
    // doing now, what the user has lined up next, and the prompt they are typing into. The activity
    // strip is always reserved — a blank row and the indicator's own — so a turn starting or
    // ending never shifts the composer out from under the cursor, and the prompt keeps its
    // breathing room from the transcript when nothing is running.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(modal_rows),
            Constraint::Length(ACTIVITY_STRIP_HEIGHT),
            Constraint::Length(tray_height),
            Constraint::Length(input_height),
        ])
        .split(chat_column);
    let (conversation_area, activity_area, tray_area, composer_column) =
        (chunks[0], chunks[2], chunks[3], chunks[4]);
    ChatRegions {
        presentation: dock_presentation,
        statusline_visible,
        separator,
        dock_column,
        stack,
        statusline_column,
        diff_column,
        composer_layout,
        conversation_area,
        activity_area,
        tray_area,
        composer_column,
    }
}

/// Rows reserved between the transcript and the composer for the live activity indicator: a
/// blank separator plus the indicator's own row.
const ACTIVITY_STRIP_HEIGHT: u16 = 2;

/// Rows of conversation a modal may never take, however tall the panel wants to be.
const MIN_TRANSCRIPT_ROWS: u16 = 1;

/// Rows a modal growing up from the composer takes out of the conversation.
///
/// A permission ask and a question are both drawn upward from the prompt over a `Clear`, so
/// whatever the transcript last laid out there is gone — and what the transcript last laid out is
/// the reasoning the prompt is about. The conversation gives up exactly the rows the panel will
/// occupy, less the `strips` between them that it was going to cover anyway, so the answer is
/// still readable beside the question that prompted it.
///
/// One row of transcript always survives: a panel taller than the terminal has already lost that
/// argument, and squeezing the conversation to nothing would let the layout take its rows out of
/// the composer instead.
fn reserved_modal_rows(app: &App, width: u16, above_composer: u16, strips: u16) -> u16 {
    let panel = if let Some(prompt) = app.overlays.permission.as_ref() {
        crate::permission_prompt::height(prompt, above_composer)
    } else if let Some(panel) = app.overlays.restack.as_ref() {
        crate::restack_panel::height(panel, &app.theme_styles, width, above_composer)
    } else if let Some(prompt) = app.overlays.question.as_ref() {
        crate::question_prompt::height(prompt, &app.theme_styles, width, above_composer)
    } else {
        return 0;
    };
    let conversation = above_composer.saturating_sub(strips);
    panel.saturating_sub(strips).min(conversation.saturating_sub(MIN_TRANSCRIPT_ROWS))
}

/// Splits the full terminal into the main content column and the full-height dock column.
///
/// Returns `(main, separator, dock)` placed on the side selected by `position`. The dock spans
/// the entire viewport height so the sidebar reads as one panel top-to-bottom, while the main
/// column keeps the conversation, composer, and optional statusline.
fn side_by_side_columns(
    size: Rect,
    position: DockPosition,
    metrics: WorkspaceLayoutMetrics,
) -> (Rect, Rect, Rect) {
    let dock = Constraint::Length(metrics.dock_width);
    let sep = Constraint::Length(metrics.separator_width);
    let main = Constraint::Length(metrics.conversation_width);
    match position {
        DockPosition::Right => {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([main, sep, dock])
                .split(size);
            (cols[0], cols[1], cols[2])
        }
        DockPosition::Left | DockPosition::Unknown => {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([dock, sep, main])
                .split(size);
            (cols[2], cols[1], cols[0])
        }
    }
}

/// Positions a `dock_width`-wide overlay against the configured side of `area`.
pub(super) fn overlay_rect(area: Rect, position: DockPosition, dock_width: u16) -> Rect {
    let width = dock_width.min(area.width);
    let x = match position {
        DockPosition::Right => area.right().saturating_sub(width),
        DockPosition::Left | DockPosition::Unknown => area.x,
    };
    Rect { x, y: area.y, width, height: area.height }
}
