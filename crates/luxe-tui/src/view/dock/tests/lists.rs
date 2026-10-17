//! Lists — scrolling lists, elsewhere, and the hint row.

use super::*;

pub(super) fn changes_of(paths: &[&str]) -> DockChanges {
    DockChanges {
        files: paths
            .iter()
            .map(|path| DockChangeItem {
                path: (*path).to_string(),
                status: luxe_core::workspace::FileChangeStatus::Modified,
                additions: Some(1),
                deletions: Some(0),
            })
            .collect(),
        total_files: paths.len(),
        additions: Some(paths.len() as u64),
        deletions: Some(0),
        remaining: None,
    }
}

/// The lane is paid for out of the rows it belongs to, and only while it is drawn.
///
/// A list that fits keeps its full width: two cells is a directory component in a forty-cell
/// rail, and giving them up for a bar that is not there would cost every reader a path.
#[test]
fn a_scrolling_list_gives_up_exactly_the_lane_and_a_settled_one_gives_up_nothing() {
    let app = app();
    let paths: Vec<String> = (0..24).map(|index| format!("src/dir{index:02}/f.rs")).collect();
    let long: Vec<&str> = paths.iter().map(String::as_str).collect();

    let widest = |section: &DockSection| {
        let (start, end) = section.list.as_ref().map_or((0, section.lines.len()), |l| l.rows);
        section.lines[start..end]
            .iter()
            .map(|line| UnicodeWidthStr::width(text(line).as_str()))
            .max()
            .unwrap_or(0)
    };

    let scrolling = file_lines(Some(&changes_of(&long)), &app, 8);
    assert!(scrolling.list.is_some(), "twenty-four folders outgrow eight rows");
    assert!(
        widest(&scrolling) <= DEFAULT_ROW - SCROLLBAR_LANE,
        "a row ran into the lane: {}",
        widest(&scrolling)
    );

    let settled = file_lines(Some(&changes_of(&long[..2])), &app, 8);
    assert!(settled.list.is_none(), "two files fit");
    assert!(widest(&settled) <= DEFAULT_ROW, "and still cannot outgrow the rail");
}

/// A place holding `files`, each as `(path, additions)`.
pub(super) fn place(root: &str, files: &[(&str, u64)]) -> DockExternalGroup {
    DockExternalGroup {
        root: root.into(),
        location: root.to_string(),
        branch: None,
        git: false,
        files: files
            .iter()
            .map(|(path, additions)| DockChangeItem {
                path: (*path).to_string(),
                status: luxe_core::workspace::FileChangeStatus::Modified,
                additions: Some(*additions),
                deletions: Some(0),
            })
            .collect(),
        remaining: None,
    }
}

/// Elsewhere reads like Changes one level in: a place, the folders under it, the files under
/// those — except at a place's own root, where the place already is the folder.
#[test]
fn elsewhere_nests_folders_under_a_place_but_not_at_its_root() {
    let app = app();
    let external = DockExternal {
        groups: vec![place("/work/sdk", &[("src/client.ts", 40), ("README.md", 2)])],
        total_files: 2,
    };
    let rows = external_rows(&external, &app);
    // A→Z, exactly as the changes are grouped — which puts the place's own root ahead of its folders.
    assert_eq!(
        shape(&rows),
        ["place", "file", "directory", "file"],
        "one heading, and the root file has none"
    );
    assert!(
        matches!(&rows[2], ExternalRow::Under { row: DockChangeRow::Directory(at), .. }
            if at.directory == "src"),
        "and the heading is the folder the nested file is in"
    );
}

/// What each ELSEWHERE row is, for a test that cares about the shape rather than the words.
pub(super) fn shape<'a>(rows: &'a [ExternalRow<'a>]) -> Vec<&'static str> {
    rows.iter()
        .map(|row| match row {
            ExternalRow::Place { .. } => "place",
            ExternalRow::Under { row: DockChangeRow::Directory(_), .. } => "directory",
            ExternalRow::Under { .. } => "file",
        })
        .collect()
}

/// A window opening mid-place repeats what its rows sit under — up to two headings, because
/// the list nests twice — and a window opening on a heading of its own repeats nothing.
#[test]
fn a_window_into_elsewhere_repeats_the_headings_it_opened_under() {
    let app = app();
    let external = DockExternal {
        groups: vec![
            place("/work/sdk", &[("src/client.ts", 40), ("src/events.ts", 20), ("README.md", 2)]),
            place("/work/tools", &[("bin/run.sh", 3)]),
        ],
        total_files: 4,
    };
    let rows = external_rows(&external, &app);
    assert_eq!(
        shape(&rows),
        ["place", "file", "directory", "file", "file", "place", "directory", "file"]
    );

    assert!(external_context(&rows, 0).is_empty(), "the top is under nothing");
    assert!(external_context(&rows, 5).is_empty(), "and so is a place heading");
    assert_eq!(external_context(&rows, 2), [0], "a folder needs its place");
    assert_eq!(external_context(&rows, 3), [0, 2], "a nested file needs both");
    assert_eq!(external_context(&rows, 1), [0], "a root file needs only the place");
    assert_eq!(external_context(&rows, 7), [5, 6], "and the second place is its own");
}

/// Both ways in, on one row, each answering for its own words — and never wrapped, because a
/// wrapped hint puts half of itself where its click region no longer points.
#[test]
fn the_hint_row_offers_both_ways_in_and_keeps_them_on_one_line() {
    let app = app();
    let (line, regions) = hint_line(&change_hints(&app), 7, &app);
    assert_eq!(text(&line), "ctrl+p preview · ctrl+g review");
    assert!(
        UnicodeWidthStr::width(text(&line).as_str()) <= usize::from(CONTEXT_DOCK_WIDTH) - 2,
        "the rail has to seat it: {:?}",
        text(&line)
    );

    // Each region names the columns its own words occupy, on the row it was built for.
    let (peek, review) = (&regions[0], &regions[1]);
    assert_eq!(peek.target, crate::mouse::MouseTarget::DockPeekChanges);
    assert_eq!((peek.line, peek.columns), (7, Some((0, 14))));
    assert_eq!(review.target, crate::mouse::MouseTarget::DockReviewChanges);
    assert_eq!((review.line, review.columns), (7, Some((17, 13))));

    // A rail too narrow for both keeps the review: every file row above already opens the preview.
    let long = vec![
        ChangeHint {
            text: "a very long way to say peek indeed, at length".into(),
            target: crate::mouse::MouseTarget::DockPeekChanges,
        },
        ChangeHint {
            text: "ctrl+g review".into(),
            target: crate::mouse::MouseTarget::DockReviewChanges,
        },
    ];
    let (line, regions) = hint_line(&long, 0, &app);
    assert_eq!(text(&line), "ctrl+g review");
    assert_eq!(regions.len(), 1);
}
