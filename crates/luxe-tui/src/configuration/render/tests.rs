#![allow(clippy::unwrap_used)]

use super::super::tests::{files, key, provider, state};
use super::*;
use crossterm::event::KeyCode;
use luxe_core::auth::CredentialStatus;
use ratatui::{backend::TestBackend, Terminal};

fn theme() -> ThemeStyles {
    let registry = luxe_core::theme::ThemeRegistry::new();
    let resolved = registry.resolve("nord").expect("nord resolves");
    ThemeStyles::from_resolved(&resolved, "nord")
}

fn render_at(state: &ConfigurationState, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let theme = theme();
    let hit = std::cell::RefCell::new(HitRegions::default());
    let clock = BannerClock::new(false);
    terminal.draw(|frame| state.view(frame, frame.area(), &theme, &hit, clock)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(width as usize)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn render(state: &ConfigurationState) -> String {
    render_at(state, 100, 34)
}

/// The logotype on the same clock as the dashboard's. Setup used to hold a still, so walking
/// here from an animated dashboard put the light out.
#[test]
fn the_logotype_moves_on_the_surface_that_is_most_peoples_first_screen() {
    let colors = |clock: BannerClock| {
        let mut terminal = Terminal::new(TestBackend::new(100, 34)).unwrap();
        let theme = theme();
        let hit = std::cell::RefCell::new(HitRegions::default());
        terminal.draw(|frame| state().view(frame, frame.area(), &theme, &hit, clock)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .filter(|cell| cell.symbol() == "█")
            .map(|cell| cell.fg)
            .collect::<Vec<_>>()
    };
    let mid_pass = colors(BannerClock::default().with_tick(24));
    assert!(!mid_pass.is_empty(), "the logotype is on screen at all");
    assert_ne!(
        mid_pass,
        colors(BannerClock::default().with_tick(crate::widgets::SHINE_SWEEP_TICKS + 8)),
        "the shine has to cross these letters too"
    );
    // And a reader who turned it off gets the still, on every surface that draws the mark.
    assert_eq!(
        colors(BannerClock::new(false)),
        colors(BannerClock::new(false).with_tick(24)),
        "banner_animation = false is one answer for the whole product"
    );
}

#[test]
fn the_menu_offers_every_way_in_and_says_what_each_reaches() {
    let screen = render(&state());
    assert!(screen.contains("Configuration"), "{screen}");
    assert!(screen.contains("Sign in to a subscription"), "{screen}");
    assert!(screen.contains("ChatGPT Plus or Pro"), "{screen}");
    assert!(screen.contains("Add an API key"));
    assert!(screen.contains("Connect a local or custom model"));
    assert!(screen.contains("Settings"), "{screen}");
    assert!(screen.contains("Edit configuration files"));
    assert!(screen.contains("[5]"), "the rows are numbered: {screen}");
    assert!(screen.contains("enter open"));
}

/// The reason the surface exists has to be on it: someone sent here by the dashboard should not
/// have to infer that nothing is set up.
#[test]
fn an_unconfigured_machine_is_told_so_and_a_configured_one_is_told_what_it_has() {
    assert!(render(&state()).contains("No provider is configured yet"));

    let signed_in = ConfigurationState::open(
        &[provider("openai", CredentialStatus::Env("OPENAI_API_KEY".into()))],
        files(),
    );
    let screen = render(&signed_in);
    assert!(screen.contains("Configured: OPENAI"), "{screen}");

    // A local server is connected rather than signed in to, and the line that names both
    // has one verb to spend — so it spends a true one.
    let local = ConfigurationState::open(
        &[provider("lmstudio", CredentialStatus::Local("http://127.0.0.1:1234/v1".into()))],
        files(),
    );
    let screen = render(&local);
    assert!(screen.contains("local — http://127.0.0.1:1234/v1"), "{screen}");
    assert!(!screen.contains("Signed in"), "nothing was signed in to: {screen}");
}

#[test]
fn the_file_list_names_the_files_and_the_directory_they_are_in() {
    let mut config = state();
    config.on_key(&key(KeyCode::Char('5')));
    let screen = render(&config);
    assert!(screen.contains("Configuration files"), "{screen}");
    assert!(screen.contains("config.json"));
    assert!(screen.contains("models.json"));
    assert!(screen.contains("auth.json"));
    assert!(screen.contains(".config/luxe"), "the bare names need a directory: {screen}");
    assert!(screen.contains("enter open in your editor"));
}

/// The logotype is the part that gives way: the panel is what the reader came for.
#[test]
fn the_logotype_takes_whatever_height_the_panel_leaves() {
    let state = state();
    let tall = frame(Rect::new(0, 0, 100, 40), state.panel_height());
    assert!(tall.has_banner(), "a tall terminal has room for the mark");
    assert!(
        tall.banner.bottom() <= tall.panel.y,
        "the mark and the panel must not overlap: {tall:?}"
    );

    let short = frame(Rect::new(0, 0, 100, state.panel_height()), state.panel_height());
    assert!(!short.has_banner(), "a terminal with room for one thing draws the panel");
    assert_eq!(short.panel.height, state.panel_height());
}

/// A panel wider than the terminal would draw its right border off screen, and the whole
/// surface with it.
#[test]
fn a_narrow_terminal_still_gets_a_whole_panel() {
    let screen = render_at(&state(), 44, 24);
    assert!(screen.contains("Sign in to a subscription"), "{screen}");
    for row in screen.lines().filter(|row| row.contains('│')) {
        assert_eq!(row.matches('│').count() % 2, 0, "an unbalanced border row: {row:?}");
    }
}

#[test]
fn a_terminal_too_small_for_the_surface_says_so() {
    let screen = render_at(&state(), 20, 6);
    assert!(screen.contains("Resize"), "{screen}");
}

/// The cell each of a row's two chamfers sits on, for whichever row carries `needle`.
fn edges(screen: &str, needle: &str) -> (usize, usize) {
    let (open, close) = (crate::widgets::Cap::Open.glyph(), crate::widgets::Cap::Close.glyph());
    let row = screen.lines().find(|row| row.contains(needle)).expect("the row is on screen");
    let at = |cut: Option<usize>| UnicodeWidthStr::width(&row[..cut.expect("a cut")]);
    (at(row.find(open)), at(row.rfind(close)))
}

/// The cell `needle` starts on, wherever it is.
fn column(screen: &str, needle: &str) -> usize {
    let row = screen.lines().find(|row| row.contains(needle)).expect("the row is on screen");
    UnicodeWidthStr::width(&row[..row.find(needle).expect("found above")])
}

/// An entry is a label and what choosing it reaches — one choice, so one slab, capped on the
/// same two columns and with the detail sitting under the label it explains.
#[test]
fn a_chosen_entry_is_cut_as_one_slab() {
    crate::glyphs::set_tier(crate::glyphs::GlyphTier::Nerd);
    let screen = render(&state());

    let (label_open, label_close) = edges(&screen, "Sign in to a subscription");
    let (detail_open, detail_close) = edges(&screen, "ChatGPT Plus or Pro");
    assert_eq!(label_open, detail_open, "one leading column");
    assert_eq!(label_close, detail_close, "one trailing column");
    assert_eq!(
        column(&screen, "ChatGPT Plus or Pro"),
        column(&screen, "Sign in to a subscription"),
        "the detail opens on the label's column"
    );
}

/// The frame is spent on every entry, chosen or not, so the list holds its columns as the
/// cursor travels rather than stepping sideways under it.
#[test]
fn moving_the_cursor_does_not_move_a_single_column() {
    crate::glyphs::set_tier(crate::glyphs::GlyphTier::Nerd);
    let resting = render(&state());
    let mut moved = state();
    moved.on_key(&key(KeyCode::Down));
    let moved = render(&moved);

    for needle in [
        "Sign in to a subscription",
        "ChatGPT Plus or Pro",
        "Add an API key",
        "OpenAI, Anthropic",
        "[4]",
    ] {
        assert_eq!(column(&resting, needle), column(&moved, needle), "{needle} moved");
    }
}

/// The panel is sized at two rows an entry, so a line that overran its row would wrap and push
/// every entry below it out through the bottom border. Both lines clip, and the slab keeps the
/// closing cap that says where the choice ends.
#[test]
fn a_narrow_panel_clips_an_entry_rather_than_overrunning_it() {
    crate::glyphs::set_tier(crate::glyphs::GlyphTier::Nerd);
    let mut config = state();
    // Onto the longest label on the surface, which is the one with something to clip.
    config.on_key(&key(KeyCode::Down));
    config.on_key(&key(KeyCode::Down));
    let screen = render_at(&config, 44, 24);

    assert!(screen.contains('\u{2026}'), "the overrun is marked as clipped: {screen}");
    assert!(screen.contains("[3]"), "the number that runs it survives the clip: {screen}");
    let (open, close) = edges(&screen, "[3]");
    assert!(open < close, "the chosen row is cut at both ends: {screen}");
    // Two lines and no more: a wrapped row would put a third between this entry and the next.
    let rows: Vec<usize> = screen
        .lines()
        .enumerate()
        .filter(|(_, row)| row.contains(crate::widgets::Cap::Close.glyph()))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(rows.len(), 2, "one slab, two lines: {rows:?}");
    assert_eq!(rows[1], rows[0] + 1, "and they are consecutive: {rows:?}");
}
