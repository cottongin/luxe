//! Footer — what the key row says at each width, and where its keys can be clicked.

use super::*;

/// The footer trims its least important key the moment the row would overflow, and not a
/// cell before: all five fit in exactly their width, and a narrower row keeps what matters.
#[test]
fn the_footer_trims_by_priority_at_its_exact_width() {
    let app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let labels = |width: u16| -> Vec<String> {
        fit_footer_segments(ready_footer_segments(&app, width), width)
            .into_iter()
            .map(|segment| segment.label)
            .collect()
    };
    let all = ["space next", "c comment", "a ask agent", "alt+enter finish", "? help"];
    assert_eq!(footer_segments_width(&ready_footer_segments(&app, 200)), 64);
    assert_eq!(labels(64), all);
    assert_eq!(labels(63), ["space next", "c comment", "alt+enter finish", "? help"]);
    assert_eq!(labels(49), ["space next", "alt+enter finish", "? help"]);
    assert_eq!(labels(37), ["space next", "? help"], "a tie drops the later key");
    assert_eq!(labels(0), ["? help"], "and one key always stays");
}

/// The footer draws each key in the accent and its words in body text, and the key under the
/// pointer lights up whole.
#[test]
fn the_footer_styles_its_keys_and_lights_the_hovered_one() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    app.pointer.hover = Some(crate::mouse::MouseTarget::ReviewFooter(ReviewFooterAction::Finish));
    let segments = ready_footer_segments(&app, 200);
    let line = footer_spans(&app, &segments, 200);
    let styles = &app.theme_styles;
    let span =
        |text: &str| line.spans.iter().find(|span| span.content == text).map(|span| span.style);
    assert_eq!(span("alt+enter finish"), Some(styles.selected_bg), "hovered, whole");
    assert_eq!(span("c"), Some(styles.accent));
    assert_eq!(span(" comment"), Some(styles.text));
    assert_eq!(line.spans[0].content, "space", "no separator before the first key");
}

/// A footer that is not the ready one is styled the same way, and a sentence among its
/// segments stays a sentence.
#[test]
fn a_plain_footer_styles_keys_and_leaves_sentences() {
    let app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let styles = &app.theme_styles;
    let line = footer_hint_line("Refresh failed: no disk · r retry", styles);
    let spans: Vec<(&str, Style)> =
        line.spans.iter().map(|span| (span.content.as_ref(), span.style)).collect();
    assert_eq!(
        spans,
        [
            ("Refresh failed: no disk", styles.text),
            (" · ", styles.muted),
            ("r", styles.accent),
            (" retry", styles.text),
        ]
    );
    for key in ["space", "enter", "esc", "tab", "pgup", "pgdn", "alt+c", "enter/↓", "?", "g"] {
        assert!(is_key(key), "{key}");
    }
    for word in ["Refresh", "Search:", "type", "go"] {
        assert!(!is_key(word), "{word}");
    }
}

/// Each footer shortcut is clickable across exactly the cells it is drawn in.
#[test]
fn footer_clicks_land_on_the_key_drawn_there() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let rows = frame_rows(&mut app, 160, 40);
    let footer = rows.last().expect("footer row");
    let at = |label: &str| -> u16 {
        let byte = footer.find(label).unwrap_or_else(|| panic!("{label} in {footer:?}"));
        UnicodeWidthStr::width(&footer[..byte]) as u16
    };
    let hit = app.hit.borrow();
    let target = |x: u16| hit.at(x, 39).cloned();
    let finish = crate::mouse::MouseTarget::ReviewFooter(ReviewFooterAction::Finish);
    let next = crate::mouse::MouseTarget::ReviewFooter(ReviewFooterAction::HunkNext);
    let start = at("alt+enter finish");
    assert_eq!(target(start), Some(finish.clone()));
    assert_eq!(target(start + 15), Some(finish.clone()), "to its last cell");
    assert_ne!(target(start + 16), Some(finish.clone()), "and no further");
    assert_ne!(target(start - 2), Some(finish), "not on the separator");
    assert_eq!(target(at("space next")), Some(next));
}

/// The feedback window offers to send from inside itself, and only where sending does
/// anything: not on someone else's pull request, whose comments go to GitHub.
#[test]
fn the_feedback_window_offers_sending() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    let hint = |app: &mut App| frame_rows(app, 160, 40).join("\n");
    let own = hint(&mut app);
    assert!(own.contains("c reply · u send · esc close"), "{own}");
    app.guided_review.as_mut().expect("review").source.base =
        luxe_core::guided_review::ReviewBase::PullRequest {
            number: 42,
            repository: "acme/notify".into(),
            base_ref: "main".into(),
            head_ref: "topic".into(),
            merge_base: "4f1c2d9a7e6b5c4d3e2f1a0b9c8d7e6f5a4b3c2d".into(),
            head_oid: "a9d9d84ee3e848f0e1ea97eaef837126d3a7d983".into(),
        };
    let theirs = hint(&mut app);
    assert!(!theirs.contains("u send"), "{theirs}");
}

