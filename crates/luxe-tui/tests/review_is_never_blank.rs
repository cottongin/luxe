//! The review always shows something.
//!
//! Every state in here rendered an *empty* content pane before: a breadcrumb naming a hunk over
//! nothing at all. Two causes, both regression-guarded here — the renderer and navigation disagreed
//! about what "folded" meant, and evidence-based accounting hid code outright.

use luxe_core::event::Event;
use luxe_core::guided_review::{
    ChapterId, ChapterKind, CheckSummary, FileId, GuidedReview, HunkId, LineRange, NoiseKind,
    ReviewBase, ReviewChapter, ReviewContext, ReviewFile, ReviewFileStatus, ReviewHunk, ReviewId,
    ReviewItemRef, ReviewLine, ReviewLineKind, ReviewLoadRequestId, ReviewRevisionId, ReviewSource,
    ReviewStats, ReviewTarget, TurnProvenance,
};
use luxe_tui::app::{App, Msg, ViewMode};
use luxe_tui::fixture::{seeded_app, FixtureScenario};
use luxe_tui::view::render;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::path::Path;

const WIDTH: u16 = 170;
const HEIGHT: u16 = 40;

fn text_file(path: &str, noise: NoiseKind, heading: &str) -> ReviewFile {
    let id = FileId::from_path_lineage(None, Some(Path::new(path)));
    let hunk_id = HunkId::from_content(&id, heading, heading);
    ReviewFile {
        id: id.clone(),
        old_path: None,
        new_path: Some(path.into()),
        status: ReviewFileStatus::Modified,
        language: Some("rust".into()),
        additions: Some(1),
        deletions: Some(0),
        hunks: vec![ReviewHunk {
            id: hunk_id,
            old_range: LineRange::new(1, 2),
            new_range: LineRange::new(1, 3),
            heading: Some(heading.into()),
            symbol: None,
            lines: vec![
                ReviewLine {
                    old_line: Some(1),
                    new_line: Some(1),
                    kind: ReviewLineKind::Context,
                    text: format!("// {heading}"),
                },
                ReviewLine {
                    old_line: None,
                    new_line: Some(2),
                    kind: ReviewLineKind::Addition,
                    text: format!("let {heading}_answer = 42;"),
                },
            ],
            noise,
        }],
        binary: false,
        generated: false,
        non_text: None,
    }
}

/// A review whose first file is accounted for by *evidence* (nobody attributed it to the agent) and
/// whose last file is nothing but mechanical churn — the two shapes that used to render nothing.
fn review_of(files: Vec<ReviewFile>) -> GuidedReview {
    let items: Vec<ReviewItemRef> = files
        .iter()
        .flat_map(|file| file.hunks.iter().map(|hunk| ReviewItemRef::Hunk(hunk.id.clone())))
        .collect();
    let revision_id = ReviewRevisionId::from_canonical_bytes(b"never blank");
    GuidedReview {
        conversation: None,
        id: ReviewId::new(),
        revision_id: revision_id.clone(),
        target: ReviewTarget::WorkingTree,
        source: ReviewSource {
            base: ReviewBase::EmptyTree,
            content_digest: revision_id,
            includes_staged: false,
            includes_unstaged: false,
            included_untracked_files: 0,
            captured_at_ms: 0,
            uncovered_paths: Vec::new(),
        },
        context: ReviewContext {
            title: "Never blank".into(),
            goal: None,
            phase_index: None,
            phase_title: None,
            planning_excerpt: None,
            acceptance_excerpt: None,
            source_intent: None,
            source_commits: 0,
            pull_request: None,
        },
        stats: ReviewStats {
            files: files.len() as u64,
            hunks: items.len() as u64,
            additions: Some(files.len() as u64),
            deletions: Some(0),
            binary_files: 0,
            non_text_files: 0,
            excluded_files: 0,
        },
        files,
        chapters: vec![ReviewChapter {
            id: ChapterId::from_items("core", &[]),
            title: "Everything".into(),
            rationale: "One chapter over the whole change.".into(),
            impact: None,
            kind: ChapterKind::Core,
            items,
            notes: Vec::new(),
        }],
        checks: CheckSummary::default(),
        // The agent wrote exactly one of these files; the rest still need review.
        provenance: Some(TurnProvenance {
            written_paths: ["src/agent.rs".to_owned()].into_iter().collect(),
            ..Default::default()
        }),
        insight: None,
        seen: None,
        brief: None,
        generated_at_ms: 0,
    }
}

