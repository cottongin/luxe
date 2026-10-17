//! Composer — the comment box, as tall as its field and placed where typing lands.

use super::*;

/// The composer is as tall as its field, which is three rows at least, and sits centered.
#[test]
fn the_composer_is_as_tall_as_its_field_and_centered() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture");
    let files = app.guided_review.as_ref().expect("fixture review").files.clone();
    app.guided_review_workspace.begin_finding_draft(&files);
    let rows = frame_rows(&mut app, 120, 40);
    let (top, bottom) = modal_rows(&rows, "Comment on this");
    let label = rows.iter().position(|row| row.contains("Comment:")).expect("label");
    assert_eq!(label, top + 3, "location, gap, label");
    assert_eq!(bottom - label - 1, 3, "a three-row field: {}", rows.join("\n"));
    let lines = (1..=6).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
    app.guided_review_workspace.finding_draft.as_mut().expect("draft").set_body(lines);
    let rows = frame_rows(&mut app, 120, 40);
    let (top, bottom) = modal_rows(&rows, "Comment on this");
    let label = rows.iter().position(|row| row.contains("Comment:")).expect("label");
    assert_eq!(bottom - label - 1, 6, "grows with what is written: {}", rows.join("\n"));
    let corner = rows[top].chars().position(|c| matches!(c, '╭' | '┌')).unwrap();
    assert_eq!(corner, (120 - 88) / 2, "{}", rows[top]);
    assert!(top.abs_diff(rows.len() - 1 - bottom) <= 3, "{top} vs {bottom}");

    // An error says so under the field, and the box grows by its row
    app.guided_review_workspace.finding_draft.as_mut().expect("draft").error =
        Some("Write something first.".into());
    let rows = frame_rows(&mut app, 120, 40);
    let (_, grown) = modal_rows(&rows, "Comment on this");
    assert!(rows[grown - 1].contains("Write something first."), "{}", rows.join("\n"));
    let label = rows.iter().position(|row| row.contains("Comment:")).expect("label");
    assert_eq!(grown - label - 1, 7, "six rows of field and the error");
}

/// Writing a reply adds its label and its field under the list, and the box stays centered.
#[test]
fn a_reply_adds_its_field_under_the_list() {
    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewOwnPullRequest).expect("fixture app");
    // The second thread has no reply written yet, so writing one hides nothing above the field
    app.guided_review_workspace.thread_answers.as_mut().expect("preview").selected = 1;
    let rows = frame_rows(&mut app, 160, 60);
    let (top, bottom) = modal_rows(&rows, "Answer on GitHub");
    let before = bottom - top;
    app.guided_review_workspace.thread_answers.as_mut().expect("preview").replying =
        Some(crate::editor::Editor::new());
    let rows = frame_rows(&mut app, 160, 60);
    let (top, bottom) = modal_rows(&rows, "Answer on GitHub");
    assert_eq!(bottom - top, before + 5, "a gap, a label and a three-row field");
    assert!(top > 5 && top.abs_diff(rows.len() - 1 - bottom) <= 3, "{top} vs {bottom}");
}

#[test]
fn the_composer_is_a_location_and_a_box_to_type_in() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let files = app.guided_review.as_ref().expect("fixture review").files.clone();
    app.guided_review_workspace.begin_finding_draft(&files);

    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render finding draft");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("Comment on this"));
    // The location names the file, not just a bare side/line pair — and names only itself:
    // "Anchor:" was a word for the mechanism, not for the place.
    assert!(text.contains("src/notification.rs"), "{text}");
    assert!(!text.contains("Anchor:"));
    // Nothing to pick. The verb already said what this asks of the agent.
    assert!(!text.contains("Severity"));
    assert!(!text.contains("severity"));
    assert!(!text.contains("Proposed fix"));
    assert!(text.contains("ctrl+enter save"));
    assert!(text.contains("esc cancel"));
}

/// Severity used to be the only thing on screen saying which verb had opened the box.
///
/// It is gone, and a question still does something a comment does not — the agent answers it
/// and is forbidden to edit, and it leaves the moment it is saved instead of waiting for
/// `Alt+U`. If the composer looks identical either way, the reviewer has no way to notice they
/// pressed the wrong key until the agent has already rewritten the code they asked about.
#[test]
fn asking_and_commenting_do_not_open_the_same_looking_box() {
    use ratatui::{backend::TestBackend, Terminal};

    let render_draft = |ask: bool| {
        let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture");
        let files = app.guided_review.as_ref().expect("fixture review").files.clone();
        if ask {
            app.guided_review_workspace.begin_question(&files);
        } else {
            app.guided_review_workspace.begin_finding_draft(&files);
        }
        let mut terminal = Terminal::new(TestBackend::new(100, 28)).expect("test terminal");
        terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render composer");
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect::<String>()
    };

    let asked = render_draft(true);
    assert!(asked.contains("Ask the agent about this"), "{asked}");
    assert!(asked.contains("Question: "), "{asked}");
    assert!(asked.contains("ctrl+enter ask the agent"), "a question does not wait for alt+u");

    let commented = render_draft(false);
    assert!(commented.contains("Comment on this"), "{commented}");
    assert!(commented.contains("Comment: "), "{commented}");
    assert!(commented.contains("ctrl+enter save"), "a comment batches");
}

/// The composer shows the terminal's cursor where typing lands, on a second line as much as
/// the first, so a comment can be edited rather than only appended to.
#[test]
fn the_composer_shows_its_cursor_where_typing_lands() {
    use ratatui::{backend::Backend, backend::TestBackend, Terminal};
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture");
    let files = app.guided_review.as_ref().expect("fixture review").files.clone();
    app.guided_review_workspace.begin_finding_draft(&files);
    let draft = app.guided_review_workspace.finding_draft.as_mut().expect("draft");
    draft.set_body("first line\nsecond");
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render composer");
    let at = terminal.backend_mut().get_cursor_position().expect("cursor");
    let buffer = terminal.backend().buffer().clone();
    let row: String = (0..100).map(|x| buffer[(x, at.y)].symbol().to_string()).collect::<String>();
    let before: String =
        (0..at.x).map(|x| buffer[(x, at.y)].symbol().to_string()).collect::<String>();
    assert!(row.contains("second"), "the cursor is on the line being written: {row}");
    assert!(before.trim_end().ends_with("second"), "right after it: {before:?}");
}
