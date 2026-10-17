//! Review guide — the guide pane's rows, the noise fixtures and the review source picker.

use super::*;

/// Guide file-row clicks (plan B6): after a render publishes each file row's wrapped-Y region,
/// clicking a guide file row selects that file.
#[test]
fn clicking_a_guide_file_row_selects_that_file() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (width, height) = (140, 40);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).unwrap();
    app.update(Msg::TerminalResized { width, height });
    let expected = app.guided_review.as_ref().unwrap().files[0].id.clone();

    let buffer = render_app(&mut app, &backend);
    // Search only the left rail (its file row) — the content pane's sticky breadcrumb also names
    // the file, so scanning the full row width would match that instead.
    let row = (0..height)
        .find(|&y| row_text(&buffer, y, 40).contains("src/notification.rs"))
        .expect("the guide file row renders");
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 10, // inside the guide rail
        row,
        modifiers: KeyModifiers::NONE,
    };
    match route_mouse(&mut app, &click) {
        RouteAction::Msg(message) => assert!(
            matches!(*message, Msg::GuidedReviewSelectFile(ref id) if id == &expected),
            "clicking the file row selects that file"
        ),
        other => panic!("expected a select-file message from the click, got {other:?}"),
    }
}

/// Clicking a chapter row in the guide points the review at that chapter.
#[test]
fn clicking_a_guide_chapter_row_opens_it() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (width, height) = (160, 44);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewDemo).unwrap();
    app.update(Msg::TerminalResized { width, height });
    render_app(&mut app, &backend);

    let (rect, chapter_id) = app
        .hit
        .borrow()
        .clicks()
        .find_map(|(rect, target)| match target {
            luxe_tui::mouse::MouseTarget::ReviewGuideChapter(id) => Some((rect, id.clone())),
            _ => None,
        })
        .expect("the guide publishes its chapter rows");
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + 1,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    };
    match route_mouse(&mut app, &click) {
        RouteAction::Msg(message) => match *message {
            Msg::GuidedReviewOpenChapter(id) => assert_eq!(id, chapter_id),
            other => panic!("expected the chapter under the pointer, got {other:?}"),
        },
        other => panic!("expected a chapter to open, got {other:?}"),
    }
}

/// Clicking a guide file row opens exactly that file.
#[test]
fn clicking_a_guide_file_row_reads_what_it_points_at() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (width, height) = (140, 40);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).unwrap();
    app.update(Msg::TerminalResized { width, height });

    let buffer = render_app(&mut app, &backend);
    let guide = luxe_tui::workspace_layout::guide_pane_width(width);
    let row = (0..height)
        .find(|&y| {
            let text: String =
                (0..guide).map(|x| buffer[(x, y)].symbol()).collect::<Vec<_>>().join("");
            text.contains("M src/notification.rs")
        })
        .expect("the file row renders under its chapter");
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 10,
        row,
        modifiers: KeyModifiers::NONE,
    };
    match route_mouse(&mut app, &click) {
        RouteAction::Msg(message) => {
            assert!(matches!(*message, Msg::GuidedReviewSelectFile(_)));
            app.update(*message);
        }
        other => panic!("expected the file row to open, got {other:?}"),
    }
    // The detail pane follows the guide's selection with no second step.
    let after = buffer_to_string(&render_app(&mut app, &backend));
    assert!(after.contains("src/notification.rs"), "the file is on screen: {after}");
    assert!(after.contains("@@"), "and its diff beside the guide: {after}");
}

