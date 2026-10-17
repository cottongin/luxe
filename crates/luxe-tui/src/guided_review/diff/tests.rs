use super::*;
use luxe_core::guided_review::{FileId, HunkId, NoiseKind};
use std::path::Path;

fn hunk(start: u32, lines: Vec<ReviewLine>) -> ReviewHunk {
    ReviewHunk {
        id: HunkId::from_content(
            &FileId::from_path_lineage(None, Some(Path::new("fixture.rs"))),
            &format!("context-{start}"),
            "let value = 1;",
        ),
        old_range: LineRange::new(start, lines.len() as u32),
        new_range: LineRange::new(start, lines.len() as u32),
        heading: Some("fn example".into()),
        symbol: None,
        lines,
        noise: NoiseKind::Semantic,
    }
}

fn line(old_line: Option<u32>, new_line: Option<u32>, kind: ReviewLineKind) -> ReviewLine {
    ReviewLine { old_line, new_line, kind, text: "let value = 1;".into() }
}

/// Every hunk open — the row space when nothing is collapsed.
fn open(hunks: &[ReviewHunk]) -> Vec<VisibleGroup<'_>> {
    hunks.iter().map(VisibleGroup::Hunk).collect()
}

/// Mechanical hunks collapsed into runs, the way `visible_groups` builds them.
fn collapsing_mechanical(hunks: &[ReviewHunk]) -> Vec<VisibleGroup<'_>> {
    let mut groups: Vec<VisibleGroup<'_>> = Vec::new();
    for hunk in hunks {
        if !super::super::fold::is_mechanical(hunk.noise) {
            groups.push(VisibleGroup::Hunk(hunk));
            continue;
        }
        match groups.last_mut() {
            Some(VisibleGroup::Folded(run)) => {
                run.hunks += 1;
                run.lines += hunk.lines.len();
            }
            _ => groups.push(VisibleGroup::Folded(FoldedRun {
                hunks: 1,
                lines: hunk.lines.len(),
                noise: hunk.noise,
            })),
        }
    }
    groups
}

/// Side-by-side is a *view* of the unified row space: every row says which unified row it
/// starts at, so scroll position, selection and click mapping need no second model.
#[test]
fn split_rows_pair_changes_and_carry_their_unified_index() {
    let hunks = vec![hunk(
        1,
        vec![
            line(Some(1), Some(1), ReviewLineKind::Context),
            line(Some(2), None, ReviewLineKind::Deletion),
            line(Some(3), None, ReviewLineKind::Deletion),
            line(None, Some(2), ReviewLineKind::Addition),
        ],
    )];

    let rows = visible_split_rows(&open(&hunks), 0, 8);

    // Header, the context row, the paired replacement, then the unpaired deletion.
    assert!(matches!(rows[0], SplitRow::Header { unified_index: 0, .. }));
    assert!(matches!(
        rows[1],
        SplitRow::Pair { old: SplitDiffCell::Source(old), unified_index: 1, .. }
            if old.old_line == Some(1)
    ));
    assert!(matches!(
        rows[2],
        SplitRow::Pair {
            old: SplitDiffCell::Source(old),
            new: SplitDiffCell::Source(new),
            unified_index: 2,
            ..
        } if old.old_line == Some(2) && new.new_line == Some(2)
    ));
    assert!(matches!(
        rows[3],
        SplitRow::Pair { old: SplitDiffCell::Source(old), new: SplitDiffCell::Empty, .. }
            if old.old_line == Some(3)
    ));
    assert_eq!(rows.len(), 4, "one hunk, four rendered rows");
}

#[test]
fn split_rows_preserve_an_unanchored_no_newline_marker() {
    let hunks = vec![hunk(1, vec![line(None, None, ReviewLineKind::NoNewlineMarker)])];

    let rows = visible_split_rows(&open(&hunks), 1, 4);

    assert!(matches!(
        rows.as_slice(),
        [SplitRow::Pair { old: SplitDiffCell::Source(line), .. }]
            if line.kind == ReviewLineKind::NoNewlineMarker
    ));
}

