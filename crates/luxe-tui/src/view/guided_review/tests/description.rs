//! A pull request's whole description reads over the review, rather than only on GitHub.

use crate::app::{App, Msg};
use crate::fixture::{seeded_app, FixtureScenario};
use crate::main_loop::{route_key, route_mouse, RouteAction};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::{backend::TestBackend, Terminal};

const WIDTH: u16 = 160;
const HEIGHT: u16 = 40;

/// The pull request fixture with a description long enough to need scrolling, its last line
/// marked so a test can tell whether it was ever reachable.
fn long_description_app() -> App {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewPullRequest).expect("fixture");
    let body =
        (1..=80).map(|n| format!("Paragraph {n} of the author's account.")).collect::<Vec<_>>();
    let body = format!("{}\n\nTHE LAST LINE", body.join("\n\n"));
    pull_request_mut(&mut app).body = body;
    app.update(Msg::TerminalResized { width: WIDTH, height: HEIGHT });
    app
}

fn pull_request_mut(app: &mut App) -> &mut luxe_core::guided_review::PullRequestContext {
    let review = app.guided_review.as_mut().expect("a review");
    review.context.pull_request.as_mut().expect("a pull request")
}

fn draw(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("test terminal");
    terminal.draw(|frame| crate::view::render(frame, app)).expect("render");
    terminal.backend().buffer().content.iter().map(ratatui::buffer::Cell::symbol).collect()
}

fn press(app: &mut App, code: KeyCode) {
    if let RouteAction::Msg(message) = route_key(app, &KeyEvent::new(code, KeyModifiers::NONE)) {
        app.update(*message);
    }
}

/// The guide points at the reader, `p` opens it at the top, it scrolls to the very end, and `esc`
/// shuts it without leaving the review.
#[test]
fn p_opens_the_whole_description_and_it_scrolls_to_the_end() {
    let mut app = long_description_app();
    let guide = draw(&mut app);
    assert!(guide.contains("Read the whole description"), "the guide names the key");
    assert!(!guide.contains("THE LAST LINE"));

    press(&mut app, KeyCode::Char('p'));
    assert_eq!(app.guided_review_workspace.description_scroll, Some(0));
    let open = draw(&mut app);
    assert!(open.contains("Paragraph 1 of the author"), "opens at the top");
    assert!(!open.contains("THE LAST LINE"), "too long to fit, or the test proves nothing");

    press(&mut app, KeyCode::End);
    assert!(draw(&mut app).contains("THE LAST LINE"), "every line is reachable");
    let room = app.guided_review_workspace.description_room.get();
    assert_eq!(app.guided_review_workspace.description_scroll, Some(room), "and stops there");

    press(&mut app, KeyCode::Esc);
    assert_eq!(app.guided_review_workspace.description_scroll, None);
    assert!(app.guided_review.is_some(), "esc shut the description, not the review");
}

/// While it is open the review behind does not move: a letter that would step the walkthrough is
/// swallowed, and the wheel scrolls the description wherever the pointer is.
#[test]
fn the_review_behind_the_description_stays_put() {
    let mut app = long_description_app();
    draw(&mut app);
    press(&mut app, KeyCode::Char('p'));
    draw(&mut app);
    let position = app.guided_review_workspace.position.clone();
    press(&mut app, KeyCode::Char('n'));
    assert_eq!(app.guided_review_workspace.position, position, "n stepped the walkthrough");

    let wheel = MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 1,
        row: 1,
        modifiers: KeyModifiers::NONE,
    };
    match route_mouse(&mut app, &wheel) {
        RouteAction::Msg(message) => app.update(*message),
        other => panic!("the wheel went elsewhere: {other:?}"),
    }
    assert_eq!(app.guided_review_workspace.description_scroll, Some(3));
}

/// A review with no description has nothing to open, so `p` leaves it as it was.
#[test]
fn p_does_nothing_without_a_description() {
    let mut app = long_description_app();
    pull_request_mut(&mut app).body = "  ".into();
    draw(&mut app);
    press(&mut app, KeyCode::Char('p'));
    assert_eq!(app.guided_review_workspace.description_scroll, None);
}