/// The guide is the contents page: every chapter, and the files under it, with the evidence that
/// accounts for them. Hunk rows appear under the file being read and nowhere else.
#[test]
fn the_guide_lists_every_chapter_and_file_with_its_evidence() {
    let (width, height) = (140, 30);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewNoise).unwrap();
    app.update(Msg::TerminalResized { width, height });

    let guide = luxe_tui::workspace_layout::guide_pane_width(width);
    let buffer = render_app(&mut app, &backend);
    let content: String =
        (0..height).map(|y| row_text(&buffer, y, guide)).collect::<Vec<_>>().join("\n");

    assert!(content.contains("01 / 01"), "the chapter, numbered: {content}");
    assert!(content.contains("Shared notification model"), "and titled: {content}");
    assert!(content.contains("M src/notification.rs"), "its file, with status: {content}");
    assert!(content.contains("+3 \u{2212}2"), "with real counts, not a \u{b1}: {content}");
}

#[test]
fn guided_review_help_overlay_is_local_and_names_review_controls() {
    let (width, height) = (100, 24);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).unwrap();
    app.update(Msg::TerminalResized { width, height });
    app.update(Msg::GuidedReviewToggleHelp);

    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(content.contains("Help · Guided Review"), "missing help title: {content}");
    // Grouped, complete help (plan A4): every verb group is present, including the feedback and
    // finish keys the old help omitted.
    for expected in [
        "Move",
        "Read",
        "Feedback",
        "Session",
        "next stop; past the last, finish",
        "open collapsed churn",
        "ask; the agent answers",
        "comment, for the agent to act on",
        "all comments: send, reply, verify",
        "resolve or verify",
        "show or hide the guide",
        "ctrl+r",
        "alt+enter",
        "back to chat",
        "? close",
    ] {
        assert!(content.contains(expected), "help missing {expected:?}: {content}");
    }
}

#[test]
fn guided_review_noise_fixture_collapses_and_expands_formatting_hunks() {
    let (width, height) = (140, 24);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewNoise).unwrap();
    app.update(Msg::TerminalResized { width, height });

    let collapsed = buffer_to_string(&render_app(&mut app, &backend));
    // The folded run says what it is on its own row in the diff, so the guide does not repeat it
    assert!(
        collapsed.contains("1 change · 2 lines ·") && collapsed.contains("enter opens"),
        "missing the fold row: {collapsed}"
    );
    assert!(!collapsed.contains("mechanical change collapsed"), "a banner too: {collapsed}");
    assert!(!collapsed.contains("f opens them"), "and its hint: {collapsed}");
    assert!(!collapsed.contains("@@ -1,1 +1,1 @@ imports"), "folded hunk rendered: {collapsed}");

    app.update(Msg::GuidedReviewToggleNoiseDetails);
    let details = buffer_to_string(&render_app(&mut app, &backend));
    // The explanation names *why this hunk* was folded, not a generic blurb about the classifier.
    assert!(
        details.contains("1 whitespace-only, tokens identical"),
        "missing per-class fold reason: {details}"
    );
    assert!(
        details.contains("Semantic and comment-only changes stay"),
        "missing the visibility promise: {details}"
    );
    assert!(details.contains("1 mechanical change collapsed · 2 lines"), "{details}");

    app.update(Msg::GuidedReviewToggleNoise);
    let expanded = buffer_to_string(&render_app(&mut app, &backend));
    assert!(expanded.contains("@@ -1,1 +1,1 @@ imports"), "expanded hunk missing: {expanded}");
    assert!(!expanded.contains("collapsed · 2 lines"), "stale fold summary: {expanded}");
}

#[test]
fn guided_review_noise_summary_survives_medium_and_narrow_layouts() {
    for (width, expected) in
        [(100, "1 mechanical change collapsed · 2 lines · f opens them"), (80, "1 collapsed")]
    {
        let height = 24;
        let backend = test_backend(width, height);
        let mut app = seeded_app("nord", FixtureScenario::GuidedReviewNoise).unwrap();
        app.update(Msg::TerminalResized { width, height });

        let content = buffer_to_string(&render_app(&mut app, &backend));
        assert!(content.contains(expected), "missing {expected:?}: {content}");
    }
}

