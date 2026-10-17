//! Panel chrome — the invariants that keep every bordered surface reading as one product.
//!
//! Two rules are enforced here, because they are the ones that silently rot as surfaces are added:
//! a panel's title is an accent glyph plus a **bold** label, and the focused row of a list is
//! **bold** on the selection background. A third test guards the seam itself — every bordered
//! panel must be built by `widgets::panel_block`/`titled_panel`, never hand-rolled.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use luxe_core::config::Config;
use luxe_core::keymap::KeyMap;
use luxe_tui::app::{App, Msg};
use luxe_tui::glyphs::Icon;
use luxe_tui::view::render;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Modifier;
use ratatui::Terminal;

fn app() -> App {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    App::new(tx, KeyMap::defaults(), Config::default())
}

fn draw(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| render(frame, app)).expect("render");
    terminal.backend().buffer().clone()
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn row_text(buffer: &Buffer, y: u16) -> String {
    (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect()
}

/// Asserts the panel identified by `glyph` wears the shared title chrome: the glyph in the theme
/// accent, one space, then `label` in bold.
fn assert_panel_title(buffer: &Buffer, app: &App, glyph: &str, label: &str) {
    let row = (0..buffer.area.height)
        .find(|&y| row_text(buffer, y).contains(glyph))
        .unwrap_or_else(|| panic!("no rendered row carries the {label:?} title glyph"));
    let text = row_text(buffer, row);
    let glyph_x = text.find(glyph).map(|byte| text[..byte].chars().count() as u16).expect("glyph");

    assert_eq!(
        buffer[(glyph_x, row)].fg,
        app.theme_styles.accent.fg.expect("the theme defines an accent"),
        "{label}: the title glyph is drawn in the accent color"
    );
    // " {glyph} {label} " — the label starts two cells past the glyph.
    for (offset, character) in label.chars().enumerate() {
        let cell = &buffer[(glyph_x + 2 + offset as u16, row)];
        assert_eq!(cell.symbol(), character.to_string(), "{label}: title text at +{offset}");
        assert!(cell.modifier.contains(Modifier::BOLD), "{label}: the title label must be bold");
    }
}

/// Asserts the row carrying the `❯` cursor is highlighted *and* bold, so the selection survives
/// a terminal that drops backgrounds. Every list row is `{cursor}{space}{label}`.
fn assert_focused_row(buffer: &Buffer, app: &App, surface: &str) {
    let selected_bg = app.theme_styles.selected_bg.bg.expect("the theme defines a selection bg");
    let row = (0..buffer.area.height)
        .find(|&y| row_text(buffer, y).contains('❯'))
        .unwrap_or_else(|| panic!("{surface}: no row carries the selection cursor"));
    let text = row_text(buffer, row);
    let cursor_x =
        text.find('❯').map(|byte| text[..byte].chars().count() as u16).expect("cursor column");
    let label = &buffer[(cursor_x + 2, row)];

    assert_eq!(label.bg, selected_bg, "{surface}: focused row is highlighted");
    assert!(
        label.modifier.contains(Modifier::BOLD),
        "{surface}: focused row label must be bold, not color-only"
    );
}

#[test]
fn the_slash_flyout_wears_the_shared_chrome() {
    let mut app = app();
    app.update(Msg::Key(key(KeyCode::Char('/'))));
    let buffer = draw(&mut app, 80, 24);

    assert_panel_title(&buffer, &app, Icon::SlashMenu.glyph(), "Commands");
    assert_focused_row(&buffer, &app, "slash flyout");
}

#[test]
fn the_mention_flyout_wears_the_shared_chrome() {
    let mut app = app();
    app.update(Msg::Key(key(KeyCode::Char('@'))));
    let buffer = draw(&mut app, 80, 24);

    assert_panel_title(&buffer, &app, Icon::FileMenu.glyph(), "Files");
    assert_focused_row(&buffer, &app, "mention flyout");
}

#[test]
fn the_command_palette_wears_the_shared_chrome() {
    let mut app = app();
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)));
    assert!(app.overlays.palette.is_some(), "ctrl+k opens the palette");
    let buffer = draw(&mut app, 80, 24);

    assert_panel_title(&buffer, &app, Icon::Palette.glyph(), "Command palette");
}

