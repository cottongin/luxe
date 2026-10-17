//! Composer — the input box: its border, its growth, pastes and the pickers anchored to it.

use super::*;

/// Finds the composer's rounded top-left corner cell.
pub(super) fn composer_corner(buffer: &Buffer) -> (u16, u16) {
    (0..buffer.area.height)
        .flat_map(|y| (0..buffer.area.width).map(move |x| (x, y)))
        .find(|&(x, y)| buffer[(x, y)].symbol() == "╭")
        .expect("composer top-left corner")
}

/// The outline used to change color with a vim-style editor mode whenever the statusline was
/// hidden. It is the theme's base border color now, in every state.
#[test]
fn composer_border_keeps_the_base_color_in_every_state() {
    let (width, height) = (80, 24);
    let mut app = create_app();
    app.statusline_state.hide();
    let base = app.theme_styles.border_accent.fg.expect("base border color");

    let buffer = render_app(&mut app, &test_backend(width, height));
    assert_eq!(buffer[composer_corner(&buffer)].fg, base, "empty composer");

    app.editor.insert_text("typing");
    let buffer = render_app(&mut app, &test_backend(width, height));
    assert_eq!(buffer[composer_corner(&buffer)].fg, base, "composer holding a draft");

    app.selected_block = Some(0);
    let buffer = render_app(&mut app, &test_backend(width, height));
    assert_eq!(buffer[composer_corner(&buffer)].fg, base, "conversation block selected");
}

#[test]
fn multiline_input_grows_the_composer_and_preserves_line_breaks() {
    let (width, height) = (80, 24);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.editor.insert_text("alpha\nbeta");

    let buffer = render_app(&mut app, &backend);

    assert!(row_text(&buffer, height - 4, width).contains("alpha"));
    assert!(row_text(&buffer, height - 3, width).contains("beta"));
    assert!(row_text(&buffer, height - 2, width).trim_start().starts_with('╰'));
}

#[test]
fn the_composer_grows_the_moment_the_draft_wraps() {
    // The prompt box is framed, so its text area is two columns narrower than the terminal.
    // Measuring the wrap at any wider value let the draft spill onto a second row while the frame
    // was still one row tall — the first line scrolled out of sight and only the wrapped word
    // showed until another keystroke arrived.
    let (width, height) = (80, 24);
    let backend = test_backend(width, height);
    let mut app = create_app();
    // 73 a's + " bravo!": exactly 80 cells, so it fits a measurement that forgot the frame but not
    // the 78 the box actually draws at.
    let head = "a".repeat(73);
    app.editor.insert_text(&format!("{head} bravo!"));

    let buffer = render_app(&mut app, &backend);
    assert!(row_text(&buffer, height - 4, width).contains(&head), "the first row keeps the draft");
    assert!(row_text(&buffer, height - 3, width).contains("bravo!"), "the wrapped word is visible");
    assert!(row_text(&buffer, height - 2, width).trim_start().starts_with('╰'), "box bottom");
}

#[test]
fn the_composer_grows_beside_the_dock_at_the_column_it_actually_wraps_at() {
    // The sidebar case, which is where this was reported: the composer lives in the main column,
    // so the wrap column comes off *that* width, not the terminal's.
    let (width, height) = (160u16, 40u16);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::DockWide).unwrap();
    app.update(Msg::TerminalResized { width, height });
    let metrics = luxe_tui::workspace_layout::resolve_workspace_layout(width);
    assert!(metrics.separator_width >= 1, "precondition: the dock sits beside the conversation");

    // Sized so the trailing word overflows the real text area but would still fit the two extra
    // columns the old measurement assumed — the exact window where the box used to lag.
    // A column of air off the separator, then the frame
    let text_width = usize::from(metrics.conversation_width) - 3;
    let head = "a".repeat(text_width - " bravo!".len() + 2);
    app.editor.insert_text(&format!("{head} bravo!"));

    let buffer = render_app(&mut app, &backend);
    let composer_top =
        (0..height).rfind(|&y| row_text(&buffer, y, width).contains('╭')).expect("composer top");
    assert!(
        row_text(&buffer, composer_top + 1, width).contains(&head),
        "the first row keeps the draft: {}",
        buffer_to_string(&buffer)
    );
    assert!(
        row_text(&buffer, composer_top + 2, width).contains("bravo!"),
        "the wrapped word gets its own row: {}",
        buffer_to_string(&buffer)
    );
}

pub(super) fn assert_composer_paste_background(app: &mut App, marker: &str) {
    let (width, height) = (80, 24);
    let backend = test_backend(width, height);
    let expected_bg = app.theme_styles.paste_indicator.bg.expect("paste background");
    let buffer = render_app(app, &backend);
    let row = height - 3;
    // The composer is flush to the edge, so its inner text starts at x=1 (border at x=0)
    let marker_start = 1 + "before ".len() as u16;
    let marker_end = marker_start + marker.len() as u16 - 1;
    let normal_bg = buffer[(1, row)].bg;

    assert!(row_text(&buffer, row, width).contains(&format!("before {marker} after")));
    assert_ne!(normal_bg, expected_bg);
    for column in marker_start..=marker_end {
        assert_eq!(buffer[(column, row)].bg, expected_bg, "column {column}");
    }
    assert_eq!(buffer[(marker_end + 2, row)].bg, normal_bg);
}