#[test]
fn guided_review_medium_context_explains_noise_classification() {
    let (width, height) = (100, 24);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewNoise).unwrap();
    app.update(Msg::TerminalResized { width, height });

    let collapsed = buffer_to_string(&render_app(&mut app, &backend));
    assert!(
        collapsed.contains("1 mechanical change collapsed · 2 lines · f opens them"),
        "missing folded summary: {collapsed}"
    );

    app.update(Msg::GuidedReviewToggleNoiseDetails);
    let expanded = buffer_to_string(&render_app(&mut app, &backend));
    assert!(
        expanded.contains("1/2 folded: format/generated/lockfile/mechanical;")
            && expanded.contains("semantic/comments kept · d close"),
        "missing medium classification detail: {expanded}"
    );
}

#[test]
fn guided_review_narrow_context_discloses_noise_classification() {
    let (width, height) = (80, 24);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewNoise).unwrap();
    app.update(Msg::TerminalResized { width, height });

    app.update(Msg::GuidedReviewToggleNoiseDetails);
    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(
        content.contains("1 fmt/gen/lock/mech"),
        "missing narrow classification disclosure: {content}"
    );
}

#[test]
fn guided_review_narrow_fixture_keeps_chapter_and_file_orientation() {
    let (width, height) = (80, 24);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).unwrap();
    app.update(Msg::TerminalResized { width, height });

    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(
        content.contains("01/01 · Shared notification model · src/notification.rs"),
        "missing narrow chapter/file header: {content}"
    );
    assert!(
        content.contains("@@ -4,3 +4,4 @@ pub struct Notification"),
        "missing narrow diff: {content}"
    );
}

#[test]
fn guided_review_tiny_fixture_explains_the_minimum_size() {
    let (width, height) = (30, 5);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).unwrap();
    app.update(Msg::TerminalResized { width, height });

    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(
        content.contains("Review needs 40 × 5 cells"),
        "missing minimum-size message: {content}"
    );
    assert!(content.contains("esc back to chat"), "missing return guidance: {content}");
    assert!(!content.contains("@@ -4,3"), "tiny terminal rendered a partial diff: {content}");
}

#[test]
fn guided_review_source_picker_enter_dispatches_the_selected_source() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.current_view = ViewMode::GuidedReview;
    app.update(Msg::GuidedReviewToggleSourcePicker);

    assert!(matches!(
        claim(&mut app, &key(KeyCode::Enter)),
        Some(Msg::GuidedReviewSelectSourcePicker)
    ));
}

#[test]
fn guided_review_source_reference_routes_plain_text_before_review_keys() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.current_view = ViewMode::GuidedReview;
    app.update(Msg::GuidedReviewToggleSourcePicker);
    app.update(Msg::GuidedReviewSourcePickerNext);
    app.update(Msg::GuidedReviewSelectSourcePicker);

    assert!(matches!(
        claim(&mut app, &key(KeyCode::Char('f'))),
        Some(Msg::GuidedReviewSourceReferenceInsert('f'))
    ));
    assert!(matches!(
        claim(&mut app, &key(KeyCode::Backspace)),
        Some(Msg::GuidedReviewSourceReferenceBackspace)
    ));
    assert!(matches!(
        claim(&mut app, &key(KeyCode::Esc)),
        Some(Msg::GuidedReviewCancelSourceReference)
    ));
}

