//! Frame gutter — where luxe draws relative to the edges it meets.
//!
//! The terminal's edge gets no air: luxe draws flush to it and uses every column it was given,
//! which also lines it up with a host that frames the pane (herdr's tab row starts flush at the
//! pane's left edge). A rule luxe drew itself keeps a column of air on the side facing the text.
//! These tests pin both halves.

use luxe_tui::app::{App, Msg};
use luxe_tui::fixture::{seeded_app, FixtureScenario};
use luxe_tui::view::render;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

/// A wide chat so the dock sits beside the conversation and both edges are in play.
const WIDE: (u16, u16) = (160, 40);

fn draw(app: &mut App, (width, height): (u16, u16)) -> Buffer {
    app.update(Msg::TerminalResized { width, height });
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| render(frame, app)).expect("render");
    terminal.backend().buffer().clone()
}

fn chat() -> Buffer {
    let mut app = seeded_app("luxe", FixtureScenario::ChatTools).expect("fixture");
    draw(&mut app, WIDE)
}

fn row_text(buffer: &Buffer, y: u16) -> String {
    (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect()
}

fn inked(buffer: &Buffer, x: u16) -> bool {
    (0..buffer.area.height).any(|y| buffer[(x, y)].symbol().trim() != "")
}

/// The row the dock's own PROJECT heading landed on.
fn project_slab_row(buffer: &Buffer) -> u16 {
    (0..buffer.area.height)
        .find(|&y| row_text(buffer, y).contains("PROJECT"))
        .expect("the dock drew its PROJECT heading")
}

/// Both outer columns carry ink: a column of air there is a column of conversation the reader
/// does not get, and inside herdr it put the dock one cell right of the tab row above it.
#[test]
fn luxe_draws_flush_to_both_terminal_edges() {
    let buffer = chat();
    assert!(inked(&buffer, 0), "the left edge is drawn on");
    assert!(inked(&buffer, buffer.area.width - 1), "the right edge is drawn on");
}

/// The inner gutter answers a different question — it keeps text off a rule luxe owns — so it
/// stays. Measured off the dock separator, the one edge both columns meet.
#[test]
fn the_column_beside_the_dock_keeps_its_air() {
    let buffer = chat();
    let slab_row = project_slab_row(&buffer);
    let separator = (0..buffer.area.width)
        .find(|&x| buffer[(x, slab_row)].symbol() == "\u{2502}")
        .expect("the dock separator");
    let first_ink_after = (separator + 1..buffer.area.width)
        .find(|&x| inked(&buffer, x))
        .expect("the conversation drew something");
    assert_eq!(
        first_ink_after,
        separator + 2,
        "the conversation column stands one cell off the rule"
    );
}
