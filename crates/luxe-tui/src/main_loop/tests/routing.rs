//! Routing — keys and the mouse reach the surface they belong to.

use super::*;

/// What the app parks for the loop becomes the loop's exit, whichever event parked it — the
/// worktree removal is parked by an engine answer, with no key press behind it.
#[test]
fn a_parked_exit_is_collected_without_waiting_for_a_key() {
    let mut app = seeded_app("nord", FixtureScenario::ChatIdle).expect("fixture");
    let mut exit = TuiExit::Quit;
    collect_exit(&mut app, &mut exit);
    assert!(matches!(exit, TuiExit::Quit) && !app.should_quit, "nothing parked, nothing happens");

    app.outbox.worktree_removal = Some(std::path::PathBuf::from("/work/app/.luxe/worktrees/x"));
    collect_exit(&mut app, &mut exit);
    assert!(app.should_quit);
    assert!(matches!(exit, TuiExit::RemoveWorktree(ref path) if path.ends_with("worktrees/x")));
    assert_eq!(app.outbox.worktree_removal, None);
}

/// `⌥O` from Ghostty on a Mac's "ABC" layout arrives as `Alt+ø`, and it opens the preview's
/// file the same as `Alt+O` does. It used to do nothing at all, while `⌥E`, which types no character,
/// worked as `Alt+E`.
#[test]
fn an_option_layer_key_reaches_the_chord_it_was_pressed_as() {
    let mut app = seeded_app("luxe", FixtureScenario::ChangesWide).expect("the fixture");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("root");
    app.project_root = Some(root);
    let mut exit = TuiExit::Quit;
    let option_o = KeyEvent::new(KeyCode::Char('\u{f8}'), KeyModifiers::ALT);
    apply_terminal_event(&mut app, Event::Key(option_o), &mut exit);
    assert!(app.outbox.editor_open.take().is_some(), "Peek's open-in-editor ran");

    // Typed without Alt, the same character is someone writing Norwegian
    let typed = KeyEvent::new(KeyCode::Char('\u{f8}'), KeyModifiers::NONE);
    apply_terminal_event(&mut app, Event::Key(typed), &mut exit);
    assert!(app.outbox.editor_open.is_none());
    assert!(app.editor.current_input().ends_with('\u{f8}'), "and it lands in the composer");
}

/// Every resizable pane resizes the same way: press its divider, drag, let go. The pane follows
/// the pointer, stops at its bounds, and comes back at the same width the next time it is
/// drawn; a double-click puts it back to its own width.
#[test]
fn a_dragged_divider_resizes_its_pane_and_a_double_click_resets_it() {
    use crate::workspace_layout::Pane;
    use crossterm::event::MouseEvent;
    use ratatui::{backend::TestBackend, Terminal};

    let (width, height) = (200u16, 44u16);
    let draw = |app: &mut crate::app::App| {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a terminal");
        terminal.draw(|frame| crate::view::render(frame, app)).expect("a frame");
    };
    let divider = |app: &crate::app::App, pane: Pane| {
        app.hit.borrow().clicks().find_map(|(rect, target)| match target {
            crate::mouse::MouseTarget::Divider(handle) if handle.pane == pane => {
                Some((rect.x, rect.y, *handle))
            }
            _ => None,
        })
    };
    let mouse = |app: &mut crate::app::App, kind: MouseEventKind, column: u16, row: u16| {
        let mut exit = TuiExit::Quit;
        let action = route_mouse_at(
            app,
            &MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE },
            std::time::Instant::now(),
        );
        apply_route_action(app, action, &mut exit);
    };

    for (scenario, pane) in [
        (FixtureScenario::DockWide, Pane::Dock),
        (FixtureScenario::GuidedReviewWide, Pane::ReviewGuide),
        (FixtureScenario::ChangesWide, Pane::Peek),
    ] {
        let mut app = seeded_app("nord", scenario).expect("the fixture");
        app.update(Msg::TerminalResized { width, height });
        draw(&mut app);
        let (x, y, handle) = divider(&app, pane).unwrap_or_else(|| panic!("{pane:?} divider"));
        let before = handle.width_at(x);

        // Five columns toward the pane's own side narrows it by five
        let toward = if handle.pane_on_left { x - 5 } else { x + 5 };
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
        assert!(app.pointer.selection_drag.is_none(), "{pane:?}: a divider grab selects nothing");
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), toward, y);
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), toward, y);
        assert_eq!(app.panes.width(pane), Some(before - 5), "{pane:?}");
        draw(&mut app);
        let (moved, _, handle) = divider(&app, pane).expect("still there");
        assert_eq!(moved, toward, "{pane:?}: the rule is drawn where it was let go");
        assert_eq!(handle.width_at(moved), before - 5);

        // Dragged far past its bound, it stops there
        let far = if handle.pane_on_left { 0 } else { width - 1 };
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), moved, y);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), far, y);
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), far, y);
        assert_eq!(app.panes.width(pane), Some(handle.min), "{pane:?}");

        // Two quick presses put it back
        draw(&mut app);
        let (x, y, _) = divider(&app, pane).expect("still there");
        for _ in 0..2 {
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            mouse(&mut app, MouseEventKind::Up(MouseButton::Left), x, y);
        }
        assert_eq!(app.panes.width(pane), None, "{pane:?}: back to its own width");
        draw(&mut app);
        let (x, _, handle) = divider(&app, pane).expect("still there");
        assert_eq!(handle.width_at(x), before, "{pane:?}");
    }
}