#[test]
fn compact_text_paste_has_a_distinct_background() {
    let mut app = create_app();
    app.editor.insert_text("before ");
    app.editor.paste_text("one\ntwo\nthree");
    app.editor.insert_text(" after");

    assert_composer_paste_background(&mut app, "[Pasted 3 lines]");
}

#[cfg(feature = "clipboard-images")]
#[test]
fn compact_image_paste_uses_clean_wording_and_a_distinct_background() {
    let mut app = create_app();
    app.editor.insert_text("before ");
    assert!(app.editor.attach_image(vec![255, 0, 0, 255], 1, 1));
    app.editor.insert_text(" after");

    assert_composer_paste_background(&mut app, "[Pasted Image]");
}

/// A slash command being typed shows in the composer.
#[test]
fn a_slash_command_being_typed_shows_in_the_composer() {
    let backend = test_backend(80, 24);
    let mut app = create_app();
    app.editor.insert_text("/help");

    let rendered = buffer_to_string(&render_app(&mut app, &backend));

    assert!(rendered.contains("/help"), "the command is missing: {rendered}");
}

#[test]
fn test_slash_picker_is_anchored_to_input() {
    let (w, h) = (80, 24);
    let backend = test_backend(w, h);
    let mut app = create_app();
    app.update(Msg::Key(key(KeyCode::Char('/'))));
    app.update(Msg::Key(key(KeyCode::Char('h'))));

    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);
    let picker_title_row = (0..h)
        .find(|&y| row_text(&buffer, y, w).contains(" Commands "))
        .expect("slash picker title");

    assert!(picker_title_row > 0, "picker must not be pinned to the top edge");
    // The picker sits one row above the composer and shares its column
    assert_eq!(buffer[(0, h - 5)].symbol(), "╰", "picker bottom-left must align with the composer");
    assert!(content.contains("│/h"), "query should render without a prompt glyph: {content}");
}

#[test]
fn slash_picker_matches_composer_width_beside_the_dock() {
    // Regression: the slash/@ pickers used to render at the full terminal width, so beside a
    // side-by-side dock they spilled over the sidebar. They must inherit the composer's column.
    let (w, h) = (160u16, 40u16);
    let backend = test_backend(w, h);
    let mut app = seeded_app("nord", FixtureScenario::DockWide).unwrap();
    app.update(Msg::TerminalResized { width: w, height: h });

    let metrics = luxe_tui::workspace_layout::resolve_workspace_layout(w);
    assert!(metrics.dock_width > 0 && metrics.separator_width >= 1, "precondition: side-by-side");

    app.update(Msg::Key(key(KeyCode::Char('/'))));
    let buffer = render_app(&mut app, &backend);

    // The composer box (rounded corners) defines the promptbox width.
    let comp_row =
        (0..h).find(|&y| row_text(&buffer, y, w).contains('╭')).expect("composer top border");
    let comp_left = (0..w).find(|&x| buffer[(x, comp_row)].symbol() == "╭").unwrap();
    let comp_right = (comp_left..w).find(|&x| buffer[(x, comp_row)].symbol() == "╮").unwrap();

    // The picker box shares the composer's rounded corners and must line up with it, not the
    // terminal edges.
    let title_row = (0..h)
        .find(|&y| row_text(&buffer, y, w).contains(" Commands "))
        .expect("slash picker title");
    let flyout_left = (0..w).find(|&x| buffer[(x, title_row)].symbol() == "╭").unwrap();
    let flyout_right = (flyout_left..w).find(|&x| buffer[(x, title_row)].symbol() == "╮").unwrap();

    assert_eq!(flyout_left, comp_left, "picker left edge must match the composer");
    assert_eq!(flyout_right, comp_right, "picker right edge must match the composer");
    assert!(
        flyout_left >= metrics.dock_width + metrics.separator_width,
        "picker must not cover the left sidebar (left={flyout_left}, dock={})",
        metrics.dock_width
    );
}

#[test]
fn test_mention_picker_is_anchored_to_input() {
    let (w, h) = (80, 24);
    let backend = test_backend(w, h);
    let mut app = create_app();
    app.update(Msg::Key(key(KeyCode::Char('@'))));
    app.update(Msg::Key(key(KeyCode::Char('c'))));

    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);
    let picker_title_row = (0..h)
        .find(|&y| row_text(&buffer, y, w).contains(" Files "))
        .expect("mention picker title");

    assert!(picker_title_row > 0, "picker must not be pinned to the top edge");
    assert_eq!(buffer[(0, h - 5)].symbol(), "╰", "picker bottom-left must align with the composer");
    assert!(content.contains("│@c"), "query should render without a prompt glyph: {content}");
}