/// The window is bounded by rendered rows and positioned by the unified offset, exactly like the
/// unified projection — so a huge file never materializes.
#[test]
fn split_rows_return_only_the_requested_window() {
    let hunks = vec![hunk(
        1,
        (1..=10_000)
            .map(|number| line(Some(number), Some(number), ReviewLineKind::Context))
            .collect(),
    )];

    let rows = visible_split_rows(&open(&hunks), 9_998, 3);

    // Row 0 is the hunk header, so unified row 9_998 is source line 9_998.
    assert_eq!(rows.len(), 3);
    assert!(matches!(
        rows[0],
        SplitRow::Pair { old: SplitDiffCell::Source(line), unified_index: 9_998, .. }
            if line.old_line == Some(9_998)
    ));
}

#[test]
fn split_rows_collapse_mechanical_hunks_like_the_unified_projection() {
    let semantic = hunk(1, vec![line(Some(1), Some(1), ReviewLineKind::Context)]);
    let mut folded = hunk(10, vec![line(Some(10), Some(10), ReviewLineKind::Context)]);
    folded.noise = NoiseKind::FormattingOnly;
    let hunks = [semantic.clone(), folded];

    let rows = visible_split_rows(&collapsing_mechanical(&hunks), 0, 8);

    assert_eq!(rows.len(), 3, "the semantic hunk's header and line, then a collapsed run");
    assert!(rows[..2].iter().all(|row| row.hunk_id() == Some(&semantic.id)));
    assert!(
        matches!(rows[2], SplitRow::Folded { run, .. } if run.hunks == 1),
        "the collapsed hunk stands as one row rather than vanishing: {rows:?}"
    );
}

#[test]
fn visible_rows_include_hunk_headers_and_preserve_both_gutters() {
    let hunks = vec![hunk(
        9,
        vec![
            line(Some(9), Some(9), ReviewLineKind::Context),
            line(Some(10), None, ReviewLineKind::Deletion),
            line(None, Some(10), ReviewLineKind::Addition),
        ],
    )];

    let rows = visible_rows(&open(&hunks), 1, 2);

    assert_eq!(rows.len(), 2);
    assert!(
        matches!(rows[0], UnifiedDiffRow::Source(line) if line.old_line == Some(9) && line.new_line == Some(9))
    );
    assert!(
        matches!(rows[1], UnifiedDiffRow::Source(line) if line.old_line == Some(10) && line.new_line.is_none())
    );
    assert_eq!(gutter_width(&hunks), 2);
}

#[test]
fn hunk_headers_keep_stable_selection_identity() {
    let hunks = vec![hunk(1, vec![line(Some(1), Some(1), ReviewLineKind::Context)])];

    let rows = visible_rows(&open(&hunks), 0, 1);

    assert!(
        matches!(rows[0], UnifiedDiffRow::HunkHeader { hunk_id, .. } if hunk_id == &hunks[0].id)
    );
}

#[test]
fn a_collapsed_run_stands_in_for_its_hunks_without_materializing_them() {
    let semantic = hunk(1, vec![line(Some(1), Some(1), ReviewLineKind::Context)]);
    let mut folded = hunk(10, vec![line(Some(10), Some(10), ReviewLineKind::Context)]);
    folded.noise = NoiseKind::FormattingOnly;

    let hunks = [semantic.clone(), folded];
    let rows = visible_rows(&collapsing_mechanical(&hunks), 0, 3);

    assert_eq!(rows.len(), 3);
    assert!(
        matches!(rows[0], UnifiedDiffRow::HunkHeader { hunk_id, .. } if hunk_id == &semantic.id)
    );
    assert!(
        matches!(rows[2], UnifiedDiffRow::Folded(run) if run.hunks == 1 && run.lines == 1),
        "a collapsed hunk is a row that says what it stands for: {rows:?}"
    );
}

#[test]
fn huge_diff_projects_only_the_requested_window() {
    let lines = (1..=10_000)
        .map(|number| line(Some(number), Some(number), ReviewLineKind::Context))
        .collect();
    let hunks = vec![hunk(1, lines)];

    let rows = visible_rows(&open(&hunks), 9_998, 3);

    assert_eq!(rows.len(), 3);
    assert!(matches!(rows[0], UnifiedDiffRow::Source(line) if line.old_line == Some(9_998)));
    assert!(matches!(rows[2], UnifiedDiffRow::Source(line) if line.old_line == Some(10_000)));
    assert_eq!(gutter_width(&hunks), 5);
}

