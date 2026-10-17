//! Guide — the walkthrough pane, its head and foot, and its scrolling.

use super::*;

/// A pull request's review leads with where it stands and who opened it, then gives the
/// author's own account under a heading that says whose words they are. It says once that the
/// work was done elsewhere, instead of the "no record" row a working-tree review would get.
#[test]
fn a_pull_request_review_leads_with_the_pull_request() {
    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewPullRequest).expect("fixture app");
    let rows = frame_rows(&mut app, 160, 90);
    let text = rows.join("\n");
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));

    assert!(has("pull request #42"), "{text}");
    assert!(has("by copilot-swe-agent (bot)"), "{text}");
    assert!(has("1 failing") && has("11 checks") && has("changes requested"), "{text}");
    assert!(has("copilot/category-enum") && has("→ main"), "{text}");
    assert!(has("What the pull request says"), "{text}");
    assert!(has("What it closes") && has("#318 A typo in a webhook"), "{text}");
    assert!(!has("No record of the work behind this source"), "{text}");
}

/// A review with no chapters has nothing to have read, so it never claims all of it.
#[test]
fn a_review_with_no_chapters_is_not_all_read() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture");
    app.guided_review.as_mut().expect("review").chapters.clear();
    let rows = frame_rows(&mut app, 160, 44);
    assert!(!rows.iter().any(|row| row.contains("All read")), "{}", rows.join("\n"));
}

/// A rule sits under the guide's head and over its foot, in place of the blank rows that used
/// to space them, so the part that scrolls reads as its own zone. With no feedback there is no
/// foot, and no rule for it. The diff's keys sit under a rule of their own.
#[test]
fn the_guide_rules_off_its_head_and_its_foot() {
    let ruled = |row: &str| row.trim_start().starts_with("──────────");
    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewPullRequest).expect("fixture app");
    let rows = frame_rows(&mut app, 160, 46);
    let text = rows.join("\n");
    let head_end = rows.iter().position(|row| row.contains("changes requested")).expect("head");
    assert!(ruled(&rows[head_end + 1]), "under the head: {text}");
    assert!(!rows.iter().any(|row| row.contains("FEEDBACK")), "nothing to list: {text}");
    assert_eq!(rows.iter().filter(|row| ruled(row)).count(), 1, "{text}");
    let keys = rows.iter().position(|row| row.contains("space next")).expect("the keys");
    let (_, over_keys) = rows[keys - 1].split_once('│').expect("the diff's column");
    assert!(over_keys.starts_with(" ─────────"), "a cell clear of the divider: {text}");

    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewPullRequestPost).expect("fixture app");
    app.guided_review_workspace.github_post = None;
    let rows = frame_rows(&mut app, 160, 46);
    let text = rows.join("\n");
    let foot = rows.iter().position(|row| row.contains("FEEDBACK")).expect("foot");
    assert!(ruled(&rows[foot - 1]), "over the foot: {text}");
}

/// The guide can be read without walking the review, and lets go when the cursor moves.
///
/// The wheel over this pane used to move the file selection, so reading the section you were
/// not standing in was impossible: every notch dragged the review's position along with the
/// view. Scrolling is now scrolling — and it releases on the next cursor move, because an
/// offset that outlives the reason it was pinned is a pane stuck where nobody is looking.
#[test]
fn the_guide_scrolls_for_reading_and_hands_itself_back_when_the_cursor_moves() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewLarge).expect("fixture app");
    app.update(crate::app::Msg::TerminalResized { width: 180, height: 44 });
    let mut terminal = Terminal::new(TestBackend::new(180, 44)).expect("test terminal");
    let mut draw = |app: &App| {
        terminal.draw(|frame| render(frame, frame.area(), app)).expect("render");
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect::<String>()
    };

    let followed = draw(&app);
    let before = app.guided_review_workspace.position.clone();
    for _ in 0..6 {
        app.update(crate::app::Msg::GuidedReviewScrollGuide { up: false });
    }
    let scrolled = draw(&app);
    assert_ne!(followed, scrolled, "the wheel moved the prose");
    assert_eq!(
        app.guided_review_workspace.position, before,
        "and moved nothing else: reading is not navigating"
    );

    // Moving the cursor is the reader saying they are done reading elsewhere.
    app.update(crate::app::Msg::GuidedReviewHunkNext);
    let _ = draw(&app);
    assert!(
        app.guided_review_workspace.guide_scroll.get().is_none(),
        "the guide follows the cursor again"
    );
}

