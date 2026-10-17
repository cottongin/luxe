//! Diff — tabs, wrapping and scrolling in the diff pane.

use super::*;

/// Replaces the first hunk with lines indented one tab deeper each, which is what exposed the
/// defect: the rendered width fell one cell behind the measured width per tab.
pub(super) fn seed_tab_indented_hunk(app: &mut crate::app::App) {
    use luxe_core::guided_review::{ReviewLine, ReviewLineKind};
    let Some(hunk) = app
        .guided_review
        .as_mut()
        .and_then(|review| review.files.first_mut().and_then(|file| file.hunks.first_mut()))
    else {
        panic!("the fixture has a hunk to replace");
    };
    hunk.lines = (1..=4u32)
        .flat_map(|depth| {
            let indent = "\t".repeat(depth as usize);
            [
                ReviewLine {
                    old_line: Some(depth * 2),
                    new_line: None,
                    kind: ReviewLineKind::Deletion,
                    text: format!("{indent}<div class=\"old-{depth}\">"),
                },
                ReviewLine {
                    old_line: None,
                    new_line: Some(depth * 2),
                    kind: ReviewLineKind::Addition,
                    text: format!("{indent}<div class=\"new-{depth}\">"),
                },
            ]
        })
        .collect();
}

/// The column each add/delete row's tint reaches, one entry per tinted row.
///
/// Only the diff's own backgrounds count — row tint and the deeper intra-line wash — because a
/// selected row and a hunk header wear the selection tint, which is a different claim and a
/// different width.
pub(super) fn diff_tint_run_ends(
    app: &crate::app::App,
    buffer: &ratatui::buffer::Buffer,
) -> Vec<u16> {
    let area = buffer.area;
    let styles = &app.theme_styles;
    let hues = [
        styles.diff_add_bg.bg,
        styles.diff_del_bg.bg,
        styles.diff_add_emphasis.bg,
        styles.diff_del_emphasis.bg,
    ];
    let mut ends = Vec::new();
    for y in 0..area.height {
        let mut open = false;
        for x in 0..area.width {
            let tinted = hues.contains(&Some(buffer[(x, y)].bg));
            if open && !tinted {
                ends.push(x);
            }
            open = tinted;
        }
        if open {
            ends.push(area.width);
        }
    }
    ends
}

/// A tab is measured as zero cells and dropped by the buffer, so a row's tint used to stop one
/// cell short for every tab in it — a staircase down the right edge of any block of indented
/// code, and in side by side a column rule that wandered with the indentation.
#[test]
fn tab_indentation_does_not_ragged_the_row_tints() {
    use ratatui::{backend::TestBackend, Terminal};

    // One edge unified, one per column side by side — and never one per indentation depth,
    // which is what the defect produced.
    for (split, edges) in [(false, 1usize), (true, 2)] {
        let (width, height) = (160u16, 30u16);
        let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
        seed_tab_indented_hunk(&mut app);
        app.guided_review_workspace.layout = if split {
            luxe_core::config::DiffLayout::Split
        } else {
            luxe_core::config::DiffLayout::Unified
        };
        app.update(Msg::TerminalResized { width, height });
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
        terminal.draw(|frame| crate::view::render(frame, &mut app)).expect("render");
        let buffer = terminal.backend().buffer().clone();

        let mut columns = diff_tint_run_ends(&app, &buffer);
        assert!(columns.len() >= 4, "split={split}: several tinted diff rows are drawn");
        columns.sort_unstable();
        columns.dedup();
        assert_eq!(
            columns.len(),
            edges,
            "split={split}: tinted rows end at {columns:?} — indentation is moving the edge"
        );
    }
}

/// The indentation itself has to survive, not just the arithmetic about it: a tab-indented
/// file used to render flush left because the buffer drops control characters.
#[test]
fn tab_indentation_reaches_the_screen() {
    use ratatui::{backend::TestBackend, Terminal};

    let (width, height) = (160u16, 30u16);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    seed_tab_indented_hunk(&mut app);
    app.guided_review_workspace.layout = luxe_core::config::DiffLayout::Unified;
    app.update(Msg::TerminalResized { width, height });
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal.draw(|frame| crate::view::render(frame, &mut app)).expect("render");
    let rendered: String =
        terminal.backend().buffer().content.iter().map(ratatui::buffer::Cell::symbol).collect();
    assert!(
        rendered.contains("            <div class=\"new-3\">"),
        "three tabs draw as twelve cells of indent: {rendered}"
    );
}