/// The footer names finishing, which is where committing and opening a pull request live, and
/// the feedback list once there is feedback to list.
#[test]
fn the_footer_names_finishing_and_the_feedback_list() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    // The diff pane's share of the bottom row: the rail's own foot shares the row now
    let footer = |app: &mut App| {
        let last = frame_rows(app, 160, 40).last().cloned().unwrap_or_default();
        last.rsplit('│').next().unwrap_or_default().trim().to_owned()
    };
    let bare = footer(&mut app);
    assert!(bare.contains("alt+enter finish") && !bare.contains("alt+c feedback"), "{bare}");
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    // The window takes the keyboard, and the footer with it
    assert!(footer(&mut app).is_empty(), "no footer under the window");
    app.guided_review_workspace.findings_visible = false;
    let with = footer(&mut app);
    assert!(with.contains("alt+c feedback") && with.contains("alt+enter finish"), "{with}");
    assert!(!with.contains("esc"), "Esc is the key nobody has to be told: {with}");
}

/// Read to the end, the guide says what finishing does next, named for what the dialog will
/// offer: a branch with no pull request can open one, someone else's pull request is posted.
#[test]
fn read_to_the_end_the_guide_says_what_finishing_offers() {
    let all_read = |scenario| {
        let mut app = seeded_app("nord", scenario).expect("fixture app");
        // A branch GitHub has no pull request for
        let mut snapshot = crate::fixture::dock_workspace_snapshot();
        snapshot.pull_request = luxe_core::workspace::PullRequestPresence::None;
        app.workspace = Some(snapshot);
        let review = app.guided_review.clone().expect("review");
        let workspace = &mut app.guided_review_workspace;
        (workspace.pull_request_opening, workspace.thread_answers) = (None, None);
        workspace.github_post = None;
        for chapter in &review.chapters {
            workspace.walked_chapters.insert(chapter.id.clone());
        }
        frame_rows(&mut app, 160, 44).join("\n")
    };
    let mut unread = seeded_app("nord", FixtureScenario::GuidedReviewOpenPullRequest).unwrap();
    assert!(!frame_rows(&mut unread, 160, 44).join("\n").contains("All read"));
    let opening = all_read(FixtureScenario::GuidedReviewOpenPullRequest);
    assert!(opening.contains("All read · alt+enter to open a pull request"), "{opening}");
    let theirs = all_read(FixtureScenario::GuidedReviewPullRequest);
    assert!(theirs.contains("All read · alt+enter to post your review"), "{theirs}");
    let own = all_read(FixtureScenario::GuidedReviewOwnPullRequest);
    assert!(own.contains("All read · alt+enter to answer on GitHub"), "{own}");
}

/// Space walks the review without finishing it; once it is on the last stop of the last
/// chapter, Space again is "what now" and opens the finish dialog — for a pull request, the
/// post preview it finishes with.
#[test]
fn space_past_the_last_stop_opens_the_finish_dialog() {
    let space = |app: &mut App| app.update(crate::app::Msg::GuidedReviewHunkNext);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let review = app.guided_review.clone().expect("review");
    let last = review.chapters.last().expect("chapters").id.clone();
    let mut steps = 0;
    while !(app.guided_review_workspace.position.chapter_id.as_ref() == Some(&last)
        && app.guided_review_workspace.at_scope_end(&review))
    {
        space(&mut app);
        assert!(app.guided_review_workspace.finish_review_choice.is_none(), "not yet");
        steps += 1;
        assert!(steps < 100, "walked off the review");
    }
    assert!(app.guided_review_workspace.is_chapter_walked(&last), "landing marks it read");
    assert!(app.guided_review_workspace.finish_review_choice.is_none());
    space(&mut app);
    assert!(app.guided_review_workspace.finish_review_choice.is_some(), "then finishes");

    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewPullRequest).expect("fixture app");
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    app.engine_tx = tx;
    for _ in 0..100 {
        space(&mut app);
        if app.guided_review_workspace.github_post.is_some() {
            break;
        }
    }
    assert!(app.guided_review_workspace.github_post.is_some(), "the post preview");
}

/// Walked to the last stop, the diff pane's end marker names the key that finishes.
#[test]
fn the_end_of_the_reading_names_the_finish_key() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let mut text = String::new();
    for _ in 0..40 {
        app.update(crate::app::Msg::GuidedReviewHunkNext);
        text = frame_rows(&mut app, 160, 44).join("\n");
        if text.contains("last chapter") {
            break;
        }
    }
    assert!(text.contains("— last chapter reviewed · space or alt+enter finishes —"), "{text}");
}

#[test]
fn low_height_review_keeps_header_diff_row_and_footer() {
    use ratatui::{backend::TestBackend, Terminal};

    let app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("review fixture");
    let backend = TestBackend::new(160, 7);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render short review");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("Refactor notification model"));
    assert!(text.contains("pub struct Notification"));
    assert!(text.contains("space next"));
    assert!(!text.contains("Review needs 40 × 5 cells"));
}

#[test]
fn base_reference_entry_footer_names_load_and_local_cancel() {
    use crate::guided_review::ReviewSourceReferenceKind;
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    app.guided_review_workspace.toggle_source_picker();
    app.guided_review_workspace.begin_source_reference_entry(ReviewSourceReferenceKind::Base);
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render source picker");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("enter load base"));
    assert!(text.contains("esc back"));
}
