//! Capture — a fixture scenario rendered the way the event loop would, settled on one frame.

use super::color::hex;
use super::fail;
use luxe_tui::app::App;
use luxe_tui::fixture::{seeded_app, FixtureScenario};
use luxe_tui::view;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;
use std::time::Duration;

/// Frames a clock-pinned scenario is stepped before capture, matching `ui_fixture` so a still is
/// the same picture in both tools.
const REVEAL_FRAMES: u32 = 28;

/// A scenario rendered through the production view path, and the theme's text color.
///
/// The ink is what a cell the app left at the terminal default is drawn in, which a palette needs
/// as a real value: one theme may paint a cell another leaves alone.
///
/// `overlay` opens a state over the scenario, by the names `ui_fixture`'s `LUXE_FIXTURE_OVERLAY`
/// takes, so the interactive fixture and the still draw the same picture: `github`,
/// `github-detail`, `github-conversation`, `github-draft`, `github-issue`, `github-pulls`,
/// `github-search`, `github-merged`, `pr`, `pr-merge`, a login state, or `mention-<tab>`.
pub(super) fn render(
    theme: &str,
    scenario: FixtureScenario,
    overlay: Option<&str>,
    cols: u16,
    rows: u16,
) -> (Buffer, String) {
    let mut app = seeded_app(theme, scenario).unwrap_or_else(|error: String| fail(&error));
    let mut terminal = Terminal::new(TestBackend::new(cols, rows)).expect("in-memory terminal");
    app.update(luxe_tui::app::Msg::TerminalResized { width: cols, height: rows });
    if let Some(overlay) = overlay {
        if let Some(login) = luxe_tui::fixture::LoginFixture::parse(overlay) {
            luxe_tui::fixture::seed_login_wizard(&mut app, login);
        } else if let Some(tab) = overlay.strip_prefix("mention-") {
            luxe_tui::fixture::seed_mention_picker(&mut app, tab);
        } else if overlay.starts_with("pr") || overlay.starts_with("github") {
            luxe_tui::fixture::seed_github(&mut app, overlay);
        } else {
            fail(&format!("unknown overlay {overlay}"));
        }
    }

    step_reveal(&mut terminal, &mut app);
    terminal.draw(|frame| view::render(frame, &mut app)).expect("render");
    let ink = app.theme_styles.text.fg.and_then(hex).unwrap_or_else(|| "#ffffff".into());
    (terminal.backend().buffer().clone(), ink)
}

/// Runs a mid-animation scenario forward the way the event loop does, so the captured frame is the
/// same one every time rather than wherever the clock happened to land.
fn step_reveal(terminal: &mut Terminal<TestBackend>, app: &mut App) {
    let Some(base) = app.status.frame_clock else { return };
    for step in 1..=REVEAL_FRAMES {
        app.status.frame_clock = Some(base + Duration::from_millis(12) * step);
        terminal.draw(|frame| view::render(frame, app)).expect("render");
    }
}
