//! Layout: split preference and fallback, columns, and horizontal scrolling.

use super::super::*;
use super::{changed_file, text_file};

/// Asking for two columns on a pane that cannot seat them is still asking. The preference has to
/// survive the fallback, or widening the terminal would silently leave the diff unified.
#[test]
fn split_preference_survives_a_narrow_width_fallback() {
    let file = changed_file("src/split.rs");
    let files = std::slice::from_ref(&file);
    let mut workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: None,
            file_id: Some(file.id.clone()),
            hunk_id: None,
            line_offset: 0,
        },
        diff_viewport_columns: 40,
        ..Default::default()
    };

    workspace.toggle_split_preference(files);
    assert_eq!(workspace.layout, DiffLayout::Split);
    assert!(!workspace.split_layout_showing(files), "40 cells are not two columns on any setting");

    workspace.diff_viewport_columns = 200;
    assert!(workspace.split_layout_showing(files), "and the preference was still standing");
    assert_eq!(workspace.layout, DiffLayout::Split);
}

/// The toggle records the opposite of what is *showing*, so it always changes the screen. Pressing
/// it on a pane `auto` had declined now pairs the sides rather than doing nothing: `split` asks
/// for less width than `auto` does, which is the whole reason the two are different settings.
#[test]
fn the_toggle_pairs_the_sides_on_a_pane_auto_turned_down() {
    let file = changed_file("src/cramped.rs");
    let files = std::slice::from_ref(&file);
    let mut workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: None,
            file_id: Some(file.id.clone()),
            hunk_id: None,
            line_offset: 0,
        },
        diff_viewport_columns: 80,
        ..Default::default()
    };
    assert!(!workspace.split_layout_showing(files), "auto leaves a cramped pane unified");

    workspace.toggle_split_preference(files);
    assert!(workspace.split_layout_showing(files), "and the key the footer offers does something");

    workspace.toggle_split_preference(files);
    assert_eq!(workspace.layout, DiffLayout::Unified, "pressing it again puts it back");
    assert!(!workspace.split_layout_showing(files));
}

/// `auto` is the width rule; `split` is someone overruling it; `unified` is neither. One setting,
/// the same three meanings the tool cards and the peek pane read.
#[test]
fn the_configured_layout_decides_what_the_pane_can_do_with_its_width() {
    let file = changed_file("src/layouts.rs");
    let files = std::slice::from_ref(&file);
    let workspace = |layout, columns| GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: None,
            file_id: Some(file.id.clone()),
            hunk_id: None,
            line_offset: 0,
        },
        layout,
        diff_viewport_columns: columns,
        ..Default::default()
    };

    assert!(workspace(DiffLayout::Auto, 200).split_layout_showing(files));
    assert!(!workspace(DiffLayout::Auto, 80).split_layout_showing(files), "auto wants a column");
    assert!(
        workspace(DiffLayout::Split, 80).split_layout_showing(files),
        "split takes a cramped one"
    );
    assert!(
        !workspace(DiffLayout::Split, 40).split_layout_showing(files),
        "but not a hopeless one"
    );
    assert!(!workspace(DiffLayout::Unified, 200).split_layout_showing(files));
}

/// A one-sided file has nothing for the second column, on any width, under any setting.
#[test]
fn a_one_sided_file_is_never_drawn_side_by_side() {
    let added = text_file("src/new.rs");
    let workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: None,
            file_id: Some(added.id.clone()),
            hunk_id: None,
            line_offset: 0,
        },
        layout: DiffLayout::Split,
        diff_viewport_columns: 400,
        ..Default::default()
    };

    assert!(!workspace.split_layout_active_for(&added));
}

#[test]
fn resize_split_fallback_and_restoration_preserve_the_selected_hunk_anchor() {
    let file = changed_file("src/resize.rs");
    let position = ReviewPosition {
        chapter_id: None,
        file_id: Some(file.id.clone()),
        hunk_id: Some(file.hunks[0].id.clone()),
        line_offset: 1,
    };
    let files = std::slice::from_ref(&file);
    let mut workspace = GuidedReviewWorkspace {
        position: position.clone(),
        layout: DiffLayout::Auto,
        ..Default::default()
    };

    workspace.set_terminal_size(200, 30);
    assert!(workspace.split_layout_showing(files));
    workspace.set_terminal_size(100, 30);

    assert!(!workspace.split_layout_showing(files));
    assert_eq!(workspace.layout, DiffLayout::Auto);
    assert_eq!(workspace.position, position);

    workspace.set_terminal_size(200, 30);
    assert!(workspace.split_layout_showing(files));
    assert_eq!(workspace.position, position);
}

#[test]
fn home_resets_the_diff_column_offset() {
    let mut workspace = GuidedReviewWorkspace { diff_column_offset: 24, ..Default::default() };

    workspace.reset_column_offset();
    assert_eq!(workspace.diff_column_offset, 0);
}

#[test]
fn column_navigation_is_bounded_by_terminal_cell_width() {
    let mut file = text_file("src/columns.rs");
    file.hunks[0].lines[0].text = "wide-界-source".into();
    let mut workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: None,
            file_id: Some(file.id.clone()),
            hunk_id: Some(file.hunks[0].id.clone()),
            line_offset: 0,
        },
        ..Default::default()
    };
    // 18 diff cells, less a unified row's chrome around its one-digit numbers, leaves 11 for a
    // source 14 cells wide: the wide glyph counts twice
    workspace.set_terminal_size(20, 8);

    workspace.next_column(std::slice::from_ref(&file));
    assert_eq!(workspace.diff_column_offset, 3);
    workspace.previous_column();
    assert_eq!(workspace.diff_column_offset, 0);

    file.hunks[0].lines[0].text = "x".repeat(120);
    workspace.diff_viewport_columns = 100;
    workspace.next_column(std::slice::from_ref(&file));
    assert_eq!(workspace.diff_column_offset, COLUMN_SCROLL_STEP);
}

#[test]
fn horizontal_offset_clamps_to_the_active_split_or_unified_presentation() {
    let mut file = changed_file("src/split-columns.rs");
    file.hunks[0].lines[0].text = "x".repeat(90);
    let mut workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: None,
            file_id: Some(file.id.clone()),
            hunk_id: Some(file.hunks[0].id.clone()),
            line_offset: 0,
        },
        layout: DiffLayout::Split,
        diff_viewport_columns: 100,
        diff_column_offset: 90,
        ..Default::default()
    };

    // Each column gets (100 - 5) / 2 = 47 cells, three of them gutter: 44 of source for 90
    workspace.clamp_column_offset(std::slice::from_ref(&file));
    assert_eq!(workspace.diff_column_offset, 46);

    workspace.layout = DiffLayout::Unified;
    workspace.clamp_column_offset(std::slice::from_ref(&file));
    assert_eq!(workspace.diff_column_offset, 0);
}
