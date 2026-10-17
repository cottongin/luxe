//! Elsewhere — work done outside the project, listed in the rail.

use super::*;

/// A session that changed one file in a checkout of its own, somewhere else.
pub(super) fn external_workspace() -> luxe_core::WorkspaceSnapshot {
    let mut snapshot = empty_workspace("elsewhere");
    snapshot.external = luxe_core::ExternalChangesSummary {
        roots: vec![luxe_core::ExternalRootSummary {
            root: "/work/sdk".into(),
            display_path: "~/work/sdk".into(),
            git_branch: Some("main".into()),
            git: true,
            files: vec![FileChangeSummary {
                path: "src/client.ts".into(),
                status: FileChangeStatus::Modified,
                additions: Some(18),
                deletions: Some(3),
                binary: false,
            }],
            total_files: 1,
            truncated: false,
        }],
        total_files: 1,
        additions: Some(18),
        deletions: Some(3),
        truncated: false,
    };
    snapshot
}

/// Work done outside the project gets its own section, and deliberately no way into review.
///
/// A review is captured from the project's Git worktree and its chapters end in commits to it, so
/// a file that lives somewhere else can never be one of its items. Offering the affordance over
/// these rows would advertise something the review cannot do — a *peek* is a promise the section
/// can keep, and that is the one its rows carry.
#[test]
fn dock_lists_external_work_without_offering_to_review_it() {
    let (width, height) = (160, 44);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(external_workspace());

    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);
    assert!(content.contains("ELSEWHERE"), "the section is drawn: {content}");
    assert!(content.contains("1 file outside this project"), "singular, and it says where");
    assert!(content.contains("~/work/sdk"), "the place is named");
    assert!(content.contains("main"), "and its branch, since Git reports the whole tree there");
    assert!(content.contains("client.ts"), "with the file under it");
    assert!(
        !content.contains("to review"),
        "no review affordance: a project review cannot contain these files\n{content}"
    );
}

/// A file listed elsewhere opens the same way a file listed in CHANGES does.
///
/// The rows are drawn in the project's own hand — same marker, same ±, same indent — so a reader
/// who has learned that a file name opens its change has already been told these do too. They
/// carry the absolute path, which is what tells every surface downstream that the project's Git
/// cannot answer for them.
#[test]
fn clicking_an_elsewhere_row_peeks_that_file() {
    let (width, height) = (160, 44);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(external_workspace());

    let buffer = render_app(&mut app, &backend);
    let row = dock_row_containing(&buffer, "client.ts", height);
    match left_click(&mut app, 5, row) {
        RouteAction::Msg(message) => assert!(
            matches!(&*message, Msg::OpenChanges(path) if path == "/work/sdk/src/client.ts"),
            "expected the file's absolute path, got {message:?}"
        ),
        other => panic!("expected a peek to open from the click, got {other:?}"),
    }
}

/// Looking at a file elsewhere is the only thing that can happen to it, so it is what makes the
/// section recede: a place with nothing left unread folds to its heading.
///
/// Driven through the pane the way a reader would, because the recording is the peek's own doing:
/// nothing else marks these files, and a look that did not mark one would leave the section
/// growing for the whole session no matter what the reader did about it.
#[test]
fn an_elsewhere_place_folds_once_everything_under_it_has_been_seen() {
    let (width, height) = (160, 44);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(external_workspace());

    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(content.contains("client.ts"), "unread work is listed: {content}");

    let path = "/work/sdk/src/client.ts";
    open_changes_with(&mut app, &mut rx, path, Some(one_hunk_file(path)));
    let reading = buffer_to_string(&render_app(&mut app, &backend));
    assert!(
        reading.contains("client.ts"),
        "the row stays while the pane is showing it — a fold here would take it out from under \
         the click that opened it: {reading}"
    );

    app.update(Msg::CloseChanges);
    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);
    assert!(!content.contains("client.ts"), "a place with nothing unread folds away: {content}");
    assert!(content.contains("~/work/sdk"), "but never silently: {content}");
    assert!(content.contains("1 file"), "and it says how much is under it: {content}");

    // The heading is the control, so a folded place can always be opened again.
    let row = dock_row_containing(&buffer, "~/work/sdk", height);
    match left_click(&mut app, 5, row) {
        RouteAction::Msg(message) => {
            assert!(
                matches!(&*message, Msg::ToggleExternalPlace(place) if place == "/work/sdk"),
                "expected the place to toggle, got {message:?}"
            );
            app.update(*message);
        }
        other => panic!("expected a fold toggle from the click, got {other:?}"),
    }
    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(content.contains("client.ts"), "unfolded by hand: {content}");
}

/// Files elsewhere are grouped under folder headings the way the project's own are, so a folder
/// reads the same wherever the rail draws one.
///
/// A file at a place's own root gets no heading: the place above it already is its directory, and
/// a `/ (root)` row under it would say the same thing twice.
#[test]
fn elsewhere_groups_its_files_under_folder_headings() {
    let (width, height) = (160, 44);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(places_workspace("elsewhere-folders", 1, 2));

    let buffer = render_app(&mut app, &backend);
    let rail = luxe_tui::workspace_layout::CONTEXT_DOCK_WIDTH;
    // Where a row's name starts, after its guides, chevron and icon: what the eye reads as nesting.
    let indent = |needle: &str| {
        let row = row_text(&buffer, dock_row_containing(&buffer, needle, height), rail);
        row[..row.find(needle).expect("the row has it")].chars().count()
    };

    // Place, then folder, then the files under it — each a step further in.
    let (place, folder, file) = (indent("~/work/place0"), indent("nested"), indent("file00.ts"));
    assert!(place < folder && folder < file, "place {place}, folder {folder}, file {file}");

    // The file sitting at the place's own root skips the folder level entirely.
    assert_eq!(indent("root.ts"), folder, "a root file sits where a folder heading would");
    let content = buffer_to_string(&buffer);
    assert!(!content.contains("(root)"), "and needs no heading of its own: {content}");
}