/// A divider lights under the pointer, the way a multiplexer shows the border it is about to
/// move, and goes back to the muted rule when the pointer leaves.
#[test]
fn a_divider_lights_under_the_pointer() {
    use crossterm::event::MouseEvent;
    use ratatui::{backend::TestBackend, Terminal};

    let (width, height) = (200u16, 44u16);
    let mut app = seeded_app("nord", FixtureScenario::DockWide).expect("the fixture");
    app.update(Msg::TerminalResized { width, height });
    let ink_at = |app: &mut crate::app::App, x: u16, y: u16| {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a terminal");
        terminal.draw(|frame| crate::view::render(frame, app)).expect("a frame");
        terminal.backend().buffer()[(x, y)].fg
    };
    let _ = ink_at(&mut app, 0, 0);
    let (x, y) = app
        .hit
        .borrow()
        .clicks()
        .find_map(|(rect, target)| {
            matches!(target, crate::mouse::MouseTarget::Divider(_)).then_some((rect.x, rect.y))
        })
        .expect("the dock's divider");
    let y = y + 2;
    let resting = ink_at(&mut app, x, y);
    let hover = |app: &mut crate::app::App, column: u16| {
        let mut exit = TuiExit::Quit;
        let event = MouseEvent {
            kind: MouseEventKind::Moved,
            column,
            row: y,
            modifiers: KeyModifiers::NONE,
        };
        let action = route_mouse(app, &event);
        apply_route_action(app, action, &mut exit);
    };
    hover(&mut app, x);
    let lit = ink_at(&mut app, x, y);
    assert_eq!(Some(lit), app.theme_styles.border_accent.fg);
    assert_ne!(lit, resting);
    hover(&mut app, x - 20);
    assert_eq!(ink_at(&mut app, x, y), resting);
}

/// The rule the first run turns on. A dashboard says what is missing and where the fix is, so
/// setup only takes the screen when there is no dashboard to say it.
#[test]
fn setup_takes_the_screen_only_when_no_dashboard_can_point_at_it() {
    assert_eq!(startup_surface(false, false, false), StartupSurface::Configuration);
    assert_eq!(
        startup_surface(false, true, false),
        StartupSurface::AsGiven,
        "the dashboard leads with the setup row instead"
    );
    assert_eq!(
        startup_surface(true, false, false),
        StartupSurface::AsGiven,
        "a configured machine goes straight to work"
    );
    assert_eq!(
        startup_surface(false, false, true),
        StartupSurface::AsGiven,
        "headless has nobody to show a surface to"
    );
}

