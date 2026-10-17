//! Changes — the dock list, the changes pane and the statusline module.

use super::*;

/// An app whose engine channel is *kept*, so a test can read the actions the UI sends and answer
/// them the way the engine would.
pub(super) fn create_app_with_engine(
) -> (App, tokio::sync::mpsc::Receiver<luxe_core::engine::Action>) {
    let (tx, rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    (App::new(tx, KeyMap::defaults(), Config::default()), rx)
}

/// A workspace with `count` changed files, biggest first, in distinct directories.
pub(super) fn changed_workspace(scope: &str, count: usize) -> WorkspaceSnapshot {
    let mut snapshot = empty_workspace(scope);
    let files: Vec<FileChangeSummary> = (0..count)
        .map(|index| FileChangeSummary {
            path: format!("src/module{index:02}/file{index:02}.rs").into(),
            status: FileChangeStatus::Modified,
            additions: Some((count - index) as u64),
            deletions: Some(0),
            binary: false,
        })
        .collect();
    snapshot.changes = WorkspaceChangesSummary {
        total_files: files.len(),
        additions: Some(files.iter().filter_map(|file| file.additions).sum()),
        deletions: Some(0),
        files,
        truncated: false,
    };
    snapshot
}

/// One replaced line, as the review's content model represents it.
pub(super) fn one_hunk_file(path: &str) -> luxe_core::guided_review::ReviewFile {
    use luxe_core::guided_review::{
        FileId, HunkId, LineRange, NoiseKind, ReviewFile, ReviewFileStatus, ReviewHunk,
    };
    let id = FileId::from_path_lineage(
        Some(std::path::Path::new(path)),
        Some(std::path::Path::new(path)),
    );
    let line = |old, new, kind, text: &str| ReviewLine {
        old_line: old,
        new_line: new,
        kind,
        text: text.into(),
    };
    ReviewFile {
        id: id.clone(),
        old_path: Some(path.into()),
        new_path: Some(path.into()),
        status: ReviewFileStatus::Modified,
        language: Some("rs".into()),
        additions: Some(1),
        deletions: Some(1),
        hunks: vec![ReviewHunk {
            id: HunkId::from_content(&id, "h", "h"),
            old_range: LineRange::new(1, 2),
            new_range: LineRange::new(1, 2),
            heading: None,
            symbol: None,
            lines: vec![
                line(Some(1), Some(1), ReviewLineKind::Context, "fn sum() {"),
                line(Some(2), None, ReviewLineKind::Deletion, "let total = a + b;"),
                line(None, Some(2), ReviewLineKind::Addition, "let total = a - b;"),
            ],
            noise: NoiseKind::Semantic,
        }],
        binary: false,
        generated: false,
        non_text: None,
    }
}

/// Opens `path` and answers the capture the UI asks for, the way the engine would.
pub(super) fn open_changes_with(
    app: &mut App,
    rx: &mut tokio::sync::mpsc::Receiver<luxe_core::engine::Action>,
    path: &str,
    file: Option<luxe_core::guided_review::ReviewFile>,
) {
    app.update(Msg::OpenChanges(path.to_string()));
    let request_id = loop {
        match rx.try_recv().expect("the pane asks the engine for the diff") {
            luxe_core::engine::Action::LoadFileDiff { request_id, path: asked } => {
                assert_eq!(asked, path, "it asks for the file it is showing");
                break request_id;
            }
            _ => continue,
        }
    };
    app.update(Msg::Engine(luxe_core::event::Event::FileDiffLoaded {
        request_id,
        diff: Box::new(luxe_core::file_diff::FileDiff {
            path: path.to_string(),
            file: file.map(Box::new),
            untracked: false,
        }),
    }));
}

/// Whether the diff row carrying `needle` is drawn as two columns.
///
/// Asked of the row rather than the screen: a bare `│` also draws the scrollbar rail and the
/// composer's own border, so a whole-buffer search for the rule finds one whether or not the diff
/// has two columns.
pub(super) fn split_row(buffer: &Buffer, needle: &str, app: &App, width: u16, height: u16) -> bool {
    let row = row_containing(buffer, needle, width, height);
    peek_diff_text(buffer, row, app, width).contains(" │ ")
}

/// One row of the peeked diff's own columns.
///
/// Scoped, because with the conversation kept beside it every row also carries the rule between
/// the two panes and whatever the transcript put after it.
pub(super) fn peek_diff_text(buffer: &Buffer, y: u16, app: &App, width: u16) -> String {
    row_text(buffer, y, luxe_tui::view::peek_diff_column(app, width).unwrap_or(width))
}

/// Screen row of the first line containing `needle`.
pub(super) fn row_containing(buffer: &Buffer, needle: &str, width: u16, height: u16) -> u16 {
    (0..height)
        .find(|&y| row_text(buffer, y, width).contains(needle))
        .unwrap_or_else(|| panic!("no rendered row contains {needle:?}"))
}

/// Screen row of the first *dock* line containing `needle`.
///
/// Scoped to the rail's own columns because the pane beside it shows the same file name in its
/// header, and a whole-row search finds whichever is drawn first.
pub(super) fn dock_row_containing(buffer: &Buffer, needle: &str, height: u16) -> u16 {
    let rail = luxe_tui::workspace_layout::CONTEXT_DOCK_WIDTH;
    (0..height)
        .find(|&y| row_text(buffer, y, rail).contains(needle))
        .unwrap_or_else(|| panic!("no dock row contains {needle:?}"))
}

pub(super) fn left_click(app: &mut App, column: u16, row: u16) -> RouteAction {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    route_mouse(
        app,
        &MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        },
    )
}

