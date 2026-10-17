//! Context — the header, the rail and what they report about checks and outcomes.

use super::*;

/// The rail's files are a list you pick from, so the chosen one wears the slab — cut against
/// the rail's *own* fill, and without moving off the column it holds when the cursor is
/// somewhere else.
#[test]
fn the_rails_chosen_file_row_is_cut_into_a_slab_on_its_own_fill() {
    use ratatui::{backend::TestBackend, Terminal};
    crate::glyphs::set_tier(crate::glyphs::GlyphTier::Nerd);
    // Every bundled theme paints the rail the same color as the app background, which would
    // let a cap cut against the wrong canvas pass unnoticed. A theme is free to raise it.
    const RAIL: ratatui::style::Color = ratatui::style::Color::Rgb(0x21, 0x1e, 0x1a);

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewWide).expect("fixture app");
    app.theme_styles.tool_bg = ratatui::style::Style::new().bg(RAIL);
    let draw = |app: &App| {
        let mut terminal = Terminal::new(TestBackend::new(160, 32)).expect("test terminal");
        terminal.draw(|frame| render(frame, frame.area(), app)).expect("render the rail");
        terminal.backend().buffer().clone()
    };
    // Only the rail: the diff pane beside it names the same file in its own header.
    let rail = crate::workspace_layout::LEFT_RAIL_WIDTH;
    let placement = |buffer: &ratatui::buffer::Buffer| -> (u16, u16) {
        (0..buffer.area.height)
            .find_map(|y| {
                let row: String = (0..rail).map(|x| buffer[(x, y)].symbol()).collect();
                row.find("notification.rs")
                    .map(|byte| (unicode_width::UnicodeWidthStr::width(&row[..byte]) as u16, y))
            })
            .expect("the rail lists the chapter's file")
    };

    let chosen = draw(&app);
    let (name_x, row) = placement(&chosen);
    app.guided_review_workspace.position.file_id = None;
    assert_eq!(
        placement(&draw(&app)).0,
        name_x,
        "the frame is spent either way, so the name holds its column"
    );

    let line: String = (0..rail).map(|x| chosen[(x, row)].symbol()).collect();
    let cap = crate::widgets::Cap::Open.glyph();
    let cut = line.find(cap).expect("the chosen row opens on a cut");
    let cut_x = unicode_width::UnicodeWidthStr::width(&line[..cut]) as u16;
    assert!(cut_x < name_x, "and the cut opens before the name: {line:?}");
    assert_eq!(
        chosen[(cut_x, row)].bg,
        RAIL,
        "the cut tapers into the rail, not into the color behind it"
    );
}

#[test]
fn wide_header_names_the_durable_review_outcome() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    app.review.session_mut().expect("review session").outcome =
        Some(luxe_core::guided_review::ReviewOutcome::ChangesRequested);

    let backend = TestBackend::new(160, 32);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render review outcome");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("Outcome: changes requested"));
}

#[test]
fn narrow_context_keeps_the_durable_outcome_before_clipped_context() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    app.review.session_mut().expect("review session").outcome =
        Some(luxe_core::guided_review::ReviewOutcome::Commented);

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render narrow review");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("Outcome: commented"));
}

#[test]
fn check_status_shows_on_the_wide_rail_and_medium_keeps_core_stats() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    app.guided_review.as_mut().expect("fixture review").checks.checks.push(ReviewCheck {
        name: "cargo test".into(),
        status: ReviewCheckStatus::Passed,
        summary: None,
    });
    let wide_backend = TestBackend::new(160, 32);
    let mut wide_terminal = Terminal::new(wide_backend).expect("wide terminal");
    wide_terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render wide header");
    let wide: String =
        wide_terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();
    let compact_backend = TestBackend::new(100, 32);
    let mut compact_terminal = Terminal::new(compact_backend).expect("compact terminal");
    compact_terminal
        .draw(|frame| render(frame, frame.area(), &app))
        .expect("render compact header");
    let compact: String =
        compact_terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    // Verification state rides on every tier: it is the difference between reading a change
    // and vouching for it, so the compact header carries it alongside the core stats.
    assert!(wide.contains("Checks ✓"));
    assert!(wide.contains("1 file ·"), "singular reads as one file: {wide}");
    assert!(compact.contains("1 file ·"));
    assert!(compact.contains("Checks ✓"));
}

#[test]
fn wide_header_names_failed_checks_without_calling_them_pending() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    app.guided_review.as_mut().expect("fixture review").checks.checks.push(ReviewCheck {
        name: "cargo test".into(),
        status: ReviewCheckStatus::Failed,
        summary: None,
    });
    let backend = TestBackend::new(160, 32);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render failed checks");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("Checks failed"));
    assert!(!text.contains("Checks pending"));
}

