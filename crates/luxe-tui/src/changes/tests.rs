use super::*;
use luxe_core::config::{MIN_SPLIT_COLUMN, SPLIT_FLOOR_COLUMN};
use luxe_core::FileChangeStatus;

fn changed(path: &str) -> ChangedFile {
    ChangedFile {
        path: path.into(),
        status: FileChangeStatus::Modified,
        additions: Some(1),
        deletions: Some(0),
    }
}

fn view() -> ChangesView {
    ChangesView::new(
        "src/b.rs".into(),
        vec![changed("src/a.rs"), changed("src/b.rs"), changed("src/c.rs")],
    )
}

#[test]
fn stepping_wraps_at_both_ends() {
    let mut view = view();
    assert_eq!(view.step(true).as_deref(), Some("src/c.rs"));
    assert_eq!(view.step(false).as_deref(), Some("src/a.rs"));

    view.show("src/c.rs".into());
    assert_eq!(view.step(true).as_deref(), Some("src/a.rs"), "wraps forward");
    view.show("src/a.rs".into());
    assert_eq!(view.step(false).as_deref(), Some("src/c.rs"), "wraps back");
}

#[test]
fn a_single_changed_file_has_nowhere_to_step() {
    let view = ChangesView::new("only.rs".into(), vec![changed("only.rs")]);
    assert_eq!(view.step(true), None);
    assert_eq!(view.position(), Some(0));
}

/// A capture the reader has already moved on from must not replace what they are looking at.
#[test]
fn a_superseded_capture_is_discarded() {
    let mut view = view();
    let stale = FileDiffRequestId::new();
    let current = FileDiffRequestId::new();
    view.request = Some(current.clone());

    let diff = FileDiff { path: "src/b.rs".into(), file: None, untracked: false };
    assert!(!view.accept(&stale, diff.clone()), "a stale request id is ignored");
    assert_eq!(view.load, ChangesLoad::Loading);

    assert!(view.accept(&current, diff));
    assert_eq!(view.load, ChangesLoad::Unchanged);
    assert_eq!(view.request, None, "the request is settled once it lands");
}

/// The same guard from the other side: the right request, the wrong file.
#[test]
fn a_capture_for_another_file_is_discarded() {
    let mut view = view();
    let request = FileDiffRequestId::new();
    view.request = Some(request.clone());
    let diff = FileDiff { path: "src/a.rs".into(), file: None, untracked: false };
    assert!(!view.accept(&request, diff));
    assert_eq!(view.load, ChangesLoad::Loading);
}

#[test]
fn a_failure_lands_under_the_same_guard() {
    let mut view = view();
    let request = FileDiffRequestId::new();
    view.request = Some(request.clone());

    assert!(!view.fail(&request, "src/a.rs", FileDiffFailure::TooLarge), "wrong file");
    assert!(view.fail(&request, "src/b.rs", FileDiffFailure::TooLarge));
    assert_eq!(view.load, ChangesLoad::Failed(FileDiffFailure::TooLarge));
}

#[test]
fn scrolling_clamps_to_what_the_last_frame_could_show() {
    let mut view = view();
    view.scroll_max.set(10);
    view.viewport_rows.set(4);

    view.scroll_by(false, SCROLL_STEP);
    assert_eq!(view.scroll, SCROLL_STEP);
    view.page(false);
    assert_eq!(view.scroll, SCROLL_STEP + 4);
    view.jump(false);
    assert_eq!(view.scroll, 10);
    view.scroll_by(false, 100);
    assert_eq!(view.scroll, 10, "never past the end");
    view.jump(true);
    assert_eq!(view.scroll, 0);
    view.scroll_by(true, 100);
    assert_eq!(view.scroll, 0, "never above the top");
}

/// Wrap is on to begin with, because the pane cannot scroll sideways — clipping here would put
/// content out of reach rather than one keystroke away.
#[test]
fn wrapping_starts_on_and_toggling_it_holds_the_readers_place() {
    let mut view = view();
    assert!(view.word_wrap, "the pane wraps by default");
    view.scroll_max.set(40);
    view.scroll_by(false, 12);
    view.toggle_word_wrap();
    assert!(!view.word_wrap);
    assert_eq!(view.scroll, 12, "the offset is a row, which wrapping does not move");
}