pub(super) fn wheel_down(app: &mut App, column: u16, row: u16) -> RouteAction {
    use crossterm::event::{MouseEvent, MouseEventKind};
    route_mouse(
        app,
        &MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        },
    )
}

/// The whole point of listing names: a click on one asks what it did to that file.
#[test]
fn clicking_a_dock_change_row_opens_that_files_diff() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(changed_workspace("dock-change-click", 3));

    let buffer = render_app(&mut app, &backend);
    let row = dock_row_containing(&buffer, "file00.rs", height);
    match left_click(&mut app, 5, row) {
        RouteAction::Msg(message) => assert!(
            matches!(&*message, Msg::OpenProjectFile(path) if path == "src/module00/file00.rs"),
            "expected the clicked file's path, got {message:?}"
        ),
        other => panic!("expected a diff to open from the click, got {other:?}"),
    }
}

/// The gesture that opened the pane puts it away, so one row is one control rather than a trap.
#[test]
fn clicking_the_open_file_again_closes_the_pane() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(changed_workspace("dock-change-toggle", 3));
    let path = "src/module00/file00.rs";
    open_changes_with(&mut app, &mut rx, path, Some(one_hunk_file(path)));

    let buffer = render_app(&mut app, &backend);
    let row = dock_row_containing(&buffer, "file00.rs", height);
    match left_click(&mut app, 5, row) {
        RouteAction::Msg(message) => {
            assert!(matches!(*message, Msg::CloseChanges), "a second click closes it")
        }
        other => panic!("expected the pane to close, got {other:?}"),
    }
}

