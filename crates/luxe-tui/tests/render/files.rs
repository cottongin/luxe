//! Files — the project tree, the file view and the start screen's flyouts.

use super::*;

/// The next action the app sends that is not the tree asking for a folder.
pub(super) fn next_action_but_listings(
    rx: &mut tokio::sync::mpsc::Receiver<luxe_core::engine::Action>,
) -> luxe_core::engine::Action {
    loop {
        match rx.try_recv().expect("an action was sent") {
            luxe_core::engine::Action::ListDirectory { .. } => continue,
            action => return action,
        }
    }
}

/// Opens `path` from the tree as an unchanged file, and lands `text` as what the engine read.
pub(super) fn open_file_view(
    app: &mut App,
    rx: &mut tokio::sync::mpsc::Receiver<luxe_core::engine::Action>,
    path: &str,
    text: &str,
) {
    app.update(Msg::OpenProjectFile(path.to_string()));
    let request_id = match next_action_but_listings(rx) {
        luxe_core::engine::Action::ReadProjectFile { request_id, path: asked } => {
            assert_eq!(asked, path, "it reads the file it is showing");
            request_id
        }
        other => panic!("an unchanged file is read, not diffed: {other:?}"),
    };
    app.update(Msg::Engine(luxe_core::event::Event::ProjectFileRead {
        request_id,
        file: Box::new(luxe_core::project_files::ProjectFile {
            path: path.to_string(),
            text: text.to_string(),
            bytes: text.len() as u64,
            truncated: false,
            binary: false,
        }),
    }));
}

/// A workspace arriving is what asks for the project root, and a listing that lands is drawn
/// beside the changes — the tree is the project, not only the part of it that changed.
#[test]
fn the_tree_lists_the_project_beside_its_changes() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.update(Msg::Engine(luxe_core::event::Event::WorkspaceUpdated(Box::new(
        changed_workspace("tree-listing", 1),
    ))));
    let request_id = match rx.try_recv().expect("the root is asked for") {
        luxe_core::engine::Action::ListDirectory { request_id, path, .. } => {
            assert_eq!(path, "", "the project root first");
            request_id
        }
        other => panic!("expected a listing request, got {other:?}"),
    };
    let entry = |name: &str, kind| luxe_core::project_files::DirectoryEntry {
        name: name.into(),
        kind,
        ignored: false,
    };
    use luxe_core::project_files::EntryKind;
    app.update(Msg::Engine(luxe_core::event::Event::DirectoryListed {
        request_id,
        listing: luxe_core::project_files::DirectoryListing {
            path: String::new(),
            entries: vec![
                entry("docs", EntryKind::Directory),
                entry("src", EntryKind::Directory),
                entry("Cargo.toml", EntryKind::File),
            ],
            omitted: 0,
        },
    }));
    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(content.contains("Cargo.toml"), "an unchanged file is in the tree: {content}");
    assert!(content.contains("docs"), "and a folder nothing changed in: {content}");
    assert!(content.contains("file00.rs"), "and the change, revealed: {content}");
}

/// Every listing request waiting in the queue, as (request, folder).
fn listing_requests(
    rx: &mut tokio::sync::mpsc::Receiver<luxe_core::engine::Action>,
) -> Vec<(luxe_core::project_files::ListingRequestId, String)> {
    let mut asked = Vec::new();
    while let Ok(action) = rx.try_recv() {
        if let luxe_core::engine::Action::ListDirectory { request_id, path, .. } = action {
            asked.push((request_id, path));
        }
    }
    asked
}

/// Lands `folders` as the listing of `path`, every entry a folder.
fn land_folders(
    app: &mut App,
    request_id: luxe_core::project_files::ListingRequestId,
    path: &str,
    folders: &[String],
) {
    use luxe_core::project_files::{DirectoryEntry, DirectoryListing, EntryKind};
    let entries = folders
        .iter()
        .map(|name| DirectoryEntry {
            name: name.clone(),
            kind: EntryKind::Directory,
            ignored: false,
        })
        .collect();
    let listing = DirectoryListing { path: path.into(), entries, omitted: 0 };
    app.update(Msg::Engine(luxe_core::event::Event::DirectoryListed { request_id, listing }));
}

/// A change in many folders asks for their listings a few at a time, and the rest follow as
/// answers land.
///
/// The break this catches: every open folder asked for in one go, filling the engine's queue ahead
/// of the reader's own steer and flashing "the engine is busy" for work they never asked for.
#[test]
fn a_change_across_many_folders_asks_for_their_listings_a_few_at_a_time() {
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::Engine(luxe_core::event::Event::WorkspaceUpdated(Box::new(
        changed_workspace("tree-burst", 20),
    ))));
    let (root, _) = listing_requests(&mut rx).pop().expect("the root is asked for");
    // src and its twenty modules are all open on the way to a change
    land_folders(&mut app, root, "", &["src".to_string()]);

    let first = listing_requests(&mut rx);
    assert_eq!(first.len(), 8, "no more than eight in flight: {first:?}");
    let (answered, folder) = first[0].clone();
    land_folders(&mut app, answered, &folder, &[]);
    let next = listing_requests(&mut rx);
    assert_eq!(next.len(), 1, "one landed, so one more is asked for: {next:?}");
    assert!(!first.iter().any(|(_, asked)| *asked == next[0].1), "and it's a new folder");
    assert_eq!(app.status.toast(), None, "and nothing told the reader to try again");
}