/// The first-run questions are for a new machine. Each sign of an earlier install, on its own,
/// keeps them away; so does a terminal nobody is watching.
#[test]
fn the_first_run_is_skipped_by_any_sign_of_an_earlier_install() {
    assert!(first_run_from(true, false, false), "a fresh, watched terminal is asked");
    assert!(!first_run_from(true, true, false), "a config file means it ran before");
    assert!(
        !first_run_from(true, false, true),
        "a stored login means it ran before, config file or not"
    );
    assert!(!first_run_from(false, false, false), "nobody is there to answer");
}

/// Every Configuration row is the app's to apply except the editor handoff, which needs the
/// terminal back — so that one, and only that one, ends the TUI.
#[test]
fn choosing_a_config_file_hands_the_terminal_to_the_editor() {
    let mut app = seeded_app("nord", FixtureScenario::Configuration).expect("the fixture");
    let path = std::path::PathBuf::from("/home/dev/.config/luxe/models.json");
    let mut exit = TuiExit::Quit;

    apply_route_action(
        &mut app,
        RouteAction::Configuration(crate::configuration::ConfigOutcome::EditFile(path.clone())),
        &mut exit,
    );
    assert_eq!(exit, TuiExit::OpenInEditor(path));
    assert!(app.should_quit, "the editor needs the terminal, so the TUI stands down");

    // Signing in stays inside the surface.
    let mut app = seeded_app("nord", FixtureScenario::Configuration).expect("the fixture");
    let mut exit = TuiExit::Quit;
    apply_route_action(
        &mut app,
        RouteAction::Configuration(crate::configuration::ConfigOutcome::SignIn(
            luxe_core::auth::LoginScope::Subscription,
        )),
        &mut exit,
    );
    assert!(!app.should_quit);
    assert_eq!(exit, TuiExit::Quit);
    assert!(app.overlays.auth_wizard.is_some(), "the row opens its list of providers");
}

/// While the wizard or the model picker is up, the surface's own keys are theirs — `q` in a
/// menu means quit, and in a login it means the letter q.
#[test]
fn the_configuration_menu_hands_its_keys_to_whatever_it_opened() {
    let mut app = seeded_app("nord", FixtureScenario::Configuration).expect("the fixture");
    assert!(matches!(
        route_key(&mut app, &KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE)),
        RouteAction::Configuration(crate::configuration::ConfigOutcome::SignIn(_))
    ));

    app.open_auth_wizard_scoped(None, luxe_core::auth::LoginScope::ApiKey);
    assert!(matches!(
        route_key(&mut app, &KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
        RouteAction::Msg(message) if matches!(*message, Msg::Key(_))
    ));
}

#[test]
fn configured_review_binding_routes_through_the_keymap() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("review fixture");
    let first_hunk = app.guided_review_workspace.position.hunk_id.clone();
    app.keymap = app.keymap.clone().with_overrides(HashMap::from([(
        "tui.review.hunk_next".to_string(),
        vec!["f2".to_string()],
    )]));

    let route = route_key(&mut app, &KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
    assert!(matches!(route, RouteAction::Msg(ref message) if matches!(**message, Msg::Key(_))));
    if let RouteAction::Msg(message) = route {
        app.update(*message);
    }
    assert_ne!(app.guided_review_workspace.position.hunk_id, first_hunk);
    assert_eq!(UiAction::ReviewHunkNext.id(), "tui.review.hunk_next");
}