/// A change bigger than the dock's budget is still entirely reachable: the section scrolls inside
/// its own viewport instead of truncating, and a bar in its own lane says where you are in it.
#[test]
fn every_changed_file_is_reachable_by_scrolling_the_dock_list() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(changed_workspace("dock-change-scroll", 24));

    let buffer = render_app(&mut app, &backend);
    let first = buffer_to_string(&buffer);
    assert!(first.contains("file00.rs"), "the biggest change leads: {first}");
    assert!(!first.contains("file23.rs"), "and the smallest is below the fold");
    // The bar replaced the words: no scroll-position prose, and the thumb is against the top.
    for prose in ["more below", "scroll here", " above", "not listed"] {
        assert!(!first.contains(prose), "the lane says it instead of {prose:?}: {first}");
    }
    let lane = dock_scroll_lane(&buffer, "file00.rs", height);
    assert_eq!(lane.first().copied(), Some(THUMB), "the thumb starts at the top: {lane:?}");
    assert_eq!(lane.last().copied(), Some(TRACK), "and there is track below it: {lane:?}");

    // Scroll until the last file arrives, bounded so a broken clamp fails instead of hanging.
    let mut reached = false;
    for _ in 0..40 {
        app.update(Msg::ScrollDockList { list: DockList::Files, up: false });
        if buffer_to_string(&render_app(&mut app, &backend)).contains("file23.rs") {
            reached = true;
            break;
        }
    }
    assert!(reached, "every changed file is reachable by scrolling");

    let buffer = render_app(&mut app, &backend);
    let lane = dock_scroll_lane(&buffer, "file23.rs", height);
    assert_eq!(lane.last().copied(), Some(THUMB), "the thumb followed to the end: {lane:?}");
    assert_eq!(lane.first().copied(), Some(TRACK), "leaving track behind it: {lane:?}");
}

/// The dock's scrollbar lane: the track glyph, the thumb, and the column they share.
pub(super) const TRACK: char = '\u{2595}';
pub(super) const THUMB: char = '\u{2590}';
/// The lane beside the dock list containing `needle`, top to bottom.
///
/// Found from the row the list is on rather than from a fixed column, so the test fails on a lane
/// that moved rather than silently reading whatever is at column forty.
pub(super) fn dock_scroll_lane(buffer: &Buffer, needle: &str, height: u16) -> Vec<char> {
    let rail = luxe_tui::workspace_layout::CONTEXT_DOCK_WIDTH;
    let column = (0..rail)
        .rev()
        .find(|&x| {
            let glyph = buffer[(x, dock_row_containing(buffer, needle, height))].symbol();
            glyph == TRACK.to_string() || glyph == THUMB.to_string()
        })
        .unwrap_or_else(|| panic!("no scrollbar lane on the row carrying {needle:?}"));
    (0..height)
        .map(|y| buffer[(column, y)].symbol().chars().next().unwrap_or(' '))
        .filter(|glyph| *glyph == TRACK || *glyph == THUMB)
        .collect()
}

/// A checklist longer than the rail is the same kind of surface: a lane beside it, the wheel over
/// it moving it, and every todo reachable.
///
/// The bar starts where the *work in hand* is rather than at the top, because that is the row a
/// reader is looking for; scrolling takes the list over, and scrolling back hands it to the plan
/// again.
#[test]
fn the_plan_scrolls_under_its_own_bar_and_goes_back_to_following_the_work() {
    let (width, height) = (160, 44);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(planned_workspace("dock-plan-scroll", 20, 14));

    let buffer = render_app(&mut app, &backend);
    let first = buffer_to_string(&buffer);
    assert!(first.contains("step 14"), "the row in hand is in view: {first}");
    assert!(!first.contains("step 00"), "the history above it is not");
    assert!(!first.contains("earlier"), "and the lane says so instead of a count: {first}");
    let lane = dock_scroll_lane(&buffer, "step 14", height);
    assert!(lane.contains(&THUMB) && lane.contains(&TRACK), "a real bar: {lane:?}");
    assert_eq!(lane.first().copied(), Some(TRACK), "parked past the top, on the work: {lane:?}");

    // The wheel over the checklist moves the checklist, not the panel.
    let row = dock_row_containing(&buffer, "step 14", height);
    match wheel_down(&mut app, 5, row) {
        RouteAction::Msg(message) => {
            assert!(
                matches!(*message, Msg::ScrollDockList { list: DockList::Plan, up: false }),
                "over the checklist, the wheel moves the checklist: {message:?}"
            );
            app.update(*message);
        }
        other => panic!("expected the checklist to scroll, got {other:?}"),
    }
    let scrolled = buffer_to_string(&render_app(&mut app, &backend));
    assert!(scrolled.contains("step 19"), "the tail is reachable: {scrolled}");

    // Back to the top, then back down to the work: the plan is following again.
    for _ in 0..20 {
        app.update(Msg::ScrollDockList { list: DockList::Plan, up: true });
    }
    let top = buffer_to_string(&render_app(&mut app, &backend));
    assert!(top.contains("step 00"), "every todo is reachable: {top}");
    for _ in 0..20 {
        app.update(Msg::ScrollDockList { list: DockList::Plan, up: false });
    }
    let bottom = buffer_to_string(&render_app(&mut app, &backend));
    assert!(bottom.contains("step 19"), "and the far end too: {bottom}");
}