/// Each row of a rendered frame, trimmed, so a phrase cannot match across a line break.
/// The frame's rows, and where the terminal cursor is with the text of its row up to it.
pub(super) fn frame_with_cursor(
    app: &mut crate::app::App,
    width: u16,
    height: u16,
) -> (Vec<String>, (u16, u16), String) {
    use ratatui::{backend::Backend, backend::TestBackend, Terminal};
    app.update(Msg::TerminalResized { width, height });
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal.draw(|frame| crate::view::render(frame, app)).expect("render");
    let at = terminal.backend_mut().get_cursor_position().expect("cursor");
    let buffer = terminal.backend().buffer();
    let rows = (0..height)
        .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect();
    let before = (0..at.x).map(|x| buffer[(x, at.y)].symbol()).collect::<String>();
    (rows, (at.x, at.y), before)
}

pub(super) fn frame_rows(app: &mut crate::app::App, width: u16, height: u16) -> Vec<String> {
    use ratatui::{backend::TestBackend, Terminal};
    app.update(Msg::TerminalResized { width, height });
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal.draw(|frame| crate::view::render(frame, app)).expect("render");
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width).map(|x| buffer[(x, y)].symbol()).collect::<String>().trim_end().to_owned()
        })
        .collect()
}

/// A review titled with a whole paragraph is cut, not left to eat the top of the guide.
///
/// The title is usually the session's first message, which is a sentence as often as it is a
/// name. Left to wrap it took four lines off the top of the pane and still ended mid-word,
/// which reads as a rendering fault rather than as a long title.
#[test]
fn a_long_review_title_is_cut_with_an_ellipsis() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let sentence = "Bulk delete (for example, selecting several messages with space or \
                        consecutive items with v) seems to remove messages from the list \
                        immediately, then some of them come back a moment later";
    app.guided_review.as_mut().expect("review").context.title = sentence.into();
    app.update(crate::app::Msg::TerminalResized { width: 180, height: 44 });

    let mut terminal = Terminal::new(TestBackend::new(180, 44)).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("Bulk delete"), "the title still opens as itself: {text}");
    assert!(!text.contains("come back a moment later"), "and does not run on: {text}");
    assert!(text.contains('\u{2026}'), "it is cut with an ellipsis: {text}");
}

/// Injects a long addition line into the fixture's first hunk for wrap/scroll tests.
pub(super) fn seed_long_line(app: &mut crate::app::App) -> String {
    let long = "let very_long_identifier = compute_something(alpha, beta, gamma, delta, epsilon, zeta, eta);".to_string();
    let review = app.guided_review.as_mut().expect("fixture review");
    let hunk = review.files[0].hunks.first_mut().expect("fixture hunk");
    let line = hunk
        .lines
        .iter_mut()
        .find(|line| line.kind == luxe_core::guided_review::ReviewLineKind::Addition)
        .expect("an addition line");
    line.text = long.clone();
    long
}

#[test]
fn word_wrap_soft_wraps_long_diff_lines_with_a_continuation_glyph() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    seed_long_line(&mut app);
    app.guided_review_workspace.word_wrap = true;
    let backend = TestBackend::new(90, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render wrapped diff");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    // A continuation glyph marks the wrapped rows, and the tail of the long line is visible
    // (it would be clipped off-screen under horizontal scroll).
    assert!(text.contains('↳'), "wrapped rows carry a continuation glyph: {text}");
    assert!(text.contains("epsilon") || text.contains("eta);"), "wrapped tail visible: {text}");
}