/// While a comment is open, every keystroke belongs to it.
///
/// Typing a sentence into the composer used to lose most of it: the review router resolved
/// `tui.review.*` bindings *before* the draft was consulted, so a space stepped to the next
/// hunk, an `f` opened the collapsed churn, and only the letters bound to nothing survived.
#[test]
fn a_typed_sentence_reaches_the_comment_and_nothing_else() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("review fixture");
    app.update(Msg::TerminalResized { width: 170, height: 44 });
    let before = app.guided_review_workspace.position.clone();

    app.update(Msg::GuidedReviewBeginFinding);

    let sentence = "why not serde_json here? a hand-rolled escape is easy to get wrong";
    for character in sentence.chars() {
        let key = KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE);
        match route_key(&mut app, &key) {
            RouteAction::Msg(message) => {
                assert!(
                    matches!(*message, Msg::Key(_)),
                    "{character:?} routed as a review command while a comment was open"
                );
                app.update(*message);
            }
            other => panic!("{character:?} left the composer: {other:?}"),
        }
    }

    let draft = app.guided_review_workspace.finding_draft.as_ref().expect("the draft is open");
    assert_eq!(draft.body(), sentence, "every character landed in the comment");
    assert_eq!(before, app.guided_review_workspace.position, "and the cursor never moved");
}

#[test]
fn wheel_scrolls_the_current_surface() {
    use crossterm::event::MouseEvent;
    let wheel = |kind| MouseEvent { kind, column: 5, row: 5, modifiers: KeyModifiers::NONE };

    // Chat: the wheel scrolls the conversation, three lines per notch by default — one line a
    // notch made paging a long transcript a chore.
    let mut chat = seeded_app("nord", FixtureScenario::ChatIdle).expect("chat fixture");
    assert!(matches!(
        route_mouse(&mut chat, &wheel(MouseEventKind::ScrollUp)),
        RouteAction::Scroll(ScrollAction::Up(3))
    ));
    assert!(matches!(
        route_mouse(&mut chat, &wheel(MouseEventKind::ScrollDown)),
        RouteAction::Scroll(ScrollAction::Down(3))
    ));
    // And it follows the configured step.
    chat.config.conversation.scroll_lines = Some(7);
    assert!(matches!(
        route_mouse(&mut chat, &wheel(MouseEventKind::ScrollUp)),
        RouteAction::Scroll(ScrollAction::Up(7))
    ));
    // Pointer motion over nothing clickable changes nothing.
    assert!(matches!(route_mouse(&mut chat, &wheel(MouseEventKind::Moved)), RouteAction::None));

    // Guided Review: the wheel scrolls the diff.
    let mut review =
        seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("review fixture");
    assert!(matches!(
        route_mouse(&mut review, &wheel(MouseEventKind::ScrollUp)),
        RouteAction::Msg(message) if matches!(*message, Msg::GuidedReviewLinePrevious)
    ));
    assert!(matches!(
        route_mouse(&mut review, &wheel(MouseEventKind::ScrollDown)),
        RouteAction::Msg(message) if matches!(*message, Msg::GuidedReviewLineNext)
    ));
}

/// The scroll indicator is a control: pressing the track jumps to that fraction of the
/// transcript, dragging keeps steering even once the pointer leaves the one-cell lane, and
/// release lets go. A press on it must not also start a text selection.
#[test]
fn the_conversation_scrollbar_can_be_grabbed_and_dragged() {
    use crossterm::event::MouseEvent;
    use ratatui::layout::Rect;

    let mut app = seeded_app("nord", FixtureScenario::ChatIdle).expect("chat fixture");
    // A transcript taller than its viewport, and a published 20-row track at column 99.
    app.scroll_state.viewport = 10;
    app.scroll_state.total = 210;
    app.scroll_state.offset = 0;
    app.scroll_state.follow = false;
    app.hit.borrow_mut().push(
        Rect::new(99, 4, 1, 20),
        crate::mouse::MouseTarget::Scrollbar {
            region: crate::mouse::ScrollRegionId::Conversation,
            y: 4,
            height: 20,
        },
    );

    let press = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 99,
        // Halfway down the track.
        row: 4 + 9,
        modifiers: KeyModifiers::NONE,
    };
    let mut exit = TuiExit::Quit;
    let action = route_mouse(&mut app, &press);
    assert!(matches!(action, RouteAction::Scroll(ScrollAction::DragTo { row: 9, height: 20 })));
    apply_route_action(&mut app, action, &mut exit);
    assert_eq!(app.scroll_state.offset, 95, "the grab lands where it points");
    let grabbed = Some((crate::mouse::ScrollRegionId::Conversation, 4, 20));
    assert_eq!((app.pointer.selection_drag, app.pointer.scrollbar_drag), (None, grabbed));

    // Dragging past the lane keeps steering: the pointer strays sideways, the bar still follows.
    let drag = MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 40,
        row: 4 + 19,
        modifiers: KeyModifiers::NONE,
    };
    let action = route_mouse(&mut app, &drag);
    apply_route_action(&mut app, action, &mut exit);
    assert_eq!(app.scroll_state.offset, app.scroll_state.max_offset(), "dragged to the end");
    assert!(app.scroll_state.follow, "and the transcript follows again at the bottom");

    let release = MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 40,
        row: 23,
        modifiers: KeyModifiers::NONE,
    };
    assert!(matches!(route_mouse(&mut app, &release), RouteAction::None));
    assert!(app.pointer.scrollbar_drag.is_none(), "release lets go of the bar");
}