/// The width has the last word: a preference cannot conjure a second readable column, and a
/// one-sided file has nothing to put in it. The setting is the same one the tool cards and the
/// review read — this pane used to read none.
#[test]
fn side_by_side_needs_room_and_two_sides() {
    use luxe_core::guided_review::{
        FileId, HunkId, LineRange, NoiseKind, ReviewFileStatus, ReviewHunk, ReviewLine,
        ReviewLineKind,
    };
    let file = |kinds: &[ReviewLineKind]| {
        let id = FileId::from_path_lineage(None, Some(std::path::Path::new("a.rs")));
        ReviewFile {
            id: id.clone(),
            old_path: Some("a.rs".into()),
            new_path: Some("a.rs".into()),
            status: ReviewFileStatus::Modified,
            language: None,
            additions: Some(1),
            deletions: Some(1),
            hunks: vec![ReviewHunk {
                id: HunkId::from_content(&id, "h", "h"),
                old_range: LineRange::new(1, 1),
                new_range: LineRange::new(1, 1),
                heading: None,
                symbol: None,
                lines: kinds
                    .iter()
                    .map(|kind| ReviewLine {
                        old_line: Some(1),
                        new_line: Some(1),
                        kind: *kind,
                        text: "x".into(),
                    })
                    .collect(),
                noise: NoiseKind::Semantic,
            }],
            binary: false,
            generated: false,
            non_text: None,
        }
    };
    let replaced = file(&[ReviewLineKind::Deletion, ReviewLineKind::Addition]);
    let added = file(&[ReviewLineKind::Addition]);
    let view = view();

    let auto = DiffLayout::Auto;
    assert!(view.split_active(auto, MIN_SPLIT_COLUMN, Some(&replaced)), "room and two sides");
    assert!(
        !view.split_active(auto, MIN_SPLIT_COLUMN - 1, Some(&replaced)),
        "a column too narrow to read code stays unified"
    );
    assert!(
        !view.split_active(auto, MIN_SPLIT_COLUMN, Some(&added)),
        "a pure addition has one side, so two columns would be half empty"
    );
    assert!(!view.split_active(auto, usize::MAX, None), "nothing shows before it loads");

    // The configured preference reaches this pane, which is the whole point of it being one
    // setting: `unified` holds on a pane wide enough to split, `split` pairs one that `auto`
    // would have left alone.
    assert!(!view.split_active(DiffLayout::Unified, usize::MAX, Some(&replaced)));
    assert!(view.split_active(DiffLayout::Split, SPLIT_FLOOR_COLUMN, Some(&replaced)));

    // A pin is this pane's own answer, and still cannot beat the width.
    let mut pinned = view;
    pinned.split = Some(true);
    assert!(pinned.split_active(DiffLayout::Unified, MIN_SPLIT_COLUMN, Some(&replaced)));
    assert!(!pinned.split_active(auto, 4, Some(&replaced)));
    pinned.split = Some(false);
    assert!(!pinned.split_active(DiffLayout::Split, usize::MAX, Some(&replaced)));
}

/// The toggle flips what is *showing*, so the first press always changes the screen — even
/// where the width had already overruled the preference.
#[test]
fn the_layout_toggle_always_changes_what_is_on_screen() {
    let mut wide = view();
    wide.side_by_side.set(true);
    wide.toggle_split();
    assert_eq!(wide.split, Some(false));

    // Auto had resolved to unified because the pane was narrow; one press asks for split.
    let mut narrow = view();
    narrow.side_by_side.set(false);
    narrow.toggle_split();
    assert_eq!(narrow.split, Some(true));
}

/// The snapshot refreshes on every tool call. Reordering under the pane is expected; losing
/// the reader's place is not.
#[test]
fn a_refreshed_snapshot_keeps_the_reader_where_they_were() {
    let mut view = view();
    view.resync(vec![changed("src/c.rs"), changed("src/b.rs")]);
    assert_eq!(view.path, "src/b.rs");
    assert_eq!(view.position(), Some(1));

    // Even when the visible file is no longer in the list at all — reverted under the pane.
    view.resync(vec![changed("src/c.rs")]);
    assert_eq!(view.path, "src/b.rs");
    assert_eq!(view.position(), None);
    assert_eq!(view.step(true).as_deref(), Some("src/c.rs"), "stepping lands back on the list");

    // And with nothing left to step to, the keys go quiet rather than guessing.
    view.resync(Vec::new());
    assert_eq!(view.step(true), None);
    assert_eq!(view.step(false), None);
}
