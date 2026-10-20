//! Guests — who's joined the session from elsewhere, as the rail's SESSION block shows them.

use super::rail::{app, text};
use super::*;

/// The rail's lines as text.
fn rendered(app: &App) -> Vec<String> {
    telemetry_lines(app).iter().map(text).collect()
}

/// Who's joined shows in SESSION while anyone has, even before a turn has anything to measure,
/// and leaves no row once they've gone.
///
/// The break this catches: a phone joined to an idle session shown nowhere (SESSION waits for a
/// turn), or the row left behind after everyone left.
#[test]
fn the_rail_shows_who_has_joined_while_they_are_here() {
    let mut app = app();
    assert!(!rendered(&app).iter().any(|line| line.contains("joined")));
    app.status.guests = vec!["iPhone".into(), "iPad".into()];
    let lines = rendered(&app);
    let row = lines.iter().find(|line| line.contains("joined")).expect("a joined row");
    assert!(row.ends_with("iPhone, iPad"), "{row:?}");
    assert!(lines.iter().any(|line| line.contains("SESSION")), "{lines:?}");
    app.status.guests.clear();
    assert!(!rendered(&app).iter().any(|line| line.contains("joined")));
}