/// The Changes pane's bar is grabbed like every other: drawn, its lane is published, and a
/// press halfway down puts the diff halfway through.
#[test]
fn the_changes_scrollbar_can_be_grabbed() {
    use crossterm::event::MouseEvent;
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::ChangesWide).expect("changes fixture");
    let mut terminal = Terminal::new(TestBackend::new(140, 20)).expect("a terminal");
    terminal.draw(|frame| crate::view::render(frame, &mut app)).expect("a frame");
    let lane = app.hit.borrow().clicks().find_map(|(rect, target)| match target {
        crate::mouse::MouseTarget::Scrollbar {
            region: crate::mouse::ScrollRegionId::Changes,
            y,
            height,
        } => Some((rect.x, *y, *height)),
        _ => None,
    });
    let (column, y, height) = lane.expect("the diff overflows twenty rows, so it has a bar");
    let max = app.changes.as_ref().expect("a changes pane").scroll_max.get();
    assert!(max > 0);

    let press = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row: y + height - 1,
        modifiers: KeyModifiers::NONE,
    };
    let action = route_mouse(&mut app, &press);
    let mut exit = TuiExit::Quit;
    apply_route_action(&mut app, action, &mut exit);
    assert_eq!(app.changes.as_ref().expect("still open").scroll, max, "the bottom of the track");
}

/// A bar means the same thing on every surface: the dock's lists are grabbed and dragged the
/// way the transcript is, and the press steers the list the track belongs to rather than the
/// conversation behind it.
#[test]
fn a_dock_list_scrollbar_can_be_grabbed_and_dragged() {
    use crate::workspace_dock::{DockList, DockListMetrics};
    use crossterm::event::MouseEvent;
    use ratatui::layout::Rect;

    let mut app = seeded_app("nord", FixtureScenario::ChatIdle).expect("chat fixture");
    let region = crate::mouse::ScrollRegionId::DockList(DockList::Files);
    let metrics = DockListMetrics { max: 40, follow: 0 };
    app.set_dock_list_metrics(DockList::Files, metrics);
    app.hit.borrow_mut().push(
        Rect::new(40, 6, 1, 9),
        crate::mouse::MouseTarget::Scrollbar { region, y: 6, height: 9 },
    );

    let at = |row| MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 40,
        row,
        modifiers: KeyModifiers::NONE,
    };
    let mut exit = TuiExit::Quit;
    let action = route_mouse(&mut app, &at(6 + 4));
    apply_route_action(&mut app, action, &mut exit);
    assert_eq!(app.dock.list_scroll(DockList::Files, metrics), 20, "halfway down the list");
    assert_eq!(app.scroll_state.offset, 0, "and the transcript did not move");
    assert_eq!(app.pointer.scrollbar_drag, Some((region, 6, 9)));

    let drag = MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 12,
        row: 6 + 8,
        modifiers: KeyModifiers::NONE,
    };
    let action = route_mouse(&mut app, &drag);
    apply_route_action(&mut app, action, &mut exit);
    assert_eq!(app.dock.list_scroll(DockList::Files, metrics), 40, "dragged to the end");
}
