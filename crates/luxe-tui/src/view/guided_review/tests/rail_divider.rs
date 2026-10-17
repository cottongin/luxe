//! Rail divider — tests moved out of the module they cover.

use crate::app::Msg;
use crate::fixture::{seeded_app, FixtureScenario};

/// The rule clears the screen top and runs down through the shortcut row, and the shortcuts
/// start beside it rather than centered under both panes: they are the diff's keys.
#[test]
fn the_rail_rule_runs_down_beside_the_diffs_shortcuts() {
    let (width, height) = (170, 44);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewWide).unwrap();
    app.update(Msg::TerminalResized { width, height });

    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| super::render(frame, frame.area(), &app)).expect("render the review");
    let buffer = terminal.backend().buffer().clone();

    let column = (0..width)
        .find(|x| {
            (0..height).filter(|y| buffer[(*x, *y)].symbol() == "│").count()
                > usize::from(height / 2)
        })
        .expect("a vertical rule exists");
    let rows: Vec<u16> = (0..height).filter(|y| buffer[(column, *y)].symbol() == "│").collect();

    assert!(!rows.contains(&0), "the rule clears the top row: {rows:?}");
    assert!(rows.contains(&(height - 1)), "the rule reaches the shortcut row: {rows:?}");
    assert!(rows.len() > 10, "it is still a rule, not a stub: {rows:?}");

    // Where you are sits on the heading's row, at the rail's right edge, like peek's `2 of 5`
    let head: String = (0..column).map(|x| buffer[(x, 1)].symbol()).collect();
    assert!(head.contains("REVIEW"), "{head:?}");
    assert!(head.trim_end().ends_with("Step 1 of 2"), "{head:?}");

    let foot: String = (0..width).map(|x| buffer[(x, height - 1)].symbol()).collect();
    let first_key = foot.find("space").expect("the footer names Space");
    assert_eq!(
        foot[..first_key].chars().count(),
        usize::from(column) + 2,
        "the shortcuts start one cell into the diff pane: {foot:?}"
    );
}