/// A file that did not change opens whole, numbered and highlighted, with no Diff to switch to.
#[test]
fn an_unchanged_file_opens_whole_in_the_file_view() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(changed_workspace("file-view", 1));
    open_file_view(&mut app, &mut rx, "README.md", "# Title\nsecond line\n");

    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);
    assert!(content.contains("README.md"), "the header names it: {content}");
    let second = row_containing(&buffer, "second line", width, height);
    assert!(row_text(&buffer, second, width).contains(" 2 "), "lines are numbered: {content}");
    assert!(!content.contains("DIFF"), "nothing changed, so there is no diff to show: {content}");
    assert!(content.contains("esc close"), "and it says how to leave: {content}");
}

/// Dragging down the line numbers picks lines, and Ask turns the pick into a mention in the
/// composer — a pointer to the lines, not a paste of them. (A drag over the code selects text.)
#[test]
fn picked_lines_become_a_mention_in_the_composer() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(changed_workspace("file-pick", 1));
    open_file_view(&mut app, &mut rx, "README.md", "one\ntwo\nthree\nfour\n");

    let buffer = render_app(&mut app, &backend);
    let from = row_containing(&buffer, "two", width, height);
    let to = row_containing(&buffer, "three", width, height);
    // The number, the keyboard's mark, the change bar and a space come before the code
    let gutter = column_of(&row_text(&buffer, from, width), "two") - 4;
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    for (kind, row) in [
        (MouseEventKind::Down(MouseButton::Left), from),
        (MouseEventKind::Drag(MouseButton::Left), to),
        (MouseEventKind::Up(MouseButton::Left), to),
    ] {
        let event = MouseEvent { kind, column: gutter, row, modifiers: KeyModifiers::NONE };
        let action = route_mouse(&mut app, &event);
        apply(&mut app, action);
    }
    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(content.contains("ask about lines 2–3"), "the pick offers the question: {content}");

    app.update(Msg::PeekSelectionAction(luxe_tui::mouse::PeekSelectionAction::Ask));
    assert_eq!(app.editor.current_input().trim(), "@[file:README.md:2-3]");
}

/// A right-click on a file in the tree offers what can be done with it, each with its key.
#[test]
fn a_right_click_on_a_file_opens_its_menu() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(changed_workspace("file-menu", 2));
    let buffer = render_app(&mut app, &backend);
    let row = dock_row_containing(&buffer, "file00.rs", height);

    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let event = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: 12,
        row,
        modifiers: KeyModifiers::NONE,
    };
    route_mouse(&mut app, &event);
    let menu = app.overlays.context_menu.as_ref().expect("a menu opened");
    let labels: Vec<&str> = menu.items.iter().map(|item| item.label.as_str()).collect();
    assert!(labels.contains(&"Show the diff"), "a changed file offers its diff: {labels:?}");
    assert!(labels.contains(&"Copy path"), "{labels:?}");
    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(content.contains("Open in editor"), "and it is drawn: {content}");

    // A click anywhere else puts it away
    left_click(&mut app, 120, 2);
    assert!(app.overlays.context_menu.is_none());
}

/// `Ctrl+T` puts the keyboard in the tree, the tree's own keys move through it, and `Enter` shows
/// the file the cursor is on.
#[test]
fn the_keyboard_walks_the_tree() {
    let (width, height) = (160, 40);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(changed_workspace("file-keys", 2));
    let key = |code| Msg::Key(KeyEvent::new(code, KeyModifiers::NONE));

    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)));
    assert!(app.files.focused, "the keyboard is in the tree");
    assert_eq!(app.files.cursor.as_deref(), Some("src"), "on its first row");
    for _ in 0..2 {
        app.update(key(KeyCode::Char('j')));
    }
    assert_eq!(app.files.cursor.as_deref(), Some("src/module00/file00.rs"));
    app.update(key(KeyCode::Enter));
    match next_action_but_listings(&mut rx) {
        luxe_core::engine::Action::LoadFileDiff { path, .. } => {
            assert_eq!(path, "src/module00/file00.rs", "a changed file opens on its diff")
        }
        other => panic!("expected the diff to load, got {other:?}"),
    }
    assert_eq!(app.editor.current_input(), "", "the keys moved the tree, not the draft");

    app.update(key(KeyCode::Esc));
    assert!(!app.files.focused, "Esc gives the keyboard back");
}