/// Stepping with `Space` keeps the hunk being read on screen in the guide.
///
/// The pane used to follow the chapter's title, so on a short terminal a chapter with a few
/// paragraphs of prose walked its own files off the bottom while the reader stepped through
/// them, and finding out which file was open meant reaching for the wheel.
#[test]
fn the_guide_keeps_the_hunk_being_read_on_screen() {
    use ratatui::{backend::TestBackend, Terminal};

    let (width, height) = (180u16, 26u16);
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewLarge).expect("fixture app");
    app.update(crate::app::Msg::TerminalResized { width, height });
    let guide = crate::workspace_layout::guide_pane_width(width);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    let mut checked = 0;
    for _ in 0..80 {
        let before = app.guided_review_workspace.position.clone();
        app.update(crate::app::Msg::GuidedReviewHunkNext);
        if app.guided_review_workspace.position == before {
            app.update(crate::app::Msg::GuidedReviewChapterNext);
        }
        if app.guided_review_workspace.position.hunk_id.is_none() {
            continue;
        }
        terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render");
        let buffer = terminal.backend().buffer();
        let marked = (0..height).any(|y| (0..guide).any(|x| buffer[(x, y)].symbol() == "\u{2503}"));
        assert!(
            marked,
            "the hunk being read is off the guide at {:?}",
            app.guided_review_workspace.position
        );
        checked += 1;
    }
    assert!(checked > 10, "the walk visited hunks: {checked}");
}

/// The overview is the whole change on one page: the prose, then every chapter and file.
/// The guide and its evidence are on screen together, which is what deleted the mode switch:
/// there is nothing left to switch between.
#[test]
fn the_guide_and_its_evidence_share_the_screen() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewDemo).expect("review fixture");
    app.update(Msg::TerminalResized { width: 180, height: 44 });
    let mut terminal = Terminal::new(TestBackend::new(180, 44)).expect("test terminal");
    terminal.draw(|frame| crate::view::render(frame, &mut app)).expect("render");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    // The guide: the change's intent, its chapters, and the files under them.
    assert!(text.contains("free-form string"), "the intent: {text}");
    assert!(text.contains("Make an invalid category unrepresentable"), "the chapter: {text}");
    assert!(text.contains("src/notification.rs"), "its files: {text}");
    // And the evidence, at the same time.
    assert!(text.contains("@@ "), "the diff is beside it, not behind a mode: {text}");
}

/// Four labeled sections used to sit between the intent and the contents, and every one of
/// them repeated something already on the rail or already printed on the item it described.
#[test]
fn the_guide_does_not_repeat_what_its_own_head_says() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewBrief).expect("review fixture");
    app.update(Msg::TerminalResized { width: 180, height: 48 });
    let mut terminal = Terminal::new(TestBackend::new(180, 48)).expect("test terminal");
    terminal.draw(|frame| crate::view::render(frame, &mut app)).expect("render");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    for gone in [
        "What backs it up",
        "What needs you",
        "What the guide might not tell you",
        "Scale",
        "Enter opens the selected row",
        "Why this change",
        "What it does",
        "The walkthrough",
    ] {
        assert!(!text.contains(gone), "{gone:?} is still on the page: {text}");
    }
    // The step is the only "where am I" on the page: an item count beside it counted a
    // different noun, never moved as the reader walked, and disagreed with the chapter tally
    // down the left.
    assert!(!text.contains("to review"), "no second progress counter: {text}");
    assert!(!text.contains("Chapters "), "the chapter tally is beside the chapters: {text}");
    assert!(text.contains("Step 1 of 2"), "and the step is capitalized: {text}");
}