#[test]
fn guided_review_source_picker_names_targets_and_unavailable_last_turn() {
    let (width, height) = (100, 24);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).unwrap();
    app.update(Msg::TerminalResized { width, height });
    app.update(Msg::GuidedReviewToggleSourcePicker);

    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(content.contains("Change review source"), "missing source picker: {content}");
    assert!(content.contains("› Working Tree"), "missing selected working tree: {content}");
    assert!(content.contains("Last Turn"), "missing last-turn source: {content}");
    assert!(
        content.contains("unavailable — no completed turn"),
        "missing disabled reason: {content}"
    );
    assert!(content.contains("Base"), "missing base source: {content}");
    assert!(content.contains("Commit"), "missing commit source: {content}");
    assert!(
        content.contains("enter load · ↑/↓ choose · esc close"),
        "missing source controls: {content}"
    );

    app.update(Msg::GuidedReviewSourcePickerNext);
    app.update(Msg::GuidedReviewSelectSourcePicker);
    let base_content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(
        base_content.contains("Base reference: _"),
        "missing Base reference entry: {base_content}"
    );

    app.update(Msg::GuidedReviewSourceReferenceInsert('m'));
    app.update(Msg::GuidedReviewSourceReferenceInsert('a'));
    app.update(Msg::GuidedReviewSourceReferenceInsert('i'));
    app.update(Msg::GuidedReviewSourceReferenceBackspace);
    let typed_content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(
        typed_content.contains("Base reference: ma"),
        "missing typed Base reference: {typed_content}"
    );

    app.update(Msg::GuidedReviewCancelSourceReference);
    app.update(Msg::GuidedReviewSourcePickerNext);
    app.update(Msg::GuidedReviewSelectSourcePicker);
    let commit_content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(
        commit_content.contains("Commit object ID: _"),
        "missing Commit object ID entry: {commit_content}"
    );
    assert!(
        commit_content.contains("enter load commit · esc back"),
        "missing Commit entry controls: {commit_content}"
    );
}

#[test]
fn guided_review_wide_fixture_renders_chapter_context_beside_the_diff() {
    let (width, height) = (140, 24);
    let backend = test_backend(width, height);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).unwrap();
    app.update(Msg::TerminalResized { width, height });

    let content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(content.contains("01 / 01"), "missing chapter progress: {content}");
    assert!(content.contains("Shared notification model"), "missing chapter title: {content}");
    assert!(content.contains("M src/notification.rs"), "missing the file row: {content}");
    assert!(
        content.contains("pub struct Notification   hunk 1 of 2"),
        "the detail pane names what it is showing: {content}"
    );
    assert!(content.contains("@@ -4,3 +4,4 @@"), "missing adjacent diff: {content}");
    // Both halves at once: the story and the code it describes, with no mode between them.
    assert!(content.contains("Shared notification model") && content.contains("@@"));

    app.update(Msg::GuidedReviewHunkNext);
    let next_hunk_content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(
        next_hunk_content.contains("› @@ -18,1 +18,2 @@ impl Notification"),
        "next hunk did not become visibly selected: {next_hunk_content}"
    );
    assert!(
        !next_hunk_content.contains("› @@ -4,3 +4,4 @@ pub struct Notification"),
        "previous hunk retained selected marker: {next_hunk_content}"
    );

    app.update(Msg::GuidedReviewHunkPrevious);
    let previous_hunk_content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(
        previous_hunk_content.contains("› @@ -4,3 +4,4 @@ pub struct Notification"),
        "previous hunk did not restore the original marker: {previous_hunk_content}"
    );
    assert!(
        !previous_hunk_content.contains("› @@ -18,1 +18,2 @@ impl Notification"),
        "next hunk retained selected marker after moving back: {previous_hunk_content}"
    );

    // Progress is the step you are on — never a per-hunk tally, and never a second counter beside
    // it counting something else.
    let workload_content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(workload_content.contains("Step "), "the rail says where you are: {workload_content}");
    assert!(!workload_content.contains("to review"), "and only once: {workload_content}");
    assert!(!workload_content.contains("Reviewed "), "no completion counter: {workload_content}");
    assert!(workload_content.contains("space next"), "stepping is offered: {workload_content}");

    // Walking to the end of a chapter records it as read — against the chapter itself, in the
    // guide, rather than as a second tally in the foot.
    app.update(Msg::GuidedReviewHunkNext);
    app.update(Msg::GuidedReviewHunkNext);
    let walked_content = buffer_to_string(&render_app(&mut app, &backend));
    assert!(walked_content.contains("reviewed"), "the chapter is marked: {walked_content}");
    assert!(!walked_content.contains("Chapters 1/1"), "and not counted twice: {walked_content}");
}