fn opened(review: GuidedReview) -> App {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).unwrap();
    app.current_view = ViewMode::GuidedReview;
    let request_id = ReviewLoadRequestId::new();
    app.active_guided_review_request = Some(request_id.clone());
    app.update(Msg::Engine(Event::GuidedReviewLoaded { request_id, review: Box::new(review) }));
    app.update(Msg::TerminalResized { width: WIDTH, height: HEIGHT });
    app
}

/// Everything the content pane drew, to the right of the rail.
/// The whole frame, rail included — for facts the review states beside the document rather than
/// inside it.
fn frame(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
    terminal.draw(|frame| render(frame, app)).unwrap();
    terminal.backend().buffer().content.iter().map(ratatui::buffer::Cell::symbol).collect()
}

fn content(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
    terminal.draw(|frame| render(frame, app)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let rail = luxe_tui::workspace_layout::LEFT_RAIL_WIDTH;
    let mut out = String::new();
    for y in 0..HEIGHT {
        for x in rail..WIDTH {
            out.push_str(buffer.cell((x, y)).unwrap().symbol());
        }
        out.push('\n');
    }
    out
}

/// A hunk nothing but *evidence* accounts for is code. It renders.
#[test]
fn evidence_based_accounting_never_hides_a_line_of_code() {
    let review = review_of(vec![
        text_file("src/vendored.rs", NoiseKind::Semantic, "vendored"),
        text_file("src/agent.rs", NoiseKind::Semantic, "agent"),
    ]);
    let mut app = opened(review);

    let pane = content(&mut app);

    assert!(
        pane.contains("let vendored_answer = 42;"),
        "the review opened on a file it accounts for and drew nothing: {pane}"
    );
    assert!(pane.contains("@@ -1,2 +1,3 @@ vendored"), "its hunk header is there too: {pane}");
}

/// A file of nothing but mechanical churn is a collapsed row that says so, never an empty pane.
#[test]
fn a_fully_collapsed_file_renders_a_row_that_says_what_it_stands_for() {
    let review = review_of(vec![
        text_file("Cargo.lock", NoiseKind::Lockfile, "dependencies"),
        text_file("src/agent.rs", NoiseKind::Semantic, "agent"),
    ]);
    let mut app = opened(review.clone());
    let lockfile = review.files[0].id.clone();
    app.update(Msg::GuidedReviewSelectFile(lockfile.clone()));

    let collapsed = content(&mut app);

    assert!(
        collapsed.contains("1 change · 2 lines · lockfile"),
        "the collapsed run names what it stands for: {collapsed}"
    );
    assert!(
        !collapsed.contains("let dependencies_answer = 42;"),
        "and it is still collapsed: {collapsed}"
    );

    // Opening it is one keystroke, and it opens only this file.
    app.update(Msg::GuidedReviewToggleFileFold(lockfile));
    let open = content(&mut app);
    assert!(open.contains("let dependencies_answer = 42;"), "opening the run reads it: {open}");
}

/// The walkthrough visits a file made entirely of churn instead of stepping silently past it.
#[test]
fn the_walkthrough_stops_on_a_collapsed_file_rather_than_skipping_it() {
    let review = review_of(vec![
        text_file("src/agent.rs", NoiseKind::Semantic, "agent"),
        text_file("Cargo.lock", NoiseKind::Lockfile, "dependencies"),
    ]);
    let mut app = opened(review);

    app.update(Msg::GuidedReviewHunkNext);
    let pane = content(&mut app);

    assert!(pane.contains("Cargo.lock"), "stepping forward reaches the collapsed file: {pane}");
    assert!(pane.contains("1 change · 2 lines · lockfile"), "and it shows what is there: {pane}");
}

/// Whatever the cursor is on, the pane has content on it.
#[test]
fn no_reachable_cursor_position_renders_an_empty_pane() {
    let review = review_of(vec![
        text_file("Cargo.lock", NoiseKind::Lockfile, "dependencies"),
        text_file("src/vendored.rs", NoiseKind::Semantic, "vendored"),
        text_file("src/agent.rs", NoiseKind::Semantic, "agent"),
    ]);
    let mut app = opened(review);

    for step in 0..12 {
        let pane = content(&mut app);
        // The first three rows are the narration band and its rule; the diff starts under them.
        let body: String = pane.lines().skip(4).collect::<Vec<_>>().join("\n");
        assert!(!body.trim().is_empty(), "step {step} left the diff pane empty:\n{pane}");
        app.update(Msg::GuidedReviewLineNext);
    }
}

/// Prints the collapsed-run shape so it can be reviewed as text, without a screenshot.
#[test]
#[ignore = "shape snapshot, run explicitly"]
fn collapsed_run_shape_snapshot() {
    let review = review_of(vec![
        text_file("src/agent.rs", NoiseKind::Semantic, "agent"),
        text_file("Cargo.lock", NoiseKind::Lockfile, "dependencies"),
    ]);
    let mut app = opened(review.clone());
    app.update(Msg::GuidedReviewToggleSplit);
    app.update(Msg::GuidedReviewSelectFile(review.files[1].id.clone()));
    println!("{}", content(&mut app));
}

/// Prints the map so its shape can be reviewed as text.
#[test]
#[ignore = "shape snapshot, run explicitly"]
fn overview_shape_snapshot() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewLarge).unwrap();
    app.update(Msg::TerminalResized { width: WIDTH, height: HEIGHT });
    println!("{}", content(&mut app));
}