#[test]
fn wide_header_names_unavailable_checks_when_no_check_is_pending_or_failed() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    app.guided_review.as_mut().expect("fixture review").checks.checks.push(ReviewCheck {
        name: "remote gate".into(),
        status: ReviewCheckStatus::Unavailable,
        summary: Some("offline".into()),
    });
    let backend = TestBackend::new(160, 32);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render unavailable checks");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    // The only unavailable state today is a run that predates the final edit, so the label
    // says that rather than the generic word.
    assert!(text.contains("Verified before the last edit"), "{text}");
    assert!(!text.contains("Checks pending"));
}

#[test]
fn guide_summary_lists_referenced_non_text_files() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let review = app.guided_review.as_mut().expect("fixture review");
    let mut non_text = review.files[0].clone();
    non_text.id = FileId::from_path_lineage(None, Some(Path::new("assets/icon.bin")));
    non_text.new_path = Some("assets/icon.bin".into());
    non_text.hunks.clear();
    non_text.binary = true;
    non_text.non_text = Some(NonTextChange {
        kind: NonTextChangeKind::Binary,
        summary: "Binary files differ".into(),
    });
    review.chapters[0].items.push(ReviewItemRef::File(non_text.id.clone()));
    review.files.push(non_text);
    let backend = TestBackend::new(160, 32);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render guide summary");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("assets/icon.bin"));
}

#[test]
fn guide_summary_windows_long_file_lists_around_the_selected_file() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let review = app.guided_review.as_mut().expect("fixture review");
    let template = review.files[0].clone();
    let mut selected_id = None;
    // More files than a chapter lists at once, so the list has to window
    for number in 1..=9 {
        let path = format!("src/support_{number}.rs");
        let mut file = template.clone();
        file.id = FileId::from_path_lineage(None, Some(Path::new(&path)));
        // Hunks of their own, or the chapter's items resolve to whichever copy came last
        for (index, hunk) in file.hunks.iter_mut().enumerate() {
            hunk.id =
                luxe_core::guided_review::HunkId::from_content(&file.id, &index.to_string(), &path);
        }
        file.new_path = Some(path.into());
        review.chapters[0].items.push(ReviewItemRef::File(file.id.clone()));
        selected_id = Some(file.id.clone());
        review.files.push(file);
    }
    app.guided_review_workspace.position.file_id = selected_id;
    // Tall enough for the rail's metadata block plus a windowed file list; the exact elided
    // count depends on height, so assert that windowing happens and the selected file stays
    // visible rather than pinning a specific number.
    let backend = TestBackend::new(160, 20);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render guide summary");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    // The guide windows the list and always keeps the cursor's file in it.
    assert!(text.contains("support_9.rs"), "the selected file stays visible: {text}");
    assert!(!text.contains("support_1.rs"), "and the rest is windowed away: {text}");
    assert!(text.contains("above"), "saying what it left out: {text}");
}

#[test]
fn non_text_diff_focus_label_matches_the_active_pane() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let review = app.guided_review.as_mut().expect("fixture review");
    let mut non_text = review.files[0].clone();
    non_text.id = FileId::from_path_lineage(None, Some(Path::new("assets/icon.bin")));
    non_text.new_path = Some("assets/icon.bin".into());
    non_text.hunks.clear();
    non_text.binary = true;
    non_text.non_text = Some(NonTextChange {
        kind: NonTextChangeKind::Binary,
        summary: "Binary files differ".into(),
    });
    app.guided_review_workspace.position.file_id = Some(non_text.id.clone());
    review.files.push(non_text);
    let backend = TestBackend::new(160, 32);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render non-text diff");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("Binary change"));
    assert!(!text.contains("Diff · active"));
}

#[test]
fn the_rail_reports_verification_only_when_it_is_missing() {
    use ratatui::{backend::TestBackend, Terminal};

    let render_rail = |app: &mut crate::app::App| -> String {
        app.update(Msg::TerminalResized { width: 150, height: 48 });
        let mut terminal = Terminal::new(TestBackend::new(150, 48)).expect("test terminal");
        terminal.draw(|frame| crate::view::render(frame, app)).expect("render");
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect()
    };

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewDemo).expect("fixture");
    let verified = render_rail(&mut app);
    assert!(
        !verified.contains("Nothing ran after the last edit"),
        "a verified turn says nothing about it: {verified}"
    );

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewDemo).expect("fixture");
    if let Some(provenance) =
        app.guided_review.as_mut().and_then(|review| review.provenance.as_mut())
    {
        provenance.verified_after_last_edit = false;
    }
    let stale = render_rail(&mut app);
    assert!(
        stale.contains("Nothing ran after the last edit"),
        "the one verification fact a self-authored summary will not volunteer: {stale}"
    );
}

#[test]
fn ninety_column_review_uses_the_medium_context_layout() {
    use ratatui::{backend::TestBackend, Terminal};

    let app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("review fixture");
    let backend = TestBackend::new(90, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render medium boundary");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("01 / 01"));
    assert!(!text.contains("01/01 · Shared notification model · src/notification.rs"));
}
