//! Review context — the backdrop, the rail and the guide's head.

use super::*;

/// A modal overlay dims the review behind it, but a blanket `set_style(dim)` patched every cell's
/// foreground — and nearly every bundled theme maps `selectedBg` to the same color as `dim`, so the
/// selected file row and diff line rendered as unreadable solid blocks. Highlighted cells must step
/// up to `muted` instead.
#[test]
fn modal_backdrop_keeps_highlighted_review_rows_readable() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).unwrap();
    app.update(Msg::TerminalResized { width, height });

    let selected_bg = app.theme_styles.selected_bg.bg.expect("selection background");
    let dim = app.theme_styles.dim.fg.expect("dim foreground");
    let muted = app.theme_styles.muted.fg.expect("muted foreground");
    assert_eq!(dim, selected_bg, "nord maps selectedBg onto dim — the case this guards");

    let before = render_app(&mut app, &backend);
    let highlighted: Vec<(u16, u16)> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .filter(|&(x, y)| before[(x, y)].bg == selected_bg)
        .collect();
    assert!(!highlighted.is_empty(), "the fixture renders a selected row");

    app.update(Msg::GuidedReviewToggleHelp);
    let after = render_app(&mut app, &backend);
    for (x, y) in highlighted {
        // Overlay cells legitimately repaint; only the backdrop behind it is under test.
        if after[(x, y)].bg != selected_bg {
            continue;
        }
        assert_ne!(
            after[(x, y)].fg,
            selected_bg,
            "cell ({x},{y}) became invisible dim-on-dim behind the overlay"
        );
        assert_eq!(after[(x, y)].fg, muted, "highlighted backdrop cells step up to muted");
    }
}

/// The review carries no wait, because it does not open until there is nothing left to wait for.
///
/// Every heading, sentence and per-file note here arrives with the guide pass, which takes tens of
/// seconds on a local model. This surface used to open on a structural placeholder and rewrite
/// itself under the reader — first silently, then behind a spinner that only advanced if something
/// happened to tick. Both were answers to the wrong question: the reader should not be on this
/// screen yet. The waiting is announced in the tray above the composer, where they are.
#[test]
fn the_review_surface_never_shows_a_wait_and_the_tray_does() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).unwrap();
    app.update(Msg::TerminalResized { width, height });

    let shown = buffer_to_string(&render_app(&mut app, &backend));
    assert!(!shown.contains("writing the guide"), "no wait on the review: {shown}");

    // And in the conversation, a review being prepared says so where it can be read.
    let mut chat = seeded_app("nord", FixtureScenario::ChatIdle).unwrap();
    chat.update(Msg::TerminalResized { width, height });
    chat.review.queued = Some(luxe_tui::app::QueuedReview {
        target: luxe_core::guided_review::ReviewTarget::WorkingTree,
        state: luxe_tui::app::QueuedReviewState::Capturing,
    });
    let waiting = buffer_to_string(&render_app(&mut chat, &backend));
    assert!(waiting.contains("capturing"), "the tray carries the wait: {waiting}");

    chat.review.queued.as_mut().expect("queued").state = luxe_tui::app::QueuedReviewState::Ready;
    let ready = buffer_to_string(&render_app(&mut chat, &backend));
    assert!(ready.contains("review ready"), "and then offers: {ready}");
}

/// A review opens on deterministic chapters and upgrades in place, so the rail must say which it
/// is showing — a structural placeholder read as the agent's explanation is worse than no prose.
#[test]
fn the_rail_names_how_the_visible_chapters_were_produced() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).unwrap();
    app.update(Msg::TerminalResized { width, height });

    let review = app.guided_review.clone().expect("fixture review");
    app.update(Msg::Engine(luxe_core::event::Event::GuidedReviewGuideUpdated {
        request_id: luxe_core::guided_review::ReviewLoadRequestId::new(),
        review_id: review.id.clone(),
        revision_id: review.revision_id.clone(),
        chapters: review.chapters.clone(),
        provenance: luxe_core::guided_review::GuideProvenance::Model { batches: 1 },
        brief: None,
    }));
    let model_written = buffer_to_string(&render_app(&mut app, &backend));
    assert!(!model_written.contains("structural guide"), "the model wrote these chapters");

    app.update(Msg::Engine(luxe_core::event::Event::GuidedReviewGuideUpdated {
        request_id: luxe_core::guided_review::ReviewLoadRequestId::new(),
        review_id: review.id.clone(),
        revision_id: review.revision_id,
        chapters: review.chapters,
        provenance: luxe_core::guided_review::GuideProvenance::Structural { reason: None },
        brief: None,
    }));
    let structural = buffer_to_string(&render_app(&mut app, &backend));
    assert!(structural.contains("structural guide"), "fallback says so: {structural}");
}