/// A checklist that fits carries no lane at all — a bar on a complete list claims there is more.
#[test]
fn a_plan_that_fits_the_rail_has_no_bar() {
    let (width, height) = (160, 44);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(planned_workspace("dock-plan-short", 4, 2));

    let buffer = render_app(&mut app, &backend);
    let rail = luxe_tui::workspace_layout::CONTEXT_DOCK_WIDTH;
    let row = dock_row_containing(&buffer, "step 02", height);
    assert!(
        (0..rail).all(|x| ![TRACK.to_string(), THUMB.to_string()]
            .contains(&buffer[(x, row)].symbol().to_string())),
        "a complete checklist draws no lane: {}",
        row_text(&buffer, row, rail)
    );
    // And the wheel over it belongs to the panel again.
    match wheel_down(&mut app, 5, row) {
        RouteAction::Msg(message) => assert!(
            matches!(*message, Msg::ScrollDock { up: false }),
            "nothing to scroll in the section, so the panel moves: {message:?}"
        ),
        other => panic!("expected the panel to scroll, got {other:?}"),
    }
}

/// A session whose checklist is longer than the rail, with `active` in progress and everything
/// before it done.
pub(super) fn planned_workspace(scope: &str, count: usize, active: usize) -> WorkspaceSnapshot {
    let mut snapshot = empty_workspace(scope);
    let items: Vec<TodoSummaryItem> = (0..count)
        .map(|index| TodoSummaryItem {
            id: index.to_string(),
            subject: format!("step {index:02}"),
            status: match index.cmp(&active) {
                std::cmp::Ordering::Less => TodoSummaryStatus::Completed,
                std::cmp::Ordering::Equal => TodoSummaryStatus::InProgress,
                std::cmp::Ordering::Greater => TodoSummaryStatus::Pending,
            },
            blocked_by: Vec::new(),
        })
        .collect();
    snapshot.todos = TodoSummary {
        current: items.get(active).cloned(),
        pending: count.saturating_sub(active + 1),
        in_progress: 1,
        completed: active,
        blocked: 0,
        total: count,
        truncated: false,
        items,
    };
    snapshot
}

/// Two scroll regions, one panel: the wheel over the file list moves the list, and the wheel
/// anywhere else in the dock moves the panel. Without the distinction the list could never be
/// bounded *and* complete.
#[test]
fn the_wheel_moves_the_change_list_over_it_and_the_panel_elsewhere() {
    let (width, height) = (160, 30);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(changed_workspace("dock-scroll-regions", 24));

    let buffer = render_app(&mut app, &backend);
    let over_list = dock_row_containing(&buffer, "file00.rs", height);
    let over_heading = dock_row_containing(&buffer, "PROJECT", height);

    match wheel_down(&mut app, 5, over_list) {
        RouteAction::Msg(message) => assert!(
            matches!(*message, Msg::ScrollDockList { list: DockList::Files, up: false }),
            "over the list, the wheel moves the list: {message:?}"
        ),
        other => panic!("expected the list to scroll, got {other:?}"),
    }
    match wheel_down(&mut app, 5, over_heading) {
        RouteAction::Msg(message) => assert!(
            matches!(*message, Msg::ScrollDock { up: false }),
            "elsewhere it moves the panel: {message:?}"
        ),
        other => panic!("expected the panel to scroll, got {other:?}"),
    }
}
