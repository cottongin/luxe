//! Batches — coalesced input lands the way single events would.

use super::*;

/// Everything the terminal had queued has to reach the app, in order, before the frame that
/// answers it. Dropping one would lose a keystroke; drawing one frame each is the freeze.
#[test]
fn a_batch_of_keystrokes_all_land_in_the_composer() {
    let mut app = seeded_app("dark", FixtureScenario::ChatIdle).expect("fixture");
    let mut exit = TuiExit::Quit;
    for character in "coalesced".chars() {
        let event = Event::Key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        assert!(!apply_terminal_event(&mut app, event, &mut exit), "a key is not idle motion");
    }
    assert_eq!(app.editor.current_input(), "coalesced");
}

/// A batch skips its frame only if *every* event in it resolved to nothing, which is what
/// keeps a pointer crossing dead space from relaying out the transcript.
#[test]
fn a_batch_is_idle_only_when_all_of_it_is() {
    let mut app = seeded_app("dark", FixtureScenario::ChatIdle).expect("fixture");
    app.mouse_enabled = true;
    let mut exit = TuiExit::Quit;
    let motion = |column| {
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column,
            row: 0,
            modifiers: KeyModifiers::NONE,
        })
    };

    let mut idle = apply_terminal_event(&mut app, motion(1), &mut exit);
    idle &= apply_terminal_event(&mut app, motion(2), &mut exit);
    assert!(idle, "motion over dead space costs no frame");

    idle &= apply_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
        &mut exit,
    );
    assert!(!idle, "one real event in the batch earns the frame");
}

/// Reasoning the reader can see is prose: a click on it selects, and must never fold it away
/// under the pointer. Only the folded peek is a control, and it opens the same way.
///
/// Driven through a real frame because that is where the answer lives — the render publishes a
/// click region per row that offers one, so a row that stopped being a handle stops having one.
#[test]
fn a_click_folds_reasoning_only_where_reasoning_is_already_folded() {
    use crate::app::ConvoBlock;
    use crossterm::event::MouseEvent;
    use ratatui::{backend::TestBackend, Terminal};

    let (width, height) = (140u16, 44u16);
    let mut app = seeded_app("nord", FixtureScenario::ChatThinking).expect("chat fixture");
    app.update(Msg::TerminalResized { width, height });

    // Where a phrase is drawn, as the cell a pointer would be over.
    let find = |app: &mut crate::app::App, phrase: &str| -> (u16, u16) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a terminal");
        terminal.draw(|frame| crate::view::render(frame, app)).expect("a frame");
        let buffer = terminal.backend().buffer().clone();
        for y in 0..height {
            let row: String = (0..width).map(|x| buffer[(x, y)].symbol().to_string()).collect();
            if let Some(column) = row.find(phrase) {
                return (u16::try_from(column).expect("a column") + 1, y);
            }
        }
        panic!("{phrase:?} is not on screen");
    };
    let click = |app: &mut crate::app::App, (column, row): (u16, u16)| {
        let mut exit = TuiExit::Quit;
        for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)]
        {
            let event = MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE };
            let action = route_mouse(app, &event);
            apply_route_action(app, action, &mut exit);
        }
    };
    let shown = |app: &crate::app::App, index: usize| {
        matches!(app.convo[index].block(), ConvoBlock::Thinking { expanded: true, .. })
    };

    // The fixture's first reasoning block is shown in full; its first row used to fold it.
    let expanded = app
        .convo
        .iter()
        .position(|item| matches!(item.block(), ConvoBlock::Thinking { expanded: true, .. }))
        .expect("the fixture shows reasoning");
    for phrase in ["Tracing the buffering", "The buffering happens in"] {
        let at = find(&mut app, phrase);
        click(&mut app, at);
        assert!(shown(&app, expanded), "a click on {phrase:?} folded reasoning away");
    }

    // The peek is the one row here that is a control, and one click is all it takes.
    let folded = app
        .convo
        .iter()
        .position(|item| matches!(item.block(), ConvoBlock::Thinking { expanded: false, .. }))
        .expect("the fixture folds one");
    let at = find(&mut app, "Planning the test");
    click(&mut app, at);
    assert!(shown(&app, folded), "a click on the peek did not open it");
}

/// The two single-slot side effects the loop body reads once an iteration. A batch that
/// produced one has to stop, or the next event overwrites what nothing has collected yet.
#[test]
fn a_batch_stops_at_the_side_effects_the_loop_reads_once() {
    let mut app = seeded_app("dark", FixtureScenario::ChatIdle).expect("fixture");
    assert!(may_coalesce(&app, 0));
    assert!(!may_coalesce(&app, MAX_COALESCED_INPUT), "the cap holds");

    app.outbox.copy = Some("copied".into());
    assert!(!may_coalesce(&app, 0), "a copy is waiting to be handed to the clipboard");
    app.outbox.copy = None;

    app.outbox.clipboard_paste = true;
    assert!(!may_coalesce(&app, 0), "a paste read is waiting to be spawned");
    app.outbox.clipboard_paste = false;

    app.should_quit = true;
    assert!(!may_coalesce(&app, 0), "nothing left to apply them to");
}