/// A Guide computed for a revision the reviewer has moved past must never replace what is on
/// screen — the prose would describe a different diff.
#[test]
fn a_guide_for_another_revision_is_discarded() {
    let (width, height) = (160, 40);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).unwrap();
    app.update(Msg::TerminalResized { width, height });
    let review = app.guided_review.clone().expect("fixture review");
    let original = review.chapters[0].title.clone();

    let mut replacement = review.chapters.clone();
    replacement[0].title = "Chapters from a stale revision".into();
    app.update(Msg::Engine(luxe_core::event::Event::GuidedReviewGuideUpdated {
        request_id: luxe_core::guided_review::ReviewLoadRequestId::new(),
        review_id: review.id,
        revision_id: luxe_core::guided_review::ReviewRevisionId::from_canonical_bytes(b"other"),
        chapters: replacement,
        provenance: luxe_core::guided_review::GuideProvenance::Model { batches: 1 },
        brief: None,
    }));

    let after = app.guided_review.as_ref().expect("review");
    assert_eq!(after.chapters[0].title, original);
    // The prose on screen is still the review's own, not the one written for a diff the
    // reader is no longer looking at.
    let rendered = buffer_to_string(&render_app(&mut app, &backend));
    assert!(rendered.contains(&original), "{rendered}");
    assert!(!rendered.contains("Chapters from a stale revision"), "{rendered}");
}

/// The guide is the landing surface: intent, what the change does, and the contents — read before
/// the diff, so attention goes to judgement rather than reconstruction.
#[test]
fn the_guide_leads_with_intent_and_the_contents() {
    let (width, height) = (160, 44);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewBrief).unwrap();
    app.update(Msg::TerminalResized { width, height });

    let content = buffer_to_string(&render_app(&mut app, &backend));

    // The summary is written for a reviewer, not replayed from the conversation they just left.
    assert!(
        content.contains("unrepresentable"),
        "the model's account of what the change does: {content}"
    );
    assert!(content.contains("Shared notification model"), "and the contents: {content}");
    // Headings are gone with the sections they labeled: the intent is the first thing in the pane,
    // so nothing has to announce it.
    for gone in ["Why this change", "What it does", "The walkthrough"] {
        assert!(!content.contains(gone), "{gone:?} is a heading over nothing now: {content}");
    }
    // The shell history is not a verdict: a grep that matched nothing is not a failed check.
    assert!(!content.contains("How it was verified"), "the synthesized list is gone: {content}");
    assert!(!content.contains("Checks failed"), "and so is the verdict it fed: {content}");
    // Coverage gaps go to the agent and to the file being read, never to a ranked list: a missing
    // test is work to hand off, not a call for the reviewer's judgement.
    assert!(
        !content.contains("has no covering test"),
        "coverage gaps are not ranked call-outs: {content}"
    );
    // One word for the unit of reading. The guide counts "Chapters", the footer moves by
    // "chapter", the diff ends "end of chapter".
    assert!(!content.contains("section"), "a chapter is never called a section: {content}");
    assert!(!content.contains("walked"), "progress is stated as reviewed: {content}");
}

/// A review is captured from the project's worktree, so files changed anywhere else are absent
/// from it and no amount of reading the diff will reveal them. The rail has to say so.
#[test]
fn the_review_says_when_the_session_worked_outside_the_project() {
    let (width, height) = (160, 44);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewBrief).unwrap();
    app.update(Msg::TerminalResized { width, height });

    let mut workspace = empty_workspace("luxe");
    workspace.external = luxe_core::ExternalChangesSummary {
        roots: vec![luxe_core::ExternalRootSummary {
            root: "/home/dev/work/sdk".into(),
            display_path: "~/work/sdk".into(),
            git_branch: Some("main".into()),
            git: true,
            files: Vec::new(),
            total_files: 2,
            truncated: false,
        }],
        total_files: 2,
        additions: Some(9),
        deletions: Some(1),
        truncated: false,
    };
    app.workspace = Some(workspace);

    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(
        content.contains("2 files changed elsewhere"),
        "the review names the work it cannot cover: {content}"
    );

    // And says nothing at all when there is nothing to say, which is what makes the line above
    // worth reading when it appears.
    app.workspace.as_mut().expect("seeded above").external =
        luxe_core::ExternalChangesSummary::default();
    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(!content.contains("changed elsewhere"), "nothing outside, nothing claimed: {content}");
}

/// Without a model-written brief the guide still says what it knows — and says whose words those
/// are, so a raw transcript is never mistaken for a summary.
#[test]
fn a_guide_with_no_model_summary_falls_back_to_the_record_and_labels_it() {
    let (width, height) = (160, 44);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewBrief).unwrap();
    app.update(Msg::TerminalResized { width, height });
    let written = buffer_to_string(&render_app(&mut app, &backend));
    assert!(
        !written.contains("no summary was written"),
        "a real summary is not labeled as a stand-in: {written}"
    );

    app.guided_review.as_mut().expect("a review").brief = None;
    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(
        content.contains("no summary was written"),
        "labeled, so the record does not pass as a summary: {content}"
    );
}