/// A binary in the change used to make the whole review's line totals unknown.
#[test]
fn one_uncountable_file_does_not_erase_the_review_s_line_totals() {
    use luxe_core::guided_review::{NonTextChange, NonTextChangeKind};
    let mut binary = text_file("logo.png", NoiseKind::Semantic, "binary");
    binary.hunks.clear();
    binary.additions = None;
    binary.deletions = None;
    binary.binary = true;
    binary.non_text =
        Some(NonTextChange { kind: NonTextChangeKind::Binary, summary: "Binary change".into() });
    let files = vec![text_file("src/agent.rs", NoiseKind::Semantic, "agent"), binary];

    let stats = ReviewStats::from_files(&files);

    assert_eq!(stats.additions, Some(1), "the text file's lines are still countable");
    assert_eq!(stats.deletions, Some(0));
    assert_eq!(stats.non_text_files, 1, "and the binary is reported as what it is");
}

/// A new file has no old side; two columns for it means half the pane is an empty tinted block.
#[test]
fn a_one_sided_file_reads_unified_even_when_the_session_prefers_split() {
    let mut added = text_file("src/added.rs", NoiseKind::Semantic, "added");
    added.status = ReviewFileStatus::Added;
    added.hunks[0].lines.retain(|line| line.kind != ReviewLineKind::Context);
    let modified = text_file("src/agent.rs", NoiseKind::Semantic, "agent");
    let review = review_of(vec![added, modified]);
    let mut app = opened(review.clone());
    app.update(Msg::GuidedReviewToggleSplit);

    // The modified file has both sides, so it keeps the two columns and their divider.
    app.update(Msg::GuidedReviewSelectFile(review.files[1].id.clone()));
    assert!(content(&mut app).contains('│'), "a two-sided file still renders side by side");

    app.update(Msg::GuidedReviewSelectFile(review.files[0].id.clone()));
    let one_sided = content(&mut app);

    assert!(
        one_sided.contains("let added_answer = 42;"),
        "the added line is on screen: {one_sided}"
    );
    let widest = one_sided
        .lines()
        .filter(|line| line.contains("let added_answer"))
        .map(|line| line.trim_end().chars().count())
        .max()
        .unwrap_or_default();
    assert!(widest < 60, "and it is not crammed into half a pane: {one_sided}");
}