#[test]
fn the_transcript_search_bar_wears_the_shared_chrome() {
    let mut app = app();
    app.search_state.searching = true;
    app.search_state.query = "notification".into();
    let buffer = draw(&mut app, 80, 24);

    assert_panel_title(&buffer, &app, Icon::Search.glyph(), "Search conversation");
    // The field keeps the pickers' inline `Search:` label so the input is findable at a glance.
    let field = (0..24)
        .map(|y| row_text(&buffer, y))
        .find(|text| text.contains("Search:"))
        .expect("the search field keeps its inline label");
    assert!(field.contains("Search: notification"), "query follows the label: {field:?}");
}

/// The palette highlights fuzzy matches in accent-bold; the rest of the focused row must still be
/// bold so an empty query (no matches to highlight) still shows where the cursor is.
#[test]
fn the_palettes_focused_row_is_bold_without_a_query() {
    let mut app = app();
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)));
    let buffer = draw(&mut app, 80, 24);

    assert_focused_row(&buffer, &app, "command palette");
}

/// Overlays render over `Clear`, which resets cells to the *terminal's* background. Without an
/// explicit repaint a light theme shows a dark hole in a dark terminal (and vice versa), so every
/// overlay body must come back carrying the theme canvas color.
#[test]
fn overlays_repaint_the_theme_background_over_clear() {
    // A light palette, which is the case that shows the hole, and a dark built-in
    for light in [true, false] {
        let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
        let mut app = App::new(
            tx,
            KeyMap::defaults(),
            Config { theme: Some("nord".into()), ..Default::default() },
        );
        if light {
            app.theme_styles = luxe_tui::fixture::light_theme_styles();
        }
        app.update(Msg::Key(key(KeyCode::Char('/'))));
        let buffer = draw(&mut app, 80, 24);
        let canvas = app.theme_styles.background.bg.expect("the theme defines a canvas");

        let top = (0..24)
            .find(|&y| (0..80).any(|x| buffer[(x, y)].symbol() == "╭"))
            .expect("flyout top border");
        let left = (0..80).find(|&x| buffer[(x, top)].symbol() == "╭").expect("flyout left edge");
        // A cell inside the frame that no row text or selection highlight reaches.
        let body = &buffer[(left + 40, top + 3)];

        assert_eq!(
            body.bg, canvas,
            "light {light}: overlay body must not fall through to the terminal"
        );
    }
}

/// Guard the seam: a new surface must not hand-roll its frame or its title. Both would drift out of
/// the shared language the moment the border style or the title convention changes.
#[test]
fn bordered_panels_are_built_by_the_shared_constructors() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let owner = root.join("widgets").join("panel.rs");
    let mut offenders = Vec::new();

    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("readable source dir") {
            let path = entry.expect("readable entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs") || path == owner {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("readable source file");
            let relative = path.strip_prefix(&root).unwrap_or(&path).display().to_string();
            for (number, line) in source.lines().enumerate() {
                let line = line.trim();
                if line.starts_with("//") {
                    continue;
                }
                if line.contains("Borders::ALL") {
                    offenders
                        .push(format!("{relative}:{}: use widgets::panel_block()", number + 1));
                }
                if line.contains(".title(\"") {
                    offenders.push(format!(
                        "{relative}:{}: use widgets::titled_panel()/panel_title()",
                        number + 1
                    ));
                }
            }
        }
    }

    assert!(offenders.is_empty(), "hand-rolled panel chrome:\n  {}", offenders.join("\n  "));
}