/// Side-by-side is a view of the unified row space, not a second one: it shows the whole file, keeps
/// the columns on fixed screen positions, and pairs each removal with its replacement.
#[test]
fn side_by_side_shows_the_file_with_aligned_columns_and_word_emphasis() {
    let (width, height) = (180, 40);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewWide).unwrap();
    app.update(Msg::TerminalResized { width, height });
    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);

    // The old renderer showed exactly one hunk; the fixture file has two.
    assert!(content.contains("@@ -4,3 +4,4 @@"), "first hunk: {content}");
    assert!(content.contains("@@ -18,1 +18,2 @@"), "second hunk in the same view: {content}");

    // The removal and its replacement share a row: old text left, new text right.
    let row = (0..height)
        .find(|&y| {
            let text = row_text(&buffer, y, width);
            text.contains("pub category: String,") && text.contains("NotificationCategory")
        })
        .expect("the replaced line renders as one paired row");

    // The column rule sits at the same screen column on every rendered row, which is what the old
    // renderer got wrong (an unpadded context cell moved it).
    let divider = (0..width)
        .find(|&x| buffer[(x, row)].symbol() == "│")
        .expect("a column rule on the paired row");
    let rows_with_content: Vec<u16> =
        (0..height).filter(|&y| row_text(&buffer, y, width).contains("pub ")).collect();
    assert!(rows_with_content.len() >= 3, "several source rows render");
    for y in rows_with_content {
        assert_eq!(
            buffer[(divider, y)].symbol(),
            "│",
            "the rule holds its column on row {y}: {content}"
        );
    }

    // Word-level emphasis on both sides: pairing is exact here, so the changed span is too — bold in
    // both columns, and never struck through.
    let marked_with = |needle: &str, modifier: ratatui::style::Modifier| {
        let (y, column) = (0..height)
            .find_map(|y| {
                let text = row_text(&buffer, y, width);
                text.find(needle).map(|byte| (y, text[..byte].chars().count() as u16))
            })
            .unwrap_or_else(|| panic!("{needle} renders somewhere"));
        buffer[(column, y)].modifier.contains(modifier)
    };
    assert!(
        marked_with("String,", ratatui::style::Modifier::BOLD),
        "the removed type is bold: {content}"
    );
    assert!(
        !marked_with("String,", ratatui::style::Modifier::CROSSED_OUT),
        "and readable: {content}"
    );
    assert!(
        marked_with("NotificationCategory", ratatui::style::Modifier::BOLD),
        "the added type is bold: {content}"
    );
}

/// Soft-wrap works in side-by-side too: a long line stays fully visible, the two columns stay
/// vertically aligned, and one logical row keeps one click target across every visual line it spans.
#[test]
fn side_by_side_soft_wraps_long_lines_without_losing_column_alignment() {
    let (width, height) = (200, 40);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewWide).unwrap();
    // A replaced line far longer than either column can show.
    {
        let file = &mut app.guided_review.as_mut().unwrap().files[0];
        let long_old = format!("    pub category: String, // {}", "old ".repeat(40));
        let long_new = format!("    pub category: NotificationCategory, // {}", "new ".repeat(40));
        for line in &mut file.hunks[0].lines {
            match line.kind {
                ReviewLineKind::Deletion => line.text = long_old.clone(),
                ReviewLineKind::Addition => line.text = long_new.clone(),
                _ => {}
            }
        }
    }
    app.update(Msg::TerminalResized { width, height });
    app.update(Msg::DispatchUiAction(luxe_core::keymap::UiAction::ReviewToggleWordWrap));

    let buffer = render_app(&mut app, &backend);
    let content = buffer_to_string(&buffer);

    // The tail of both sides is on screen rather than clipped off the right edge.
    assert!(content.contains("new new"), "the added line wrapped into view: {content}");
    assert!(content.contains("old old"), "the removed line wrapped into view: {content}");
    // Continuation rows are marked, as they are unified.
    assert!(content.contains('↳'), "wrapped rows carry a continuation glyph: {content}");

    // The rule holds one screen column across every wrapped row. Scanned inside the content pane
    // only: the rail divider and the mode tabs draw their own vertical rules.
    let pane = luxe_tui::workspace_layout::LEFT_RAIL_WIDTH
        + luxe_tui::workspace_layout::CONTEXT_DOCK_SEPARATOR_WIDTH;
    let columns: std::collections::HashSet<u16> = (0..height)
        .filter_map(|y| (pane..width).find(|&x| buffer[(x, y)].symbol() == "│"))
        .collect();
    assert_eq!(columns.len(), 1, "the rule never wanders: {columns:?}");
}
