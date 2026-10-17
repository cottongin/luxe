//! Tree — the file tree in the rail, its filter and its commit hint.

use super::*;

#[test]
fn the_tree_takes_the_height_the_rest_of_the_rail_leaves() {
    // A tall dock gives the tree everything else does not use: it is the project, not a list
    assert_eq!(files_row_cap(40, 8), 40 - 8 - FILES_FIXED_ROWS);

    // A crowded rail keeps a usable window rather than collapsing: the tree scrolls, so the
    // rows it cannot show are reachable, and the sections underneath it are reached by
    // scrolling the panel.
    assert_eq!(files_row_cap(20, 16), FILE_ROWS_MIN);
    assert_eq!(files_row_cap(40, 36), FILE_ROWS_MIN);

    // The floor scales down with the panel, so a very short dock still spends most of itself
    // on everything else — a quarter of the rail rather than a flat five rows.
    assert_eq!(files_row_cap(12, 10), 3);
    assert_eq!(files_row_cap(6, 4), 1);
}

/// A commit must not take the way into the review with it.
///
/// This is the loss a real session reported: the agent committed at the end of its turn, the
/// diff against `HEAD` went to zero, the section rendered nothing, and the review that was
/// about to be walked had no affordance left anywhere on the surface.
/// A filter says what it is filtering by and how much it found, takes a row from the tree
/// rather than growing the section, and lights the matched characters in the accent.
#[test]
fn a_filter_says_what_it_matched_and_lights_it() {
    let mut app = app();
    let files = ["src/view/dock.rs".to_string(), "src/app.rs".to_string()];
    crate::fixture::filter_tree(&mut app, &files, "dock");
    let section = file_lines(None, &app, 8);
    let lines: Vec<String> = section.lines.iter().map(text).collect();
    assert!(lines.iter().any(|line| line.starts_with("/ dock▏") && line.ends_with('1')));
    assert!(lines.iter().any(|line| line.contains("esc clear")), "{lines:?}");
    let row = section.lines.iter().find(|line| text(line).contains("dock.rs")).unwrap();
    let lit: String = row
        .spans
        .iter()
        .filter(|span| span.style.fg == app.theme_styles.accent.fg)
        .map(|span| span.content.as_ref())
        .collect();
    assert_eq!(lit, "dock");
}

#[test]
fn a_commit_leaves_the_review_reachable_instead_of_an_empty_rail() {
    let mut app = app();
    let rendered = |app: &App| -> String {
        file_lines(None, app, 8).lines.iter().map(text).collect::<Vec<_>>().join("\n")
    };
    assert!(!rendered(&app).contains("committed"), "nothing has happened yet");

    app.worktree_emptied = true;
    assert!(!rendered(&app).contains("committed"), "and nothing is retained to review");

    app.review.last_turn = Some(("session-1".into(), "turn-1".into()));
    let rendered = rendered(&app);
    assert!(rendered.contains("FILES"), "{rendered}");
    assert!(rendered.contains("committed"), "says where the work went: {rendered}");
    assert!(rendered.contains("still reviewable"), "and that it is not lost: {rendered}");
    assert!(rendered.contains("ctrl+g review"), "and how to get there: {rendered}");
    assert!(
        !rendered.contains("preview"),
        "and not how to glance, with nothing left in the worktree to glance at: {rendered}"
    );
}

/// The way into the review is a row of its own that answers a click.
#[test]
fn the_committed_review_hint_is_a_row_that_opens_the_review() {
    let mut app = app();
    app.worktree_emptied = true;
    app.review.last_turn = Some(("session-1".into(), "turn-1".into()));
    let section = file_lines(None, &app, 8);
    let last = section.lines.len() - 1;
    assert_eq!(text(&section.lines[last]), "ctrl+t browse · ctrl+g review");
    assert!(section
        .hits
        .iter()
        .any(|hit| hit.line == last && hit.target == crate::mouse::MouseTarget::DockReviewChanges));
}

/// The file showing in the preview is told apart by its name, accent and bold, and the band is
/// left to the row the keyboard is on, so the two never look alike on different rows.
#[test]
fn the_showing_file_wears_its_name_and_the_keyboard_row_wears_the_band() {
    let mut app = app();
    let showing = TreeRow::File {
        path: "CHANGELOG.md".into(),
        name: "CHANGELOG.md".into(),
        depth: 0,
        change: None,
        ignored: false,
    };
    let line = tree_row_line(&showing, 40, Some("CHANGELOG.md"), None, &app);
    let name = line.spans.iter().find(|span| span.content.contains("CHANGELOG")).unwrap();
    assert_eq!(name.style.fg, app.theme_styles.accent.fg, "the name says it is showing");
    assert!(name.style.add_modifier.contains(Modifier::BOLD));
    assert_ne!(name.style.bg, app.theme_styles.selected_bg.bg, "but wears no band");

    app.files.focused = true;
    app.files.cursor = Some("src".into());
    let keyed = TreeRow::Folder {
        path: "src".into(),
        name: "src".into(),
        depth: 0,
        open: false,
        holds_change: false,
        ignored: false,
    };
    let line = tree_row_line(&keyed, 40, Some("CHANGELOG.md"), None, &app);
    let name = line.spans.iter().find(|span| span.content == "src").unwrap();
    assert_eq!(name.style.bg, app.theme_styles.selected_bg.bg, "the keyboard's row is banded");
}

/// The tree says how to reach it from the keyboard, and once the keyboard is in it, what the
/// keys do there; the hint answers a click by putting the keyboard in.
#[test]
fn the_tree_says_how_to_move_through_it_with_keys() {
    let mut app = app();
    let section = file_lines(None, &app, 8);
    let last = section.lines.len() - 1;
    assert_eq!(text(&section.lines[last]), "ctrl+t browse");
    assert!(section
        .hits
        .iter()
        .any(|hit| hit.line == last && hit.target == crate::mouse::MouseTarget::DockFocusFiles));

    app.files.focused = true;
    let section = file_lines(None, &app, 8);
    let last = section.lines.len() - 1;
    assert_eq!(text(&section.lines[last]), "j k move · l open · / filter · esc back");
}