/// A review opens at the start of the walkthrough, not at whatever file sorts first.
#[test]
fn a_review_opens_on_the_first_chapter() {
    let mut review = review_of(vec![
        text_file("AGENTS.md", NoiseKind::Semantic, "docs"),
        text_file("src/agent.rs", NoiseKind::Semantic, "agent"),
    ]);
    // Two chapters, and the file that sorts first belongs to the last one.
    let docs = ReviewItemRef::Hunk(review.files[0].hunks[0].id.clone());
    let core = ReviewItemRef::Hunk(review.files[1].hunks[0].id.clone());
    review.chapters = vec![
        ReviewChapter {
            id: ChapterId::from_items("core", &[]),
            title: "The feature".into(),
            rationale: "What was asked for.".into(),
            impact: None,
            kind: ChapterKind::Core,
            items: vec![core],
            notes: Vec::new(),
        },
        ReviewChapter {
            id: ChapterId::from_items("generated", &[]),
            title: "The rest".into(),
            rationale: "Everything else.".into(),
            impact: None,
            kind: ChapterKind::Generated,
            items: vec![docs],
            notes: Vec::new(),
        },
    ];
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).unwrap();
    app.current_view = ViewMode::GuidedReview;
    let request_id = ReviewLoadRequestId::new();
    app.active_guided_review_request = Some(request_id.clone());
    app.update(Msg::Engine(Event::GuidedReviewLoaded { request_id, review: Box::new(review) }));
    app.update(Msg::TerminalResized { width: WIDTH, height: HEIGHT });

    let rendered = frame(&mut app);
    assert!(rendered.contains("01 / 02"), "the walkthrough starts at its start: {rendered}");
    assert!(rendered.contains("The feature"), "on the chapter the guide put first: {rendered}");
    assert!(rendered.contains("src/agent.rs"), "and on that chapter's file: {rendered}");
}

/// What a review leaves out, it says out loud.
#[test]
fn excluded_files_are_counted_not_silently_dropped() {
    let mut review = review_of(vec![text_file("src/agent.rs", NoiseKind::Semantic, "agent")]);
    review.stats.excluded_files = 3;
    let mut app = opened(review);

    let rendered = frame(&mut app);

    assert!(
        rendered.contains("3 files excluded by review.exclude"),
        "the review admits it is not the whole diff: {rendered}"
    );
}

/// What a preview's surface draws at `width` × `height`, as text.
fn drawn(scenario: FixtureScenario, width: u16, height: u16) -> String {
    let mut app = seeded_app("nord", scenario).expect("fixture");
    app.update(Msg::TerminalResized { width, height });
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| render(frame, &mut app)).expect("drawn without a panic");
    terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect()
}

/// Every review preview survives a short or narrow screen, opened there or resized into it: where
/// it cannot fit it says so, and from there up it draws inside the screen with its way out.
///
/// The break this catches: the previews sized themselves with `clamp(minimum, height - 2)`, which
/// panics, even in a release build, the moment the screen is shorter than the minimum plus two.
#[test]
fn review_previews_survive_a_short_terminal() {
    let previews = [
        (FixtureScenario::GuidedReviewPullRequestPost, 10),
        (FixtureScenario::GuidedReviewFixPullRequest, 9),
        (FixtureScenario::GuidedReviewOpenStack, 10),
        (FixtureScenario::GuidedReviewOwnPullRequest, 12),
    ];
    for (scenario, threshold) in previews {
        let tiny = drawn(scenario, 40, 5);
        assert!(tiny.contains("Make the window bigger"), "{scenario:?} at 40×5: {tiny}");
        for height in [threshold - 1, threshold, 40] {
            for width in [40, 49, 120] {
                let shown = drawn(scenario, width, height);
                // drawn, not the notice, whose own text also names esc
                let drawn = !shown.contains("Make the window bigger") && shown.contains("esc");
                assert!(drawn, "{scenario:?} at {width}×{height}: {shown}");
            }
        }
        // opened on a normal screen, then shrunk under it
        let mut app = seeded_app("nord", scenario).expect("fixture");
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
        terminal.draw(|frame| render(frame, &mut app)).expect("drawn");
        for height in [threshold - 1, 5] {
            terminal.backend_mut().resize(40, height);
            app.update(Msg::TerminalResized { width: 40, height });
            terminal.autoresize().expect("resized");
            terminal.draw(|frame| render(frame, &mut app)).expect("redrawn without a panic");
            let shown: String =
                terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();
            let notice = shown.contains("Make the window bigger");
            assert_eq!(notice, height == 5, "{scenario:?} shrunk to {height}: {shown}");
        }
    }
    // the one preview that always fitted still offers its way out at the smallest size
    let smallest = drawn(FixtureScenario::GuidedReviewOpenPullRequest, 40, 5);
    assert!(smallest.contains("esc") && !smallest.contains("Make the window bigger"), "{smallest}");
}