#[test]
fn long_file_paths_truncate_to_one_scannable_row_with_colored_counts() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let long_path = "crates/luxe-tui/src/view/guided_review/context.rs";
    {
        let review = app.guided_review.as_mut().expect("review");
        let mut file = review.files[0].clone();
        file.id = FileId::from_path_lineage(None, Some(Path::new(long_path)));
        file.new_path = Some(long_path.into());
        file.old_path = Some(long_path.into());
        file.additions = Some(151);
        file.deletions = Some(30);
        review.chapters[0].items.push(ReviewItemRef::File(file.id.clone()));
        review.files.push(file);
    }
    let backend = TestBackend::new(150, 40);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render file list");
    let buffer = terminal.backend().buffer().clone();
    let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();

    // The 42-col rail can't fit the full path, so it truncates behind `…/` with the filename
    // (and its colored counts) kept on one row — the full deep prefix is elided.
    assert!(text.contains("…/"), "long path is truncated: {text}");
    assert!(text.contains("context.rs +151"), "filename + counts stay on one row: {text}");
    assert!(
        !text.contains("luxe-tui/src/view/guided_review/context.rs"),
        "the full deep path is not shown verbatim in the rail"
    );
    // The change counts are colored (green additions), matching the dock. Scope the check to
    // the long file's rail row so it can't match a `+` in the diff pane or the metadata stats.
    let row = (0..buffer.area.height)
        .find(|&y| {
            (0..40u16).map(|x| buffer[(x, y)].symbol()).collect::<String>().contains("context.rs")
        })
        .expect("the long file's rail row renders");
    let plus_x = (0..42u16)
        .find(|&x| buffer[(x, row)].symbol() == "+")
        .expect("a `+` additions count in the rail row");
    assert_eq!(buffer[(plus_x, row)].fg, app.theme_styles.success.fg.expect("success color"));
}

#[test]
fn horizontal_scroll_mode_clips_long_lines_without_a_continuation_glyph() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    seed_long_line(&mut app);
    app.guided_review_workspace.word_wrap = false;
    let backend = TestBackend::new(90, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render scrolled diff");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(!text.contains('↳'), "scroll mode shows no continuation glyph: {text}");
}

/// Scrolled all the way right, the longest line in the file ends at the pane's edge — even
/// when it is in a hunk other than the one under the cursor, which the pane draws all the same.
/// Both halves matter: one cell short of the end must still be hiding its last character, or
/// the bound is overshooting into blank space.
#[test]
fn scrolling_right_reaches_the_end_of_every_hunk_in_both_layouts() {
    use luxe_core::guided_review::{HunkId, ReviewLineKind};
    use ratatui::{backend::TestBackend, Terminal};

    for (width, split) in [(160u16, true), (100, false)] {
        let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
        app.guided_review_workspace.word_wrap = false;
        let tail = "QTAIL";
        let (file_id, first_hunk) = {
            let review = app.guided_review.as_mut().expect("review");
            let file = &mut review.files[0];
            let mut later = file.hunks[0].clone();
            later.id = HunkId::from_content(&file.id, "later", "a later hunk");
            let line = later
                .lines
                .iter_mut()
                .find(|line| line.kind == ReviewLineKind::Addition)
                .expect("an addition");
            line.text = format!("{}{tail}", "x".repeat(300));
            file.hunks.push(later);
            (file.id.clone(), file.hunks[0].id.clone())
        };
        app.update(Msg::TerminalResized { width, height: 60 });
        let position = &mut app.guided_review_workspace.position;
        position.file_id = Some(file_id);
        position.hunk_id = Some(first_hunk);
        position.line_offset = 0;
        let files = app.guided_review.as_ref().expect("review").files.clone();
        assert_eq!(app.guided_review_workspace.split_layout_showing(&files), split);

        for _ in 0..100 {
            app.update(Msg::GuidedReviewColumnNext);
        }
        let drawn = |app: &crate::app::App| {
            let mut terminal = Terminal::new(TestBackend::new(width, 60)).expect("test terminal");
            terminal.draw(|frame| render(frame, frame.area(), app)).expect("render");
            terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect::<String>()
        };
        assert!(drawn(&app).contains(tail), "split={split}: the line's end is reachable");

        app.guided_review_workspace.diff_column_offset -= 1;
        let short = drawn(&app);
        assert!(
            short.contains("QTAI") && !short.contains(tail),
            "split={split}: one cell back still hides the last character"
        );
    }
}
