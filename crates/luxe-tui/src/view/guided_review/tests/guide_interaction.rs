//! The guide is a document you can point at, not a page of paint.
//!
//! Reported from a real session: the marker never appeared, `Enter` did nothing, and clicking a
//! row did nothing — three separate breaks that added up to "I can't navigate the walkthrough".
//! Two of the three were focus-conditional routing; the surface no longer has a focus to condition
//! on, and these are what say so.

use crate::app::Msg;
use crate::fixture::{seeded_app, FixtureScenario};
use crate::main_loop::{route_key, route_mouse, RouteAction};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{backend::TestBackend, Terminal};

fn review_app(scenario: FixtureScenario, width: u16, height: u16) -> crate::app::App {
    let mut app = seeded_app("nord", scenario).expect("review fixture");
    app.update(Msg::TerminalResized { width, height });
    app
}

fn draw(app: &mut crate::app::App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal.draw(|frame| crate::view::render(frame, app)).expect("render");
    terminal.backend().buffer().content.iter().map(ratatui::buffer::Cell::symbol).collect()
}

/// A slab fades with the surface it sits on.
///
/// Style patching touches the foreground and leaves the background alone, so a section heading
/// kept its undiluted accent fill behind a modal while the triangle caps either side of it —
/// foreground glyphs — dimmed correctly. The heading was then the brightest thing on a
/// backgrounded surface, and the caps looked detached from a badge that had not moved.
#[test]
fn a_section_heading_dims_with_everything_else_behind_a_modal() {
    let ramp = {
        let app = review_app(FixtureScenario::GuidedReviewDemo, 180, 44);
        app.theme_styles.slab.ramp()
    };

    let slab_cells = |app: &mut crate::app::App| {
        let mut terminal = Terminal::new(TestBackend::new(180, 44)).expect("test terminal");
        terminal.draw(|frame| crate::view::render(frame, app)).expect("render");
        terminal.backend().buffer().content.iter().filter(|cell| ramp.contains(&cell.bg)).count()
    };

    let mut app = review_app(FixtureScenario::GuidedReviewDemo, 180, 44);
    let bright = slab_cells(&mut app);
    assert!(bright > 0, "the surface draws slab headings when nothing is over it");

    app.guided_review_workspace.help_visible = true;
    assert_eq!(
        slab_cells(&mut app),
        0,
        "no slab fill survives a modal — chrome recedes with the surface it is chrome for",
    );
}

/// The guide opens on the first chapter and marks it, so the document has a place in it from
/// the moment it is drawn.
#[test]
fn the_guide_opens_on_the_first_chapter() {
    let mut app = review_app(FixtureScenario::GuidedReviewDemo, 180, 44);
    let content = draw(&mut app, 180, 44);
    let first = app.guided_review.as_ref().expect("a review").chapters[0].id.clone();
    assert_eq!(app.guided_review_workspace.position.chapter_id, Some(first));
    assert!(content.contains('›'), "and says so on the row: {content}");
}

/// `↑`/`↓` scroll what is being read, in every state the surface can be in. They used to mean
/// one thing or the other depending on which pane held focus, and on the surface that needed
/// them most they meant nothing at all.
#[test]
fn the_arrows_always_scroll_the_diff() {
    for guide_visible in [true, false] {
        let mut app = review_app(FixtureScenario::GuidedReviewLarge, 180, 44);
        app.guided_review_workspace.guide_visible = guide_visible;
        draw(&mut app, 180, 44);
        let before = app.guided_review_workspace.position.line_offset;

        app.update(Msg::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)));
        assert!(
            app.guided_review_workspace.position.line_offset > before,
            "guide_visible={guide_visible}: ↓ moves the reading"
        );
        app.update(Msg::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)));
        assert_eq!(app.guided_review_workspace.position.line_offset, before);
    }
}

/// Stepping a chapter moves the one selection, so both panes follow it together.
#[test]
fn stepping_a_chapter_moves_the_one_selection() {
    let mut app = review_app(FixtureScenario::GuidedReviewDemo, 180, 44);
    draw(&mut app, 180, 44);
    let review = app.guided_review.clone().expect("a review");
    assert_eq!(
        app.guided_review_workspace.position.chapter_id,
        Some(review.chapters[0].id.clone())
    );

    app.update(Msg::Key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE)));
    assert_eq!(
        app.guided_review_workspace.position.chapter_id,
        Some(review.chapters[1].id.clone()),
        "] moves the chapter"
    );
    // And the diff follows: the file on screen belongs to the chapter the guide now marks.
    let file = app.guided_review_workspace.position.file_id.clone().expect("a file");
    let reach = &review.chapters[1];
    let owner = review.files.iter().find(|candidate| candidate.id == file).expect("the file");
    assert!(
        reach.items.iter().any(|item| match item {
            luxe_core::guided_review::ReviewItemRef::File(id) => *id == owner.id,
            luxe_core::guided_review::ReviewItemRef::Hunk(id) =>
                owner.hunks.iter().any(|hunk| hunk.id == *id),
        }),
        "the narration on screen owns the diff on screen"
    );
}

/// Clicking a chapter row in the guide points the review at it.
#[test]
fn clicking_a_guide_chapter_opens_it() {
    let mut app = review_app(FixtureScenario::GuidedReviewDemo, 180, 44);
    draw(&mut app, 180, 44);

    let (rect, chapter_id) = app
        .hit
        .borrow()
        .clicks()
        .find_map(|(rect, target)| match target {
            crate::mouse::MouseTarget::ReviewGuideChapter(id) => Some((rect, id.clone())),
            _ => None,
        })
        .expect("the guide publishes its chapter rows as click targets");
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + 1,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    };
    let routed = route_mouse(&mut app, &click);
    assert!(
        matches!(&routed, RouteAction::Msg(message)
            if matches!(&**message, Msg::GuidedReviewOpenChapter(id) if *id == chapter_id)),
        "clicking a chapter opens it: {routed:?}"
    );
}

/// `Alt+V` puts the guide away and brings it back, and the diff takes the room.
#[test]
fn the_guide_can_be_put_away() {
    let mut app = review_app(FixtureScenario::GuidedReviewDemo, 180, 44);
    let with_guide = draw(&mut app, 180, 44);
    assert!(with_guide.contains("REVIEW"), "the guide names itself: {with_guide}");

    let routed = route_key(&mut app, &KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT));
    assert!(
        !matches!(&routed, RouteAction::Msg(message) if matches!(**message, Msg::Tick)),
        "alt+v is routed: {routed:?}"
    );
    app.update(Msg::GuidedReviewToggleGuide);
    let without = draw(&mut app, 180, 44);
    assert!(!without.contains("REVIEW"), "the guide is away: {without}");
    assert!(without.contains("alt+v shows the guide"), "and says how to bring it back");
}