/// The peek's FILE / DIFF switch: both faces in capitals, both slabs, neither moving when the
/// view flips, and the hidden face — the whole of it — is what a click reaches.
#[test]
fn the_view_switch_names_both_faces_and_the_hidden_one_takes_the_click() {
    let (width, height) = (200, 24);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.dock.hide();
    app.workspace = Some(changed_workspace("view-switch", 2));
    let path = "src/module00/file00.rs";
    open_changes_with(&mut app, &mut rx, path, Some(one_hunk_file(path)));

    let buffer = render_app(&mut app, &backend);
    let header = row_containing(&buffer, "DIFF", width, height);
    let text = row_text(&buffer, header, width);
    let (file_at, diff_at) = (text.find("FILE"), text.find("DIFF"));
    assert!(file_at.is_some(), "the hidden face is named in capitals too: {text}");
    assert!(!text.contains(" file "), "and never as a lowercase word: {text}");
    let file_x = text[..file_at.unwrap()].chars().count() as u16;
    let lit = buffer[(file_x + 1, header)].bg;
    assert_ne!(lit, buffer[(width - 1, header)].bg, "the hidden face sits on a slab of its own");

    // Every cell of the hidden badge is the click target, and none of the showing one is
    for x in file_x.saturating_sub(2)..file_x + 4 + 4 {
        assert_eq!(
            app.hit.borrow().at(x, header),
            Some(&luxe_tui::mouse::MouseTarget::ChangesFooter(
                luxe_tui::mouse::ChangesFooterAction::ToggleView
            )),
            "column {x} of the hidden face toggles the view"
        );
    }
    let diff_x = text[..diff_at.unwrap()].chars().count() as u16;
    assert_eq!(app.hit.borrow().at(diff_x + 1, header), None, "the showing face is not a button");

    app.update(Msg::DispatchUiAction(luxe_core::keymap::UiAction::ChangesToggleView));
    let buffer = render_app(&mut app, &backend);
    let flipped = row_text(&buffer, header, width);
    assert_eq!(flipped.find("FILE"), file_at, "FILE holds its column when the view flips");
    assert_eq!(flipped.find("DIFF"), diff_at, "and so does DIFF");
}

/// Drives a key through the start screen the way the event loop does.
pub(super) fn press_on_dashboard(app: &mut App, code: KeyCode) {
    match route_key(app, &key(code)) {
        RouteAction::Msg(message) => app.update(*message),
        RouteAction::Dashboard(outcome) => app.handle_dashboard_outcome(outcome),
        RouteAction::None => {}
        other => panic!("{code:?} routed to {other:?}"),
    }
}

/// The start screen's prompt is the session's composer, so `/` and `@` open its flyouts — under
/// the prompt rather than over it, since here the room is below — and the launcher's list folds
/// away while one is open.
#[test]
fn the_start_screens_prompt_opens_the_composers_flyouts_below_it() {
    let (width, height) = (140, 40);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::Dashboard).expect("the fixture");
    app.update(Msg::TerminalResized { width, height });

    press_on_dashboard(&mut app, KeyCode::Char('@'));
    assert!(app.overlays.mention.is_some(), "@ opens the mention flyout");
    let buffer = render_app(&mut app, &backend);
    let prompt = row_containing(&buffer, "build", width, height);
    let mention = row_containing(&buffer, "Mention", width, height);
    assert!(mention > prompt, "it hangs below the prompt: {mention} vs {prompt}");
    let screen = buffer_to_string(&buffer);
    assert!(!screen.contains("New session"), "and the list has made way for it: {screen}");

    press_on_dashboard(&mut app, KeyCode::Esc);
    assert!(app.overlays.mention.is_none(), "esc puts it away");
    assert_eq!(app.current_view, ViewMode::Dashboard, "and nothing else");
}

/// A command chosen from the slash flyout runs the way the launcher runs one: the theme picker
/// opens over the start screen rather than in a session that is not there.
#[test]
fn a_command_chosen_from_the_slash_flyout_runs_on_the_start_screen() {
    let (width, height) = (140, 40);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::Dashboard).expect("the fixture");
    app.update(Msg::TerminalResized { width, height });
    for character in "/theme".chars() {
        press_on_dashboard(&mut app, KeyCode::Char(character));
    }
    assert!(app.overlays.slash_flyout.is_some(), "/ opens the command flyout");
    let buffer = render_app(&mut app, &backend);
    let prompt = row_containing(&buffer, "build", width, height);
    let commands = row_containing(&buffer, "Commands", width, height);
    assert!(commands > prompt, "below the prompt: {commands} vs {prompt}");

    press_on_dashboard(&mut app, KeyCode::Enter);
    assert!(app.overlays.theme_picker.is_some(), "the theme picker opened");
    assert_eq!(app.current_view, ViewMode::Dashboard, "over the start screen");
    assert!(app.editor.current_input().is_empty(), "and the command left the prompt");
}