#[test]
fn terminal_cell_clipping_keeps_wide_glyphs_intact() {
    assert_eq!(clip_terminal_cells("ab界cd", 2, 3), "界c");
    assert_eq!(clip_terminal_cells("ab界cd", 3, 2), "cd");
}

#[test]
fn ellipsis_clip_marks_truncation_but_leaves_fitting_text_alone() {
    // Fits: returned unchanged, no ellipsis.
    assert_eq!(clip_terminal_cells_ellipsis("short", 10), "short");
    assert_eq!(clip_terminal_cells_ellipsis("exactfit", 8), "exactfit");
    // Truncates: one cell reserved for the ellipsis (no silent mid-word cut).
    assert_eq!(clip_terminal_cells_ellipsis("handles output", 10), "handles o…");
    // Wide glyphs are never split by the reserved-cell math.
    assert_eq!(clip_terminal_cells_ellipsis("ab界cd", 4), "ab…");
    assert_eq!(clip_terminal_cells_ellipsis("anything", 0), "");
}

#[test]
fn markers_remain_distinguishable_without_theme_color() {
    assert_eq!(marker(ReviewLineKind::Context), ' ');
    assert_eq!(marker(ReviewLineKind::Deletion), '-');
    assert_eq!(marker(ReviewLineKind::Addition), '+');
    assert_eq!(marker(ReviewLineKind::NoNewlineMarker), '\\');
}

/// A window starting at the added half of a pair starts with that pair, whatever follows it.
///
/// The break this catches: a pair carried only its earlier index, so a window starting at the
/// addition dropped the pair and began at the next context line, or held nothing at all.
#[test]
fn split_window_keeps_the_pair_containing_the_selected_addition() {
    use ReviewLineKind::{Addition, Context, Deletion};
    let starts_with_addition = |rows: &[SplitRow<'_>], new_line: u32| {
        matches!(rows.first(), Some(SplitRow::Pair { new: SplitDiffCell::Source(line), .. })
            if line.new_line == Some(new_line))
    };
    // header 0, deletion 1, addition 2, context 3
    let replaced = [hunk(
        1,
        vec![
            line(Some(1), None, Deletion),
            line(None, Some(1), Addition),
            line(Some(2), Some(2), Context),
        ],
    )];
    let rows = visible_split_rows(&open(&replaced), 2, 3);
    assert!(starts_with_addition(&rows, 1), "one replaced line");
    assert_eq!(rows.len(), 2, "the pair, then the context after it");

    // header 0, deletions 1 and 2, addition 3: the pair, then the unpaired deletion
    let longer = [hunk(
        1,
        vec![
            line(Some(1), None, Deletion),
            line(Some(2), None, Deletion),
            line(None, Some(1), Addition),
        ],
    )];
    let rows = visible_split_rows(&open(&longer), 3, 4);
    assert!(starts_with_addition(&rows, 1), "a longer run, no trailing context");
    assert!(matches!(rows.get(1), Some(SplitRow::Pair { new: SplitDiffCell::Empty, .. })));

    assert!(visible_split_rows(&open(&replaced), 2, 0).is_empty(), "zero height is nothing");
}

/// In a run of three replaced lines, a cursor on any deletion starts the window at that deletion's
/// own pair, not the first pair of the run.
///
/// The break this catches: the window started at the first pair whose later half reached the
/// offset, which for a deletion deep in a run is the run's first pair, so the cursor row drew
/// lower and lower until it left the pane.
#[test]
fn split_window_starts_at_the_pair_holding_a_selected_deletion() {
    use ReviewLineKind::{Addition, Deletion};
    // header 0, deletions 1..=3, additions 4..=6
    let run = [hunk(
        1,
        vec![
            line(Some(1), None, Deletion),
            line(Some(2), None, Deletion),
            line(Some(3), None, Deletion),
            line(None, Some(1), Addition),
            line(None, Some(2), Addition),
            line(None, Some(3), Addition),
        ],
    )];
    for (offset, old_line) in [(1, 1), (2, 2), (3, 3), (4, 1), (5, 2), (6, 3)] {
        let rows = visible_split_rows(&open(&run), offset, 1);
        assert!(
            matches!(rows.first(), Some(SplitRow::Pair { old: SplitDiffCell::Source(line), .. })
                if line.old_line == Some(old_line)),
            "offset {offset} should start on the pair of old line {old_line}"
        );
    }
}
