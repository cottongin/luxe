//! Every chord the review footer advertises must actually resolve to something.
//!
//! This exists because `Enter` did not. Both footers said "Enter open" from the day the
//! walkthrough shipped, but nothing was bound to it, so it fell through the review router's
//! catch-all into `Msg::Tick` — the rows looked like a list and behaved like paint. A promise
//! printed on screen is a contract; this test is what keeps it one.

use super::*;
use crate::app::Msg;
use crate::fixture::{seeded_app, FixtureScenario};
use crate::main_loop::{route_key, RouteAction};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Turns a footer label's leading chord ("Alt+V hide guide", "s change source") into the key
/// event a user would produce. `None` for a label that names a constraint rather than a key.
fn chord_of(label: &str) -> Option<KeyEvent> {
    let chord = label.split_whitespace().next()?;
    if label.contains("needs") {
        return None;
    }
    let (modifiers, key) = match chord.split_once('+') {
        Some(("alt", key)) => (KeyModifiers::ALT, key),
        Some(("ctrl", key)) => (KeyModifiers::CONTROL, key),
        Some(("shift", key)) => (KeyModifiers::SHIFT, key),
        Some(_) => return None,
        None => (KeyModifiers::NONE, chord),
    };
    let code = match key {
        "enter" => KeyCode::Enter,
        "space" => KeyCode::Char(' '),
        "esc" => KeyCode::Esc,
        other => {
            let mut characters = other.chars();
            let first = characters.next()?;
            // A multi-character token is a word, not a key. A modified letter arrives from
            // the terminal in lower case however the label spells it (`Ctrl+B` → `ctrl+'b'`).
            let code = if modifiers.is_empty() { first } else { first.to_ascii_lowercase() };
            characters.next().is_none().then_some(KeyCode::Char(code))?
        }
    };
    Some(KeyEvent::new(code, modifiers))
}

#[test]
fn every_advertised_footer_chord_resolves_to_an_action() {
    // Every state the surface can be in: the guide shown and put away, at every width tier.
    // A chord the footer promises has to resolve in all of them, which is the invariant that
    // focus-conditional routing kept breaking.
    for guide_visible in [true, false] {
        for width in [80u16, 100, 120, 160, 200] {
            let scenario = FixtureScenario::GuidedReviewReady;
            let mut reference = seeded_app("nord", scenario).expect("fixture app");
            reference.update(Msg::TerminalResized { width, height: 44 });
            reference.guided_review_workspace.guide_visible = guide_visible;

            for segment in ready_footer_segments(&reference, width) {
                let Some(key) = chord_of(&segment.label) else { continue };
                // A fresh app per chord: several of these open a composer or move a pane, and
                // a shared one would test each key against the last key's aftermath.
                let mut app = seeded_app("nord", scenario).expect("fixture app");
                app.update(Msg::TerminalResized { width, height: 44 });
                app.guided_review_workspace.guide_visible = guide_visible;
                let routed = route_key(&mut app, &key);
                let inert = matches!(&routed, RouteAction::Msg(message) if matches!(**message, Msg::Tick))
                    || matches!(routed, RouteAction::None);
                assert!(
                    !inert,
                    "footer advertises {:?} with guide_visible={guide_visible} at {width} cols, but the router drops it ({routed:?})",
                    segment.label
                );
            }
        }
    }
}

/// `g` puts the guide away and brings it back, and the old chord still does too.
#[test]
fn g_toggles_the_guide() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    app.update(Msg::TerminalResized { width: 200, height: 30 });
    assert!(app.guided_review_workspace.guide_visible);
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE)));
    assert!(!app.guided_review_workspace.guide_visible, "g hides it");
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT)));
    assert!(app.guided_review_workspace.guide_visible, "alt+v brings it back");
}

/// The help overlay and the footer are the same contract said twice, so they must say it the
/// same way. They drifted once already: the footer said `f read all` while help said `f read
/// what is accounted for`, for a key that did neither.
#[test]
fn help_and_the_footer_name_the_same_keys() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    app.update(Msg::TerminalResized { width: 160, height: 44 });
    app.guided_review_workspace.toggle_help();

    let backend = ratatui::backend::TestBackend::new(160, 44);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| super::render(frame, frame.area(), &app)).expect("render help");
    let help: String = (0..44u16)
        .map(|y| {
            (0..160u16)
                .map(|x| terminal.backend().buffer()[(x, y)].symbol().to_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    for segment in ready_footer_segments(&app, 160) {
        let Some(chord) = segment.label.split_whitespace().next() else { continue };
        // Esc and `?` are the two universals; help names them in its own words.
        if matches!(chord, "esc" | "?") {
            continue;
        }
        assert!(
            help.contains(chord),
            "the footer offers {chord:?} but help never mentions it:\n{help}"
        );
    }
}