/// ELSEWHERE is a viewport onto every place, not the first three of them: the caps that used to
/// count the rest are gone, and the lane beside it reaches them instead.
#[test]
fn every_place_elsewhere_is_reachable_by_scrolling() {
    let (width, height) = (160, 44);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(places_workspace("elsewhere-scroll", 6, 2));

    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);
    assert!(content.contains("~/work/place0"), "the first place leads: {content}");
    assert!(!content.contains("~/work/place5"), "and the last is below the fold");
    assert!(!content.contains("more place"), "counted places are gone: {content}");
    let lane = dock_scroll_lane(&buffer, "~/work/place0", height);
    assert_eq!(lane.first().copied(), Some(THUMB), "the thumb is at the top: {lane:?}");

    // The wheel over the section moves the section, and reaches the last place.
    let row = dock_row_containing(&buffer, "~/work/place0", height);
    assert!(
        matches!(wheel_down(&mut app, 5, row), RouteAction::Msg(message)
            if matches!(*message, Msg::ScrollDockList { list: DockList::Elsewhere, up: false })),
        "the wheel over ELSEWHERE moves ELSEWHERE"
    );
    // One notch opens the window inside the first place — which still has to say which place, and
    // which folder, its rows are under.
    app.update(Msg::ScrollDockList { list: DockList::Elsewhere, up: false });
    let buffer = render_app(&mut app, &backend);
    let rail = luxe_tui::workspace_layout::CONTEXT_DOCK_WIDTH;
    let headline = dock_row_containing(&buffer, "outside this project", height);
    let under = |offset: u16| row_text(&buffer, headline + offset, rail);
    assert!(under(1).contains("~/work/place0"), "the place is repeated: {:?}", under(1));
    assert!(under(2).contains("nested"), "and the folder inside it: {:?}", under(2));
    assert!(under(3).contains("file0"), "before the rows the window actually opened on");

    let mut reached = false;
    for _ in 0..40 {
        app.update(Msg::ScrollDockList { list: DockList::Elsewhere, up: false });
        if buffer_to_string(&render_app(&mut app, &backend)).contains("~/work/place5") {
            reached = true;
            break;
        }
    }
    assert!(reached, "every place is reachable by scrolling");
}

/// `count` places outside the project, each with `files` files under a folder plus one at its root.
pub(super) fn places_workspace(scope: &str, count: usize, files: usize) -> WorkspaceSnapshot {
    let mut snapshot = empty_workspace(scope);
    let change = |path: String| FileChangeSummary {
        path: path.into(),
        status: FileChangeStatus::Modified,
        additions: Some(4),
        deletions: Some(1),
        binary: false,
    };
    let roots: Vec<luxe_core::ExternalRootSummary> = (0..count)
        .map(|place| {
            let mut entries: Vec<FileChangeSummary> =
                (0..files).map(|index| change(format!("nested/file{index:02}.ts"))).collect();
            entries.push(change("root.ts".to_string()));
            luxe_core::ExternalRootSummary {
                root: format!("/work/place{place}").into(),
                display_path: format!("~/work/place{place}"),
                git_branch: None,
                git: false,
                total_files: entries.len(),
                files: entries,
                truncated: false,
            }
        })
        .collect();
    snapshot.external = luxe_core::ExternalChangesSummary {
        total_files: roots.iter().map(|root| root.total_files).sum(),
        additions: Some(4),
        deletions: Some(1),
        roots,
        truncated: false,
    };
    snapshot
}

/// A file touched again since it was looked at is new work, whatever was read before — the same
/// rule the project's own reviewed markers follow, and what keeps a fold from hiding fresh edits.
#[test]
fn work_done_elsewhere_since_the_last_look_comes_back_unread() {
    let (width, height) = (160, 44);
    let backend = test_backend(width, height);
    let (mut app, mut rx) = create_app_with_engine();
    app.update(Msg::TerminalResized { width, height });
    app.workspace = Some(external_workspace());

    let path = "/work/sdk/src/client.ts";
    open_changes_with(&mut app, &mut rx, path, Some(one_hunk_file(path)));
    app.update(Msg::CloseChanges);
    assert!(!buffer_to_string(&render_app(&mut app, &backend)).contains("client.ts"), "seen");

    if let Some(workspace) = app.workspace.as_mut() {
        workspace.external.roots[0].files[0].additions = Some(40);
    }
    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(content.contains("client.ts"), "touched again since, so unread again: {content}");
}

/// The project's own Changes section keeps its way into review — the assertion above only means
/// something if the affordance is present when it should be.
#[test]
fn dock_still_offers_review_for_the_projects_own_changes() {
    let (width, height) = (160, 44);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.update(Msg::TerminalResized { width, height });
    let mut snapshot = empty_workspace("changes");
    snapshot.changes = WorkspaceChangesSummary {
        files: vec![FileChangeSummary {
            path: "src/lib.rs".into(),
            status: FileChangeStatus::Modified,
            additions: Some(1),
            deletions: Some(0),
            binary: false,
        }],
        total_files: 1,
        additions: Some(1),
        deletions: Some(0),
        truncated: false,
    };
    app.workspace = Some(snapshot);

    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(content.contains("ctrl+g review"), "in-project changes lead to a review: {content}");
}
