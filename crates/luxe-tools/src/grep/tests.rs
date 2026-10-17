#![allow(clippy::unwrap_used)]

use super::*;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

fn test_context(tmp_dir: &std::path::Path) -> ToolContext {
    let (event_tx, _event_rx) = mpsc::channel(64);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    ToolContext {
        cwd: tmp_dir.to_path_buf(),
        session_id: "test-session".to_string(),
        cancelled: Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    }
}

/// A plain content search of `.`, for a test to vary one field of.
fn search<'a>(pattern: &'a str) -> Search<'a> {
    Search {
        pattern,
        path: Path::new("."),
        glob: None,
        before: 0,
        after: 0,
        all: false,
        invert: false,
        mode: OutputMode::Content,
        limit: GREP_MAX_LINES,
    }
}

/// A repo with one real match and a build directory full of look-alikes.
fn repo_with_build_output() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src").join("main.rs"), "let needle = 1;").unwrap();
    let build = dir.path().join("target").join("debug");
    std::fs::create_dir_all(&build).unwrap();
    for index in 0..30 {
        std::fs::write(build.join(format!("gen_{index}.rs")), "needle in build output").unwrap();
    }
    dir
}

/// The search honors ignore files and answers in ripgrep's row shape, which the old `grep`
/// fallback, a different program with different rules, did not.
#[test]
fn the_native_search_honors_ignores_and_reports_ripgrep_shaped_rows() {
    let dir = repo_with_build_output();
    let cwd = dir.path();

    let rows = native_search(cwd, &search("needle")).expect("a valid regex");
    assert_eq!(rows, vec!["src/main.rs:1:let needle = 1;"], "file:line:text, project-relative");

    let unfiltered =
        native_search(cwd, &Search { all: true, ..search("needle") }).expect("a valid regex");
    assert!(unfiltered.len() > 30, "all:true reaches build output: {}", unfiltered.len());

    let filtered =
        native_search(cwd, &Search { glob: Some("*.py"), all: true, ..search("needle") })
            .expect("valid");
    assert!(filtered.is_empty(), "the glob filter applies: {filtered:?}");

    assert!(native_search(cwd, &search("no_such_text")).expect("valid").is_empty());
}

/// Context lines use `-` where a match uses `:`, the way ripgrep prints them, so a reader (and
/// the model) can tell the answer from its surroundings.
#[test]
fn the_native_search_marks_context_lines_apart_from_matches() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.rs"), "one\ntwo\nNEEDLE\nfour\nfive\n").unwrap();

    let rows =
        native_search(dir.path(), &Search { before: 1, after: 1, ..search("NEEDLE") }).expect("ok");
    assert_eq!(rows, vec!["a.rs-2-two", "a.rs:3:NEEDLE", "a.rs-4-four"]);
}

/// The two sides are independent, because "show me what follows this signature" is the common
/// question and a symmetric window answers it with half the lines wasted.
#[test]
fn context_can_be_asked_for_on_one_side_only() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.rs"), "one\ntwo\nNEEDLE\nfour\nfive\n").unwrap();

    let after = native_search(dir.path(), &Search { after: 2, ..search("NEEDLE") }).expect("ok");
    assert_eq!(after, vec!["a.rs:3:NEEDLE", "a.rs-4-four", "a.rs-5-five"]);

    let before = native_search(dir.path(), &Search { before: 2, ..search("NEEDLE") }).expect("ok");
    assert_eq!(before, vec!["a.rs-1-one", "a.rs-2-two", "a.rs:3:NEEDLE"]);
}

/// A match that falls inside an earlier match's trailing context is still a match.
///
/// The break this catches: the search walked the matches and printed a window around each,
/// skipping lines an earlier window had already spent. The second `needle` was inside the first's
/// `after`, so it came back as a `-` context row and the answer silently lost one of its two hits.
#[test]
fn a_match_inside_an_earlier_match_s_context_is_still_a_match() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.rs"), "one\nneedle A\nfiller\nneedle B\nlast\n").unwrap();

    let rows = native_search(dir.path(), &Search { after: 2, ..search("needle") }).expect("ok");
    assert_eq!(
        rows,
        vec!["a.rs:2:needle A", "a.rs-3-filler", "a.rs:4:needle B", "a.rs-5-last"],
        "both hits are `:` rows, and no line is printed twice"
    );
}

/// The coarse modes are the token lever: the same question, answered in rows a model can act on
/// without reading the lines.
#[test]
fn the_native_search_answers_files_and_count_modes() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.rs"), "needle\nplain\nneedle\n").unwrap();
    std::fs::write(dir.path().join("b.rs"), "needle\n").unwrap();
    std::fs::write(dir.path().join("c.rs"), "nothing here\n").unwrap();

    let mut files =
        native_search(dir.path(), &Search { mode: OutputMode::Files, ..search("needle") })
            .expect("ok");
    files.sort();
    assert_eq!(files, vec!["a.rs", "b.rs"], "one path per matching file, and no c.rs");

    let mut counts =
        native_search(dir.path(), &Search { mode: OutputMode::Count, ..search("needle") })
            .expect("ok");
    counts.sort();
    assert_eq!(counts, vec!["a.rs:2", "b.rs:1"], "matching lines per file");
}

/// An inverted search reports the lines that did *not* match, which is a different question and
/// not one a pattern can always express.
#[test]
fn the_native_search_can_invert_the_match() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.rs"), "keep\ndrop\nkeep\n").unwrap();

    let rows = native_search(dir.path(), &Search { invert: true, ..search("keep") }).expect("ok");
    assert_eq!(rows, vec!["a.rs:2:drop"]);

    let counts = native_search(
        dir.path(),
        &Search { invert: true, mode: OutputMode::Count, ..search("keep") },
    )
    .expect("ok");
    assert_eq!(counts, vec!["a.rs:1"], "the count follows the inversion");
}

/// A binary file has no lines worth reporting, and dumping one into the context window is the
/// worst possible answer to a broad pattern.
#[test]
fn the_native_search_skips_binary_files() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.bin"), b"needle\x00\x01\x02needle").unwrap();
    std::fs::write(dir.path().join("a.txt"), "needle\n").unwrap();

    let rows = native_search(dir.path(), &search("needle")).expect("ok");
    assert_eq!(rows, vec!["a.txt:1:needle"]);
}

/// An empty result reads as "nothing matches"; a pattern that will not compile is a different
/// thing and has to say so, or the model retries the same broken regex.
#[test]
fn the_native_search_reports_a_broken_regex_rather_than_finding_nothing() {
    let dir = repo_with_build_output();
    let problem = native_search(dir.path(), &search("(unclosed")).expect_err("not a regex");
    assert!(problem.contains("not a valid regex"), "{problem}");
}

/// A search that reports build artifacts is worse than useless: the 200-line cap fills with
/// them and the real match never appears.
#[tokio::test]
async fn build_output_is_skipped_by_default() {
    let dir = repo_with_build_output();
    let tool = GrepTool::new();
    let result = tool.run(json!({"pattern": "needle"}), &test_context(dir.path())).await;

    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("main.rs"), "the real match is found: {text}");
    assert!(!text.contains("gen_"), "build output is not: {text}");
}

/// `path: ""` is what a strict-schema model sends for "the whole project".
#[tokio::test]
async fn an_empty_path_searches_the_project() {
    let dir = repo_with_build_output();
    let result = GrepTool::new()
        .run(json!({"pattern": "needle", "path": ""}), &test_context(dir.path()))
        .await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(!result.is_error && text.contains("main.rs"), "{text}");
}

#[tokio::test]
async fn ignored_files_are_searchable_on_request() {
    let dir = repo_with_build_output();
    let tool = GrepTool::new();
    let result =
        tool.run(json!({"pattern": "needle", "all": true}), &test_context(dir.path())).await;

    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("gen_"), "all:true reaches build output: {text}");
}

#[tokio::test]
async fn grep_tool_basic() {
    let tmp_dir_guard = tempfile::tempdir().expect("temp dir");
    let tmp_dir = tmp_dir_guard.path().to_path_buf();
    std::fs::write(tmp_dir.join("test.txt"), "line1\nline2\nline3 with pattern\nline4").unwrap();

    let tool = GrepTool::new();
    let cx = test_context(&tmp_dir);

    let args = json!({
        "pattern": "pattern",
        "path": "."
    });
    let result = tool.run(args, &cx).await;

    assert!(!result.is_error);
    if let ContentBlock::Text(text) = &result.content[0] {
        assert!(text.contains("pattern"));
    } else {
        panic!("expected Text block");
    }
}

/// A zero is the schema's default, not a request for nothing.
///
/// A model under a strict schema fills every property it was shown, so `limit: 0` arrives
/// unasked — and read literally it caps the answer at no rows at all, which looks exactly like
/// a search that found nothing.
#[tokio::test]
async fn a_zero_limit_is_an_absent_limit() {
    let dir = tempfile::TempDir::new().unwrap();
    let body = (1..=20).map(|n| format!("needle {n}\n")).collect::<String>();
    std::fs::write(dir.path().join("many.txt"), body).unwrap();

    let result = GrepTool::new()
        .run(json!({"pattern": "needle", "limit": 0}), &test_context(dir.path()))
        .await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert_eq!(text.lines().count(), 20, "every match, not none: {text}");
    assert!(!text.contains("not shown"), "and nothing was cut: {text}");
}

/// The hint is the whole point of the cheap modes: a capped content search has to name the one
/// that would have answered without the lines.
#[tokio::test]
async fn a_capped_content_search_points_at_the_cheaper_mode() {
    let dir = tempfile::TempDir::new().unwrap();
    let body = (1..=300).map(|n| format!("needle {n}\n")).collect::<String>();
    std::fs::write(dir.path().join("many.txt"), body).unwrap();

    let result = GrepTool::new().run(json!({"pattern": "needle"}), &test_context(dir.path())).await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("outputMode 'files'"), "names the cheaper answer: {text}");

    let files = GrepTool::new()
        .run(
            json!({"pattern": "needle", "outputMode": "files", "limit": 1}),
            &test_context(dir.path()),
        )
        .await;
    let ContentBlock::Text(text) = &files.content[0] else { panic!("expected text") };
    assert!(!text.contains("outputMode"), "the cheapest mode is not told to use itself: {text}");
}

/// `limit` may tighten the bound and never loosen it.
#[tokio::test]
async fn a_limit_can_only_tighten_the_cap() {
    let dir = tempfile::TempDir::new().unwrap();
    let body = (1..=300).map(|n| format!("needle {n}\n")).collect::<String>();
    std::fs::write(dir.path().join("many.txt"), body).unwrap();
    let cx = test_context(dir.path());

    let sampled = GrepTool::new().run(json!({"pattern": "needle", "limit": 5}), &cx).await;
    let ContentBlock::Text(text) = &sampled.content[0] else { panic!("expected text") };
    assert_eq!(text.lines().filter(|line| line.contains("many.txt")).count(), 5, "{text}");
    assert!(text.contains("295 more matching lines not shown (300 total)"), "{text}");

    let greedy = GrepTool::new().run(json!({"pattern": "needle", "limit": 5000}), &cx).await;
    let ContentBlock::Text(text) = &greedy.content[0] else { panic!("expected text") };
    assert_eq!(
        text.lines().filter(|line| line.contains("many.txt")).count(),
        GREP_MAX_LINES,
        "a limit past the ceiling is the ceiling: {text}"
    );
}

/// The count a capped result reports is the count that exists, not the count that was kept.
///
/// The break this catches: the search used to stop walking once it had one row past the caller's
/// limit, so a `limit: 5` over three hundred hits reported "(6 total)". A wrong total tells a
/// model its search was narrow when it was not.
#[test]
fn the_search_reports_every_match_it_could_have_shown() {
    let dir = tempfile::TempDir::new().unwrap();
    for file in 0..5 {
        let body = (1..=60).map(|n| format!("needle {n}\n")).collect::<String>();
        std::fs::write(dir.path().join(format!("f{file}.txt")), body).unwrap();
    }

    let rows = native_search(dir.path(), &Search { limit: 5, ..search("needle") }).expect("ok");
    assert_eq!(rows.len(), 300, "a tight limit bounds the answer, not the search");

    let capped = crate::output_limit::cap_lines_within(
        &rows.join("\n"),
        5,
        GREP_MAX_CHARS,
        OutputMode::Content.unit(),
        OutputMode::Content.hint(),
    );
    assert!(capped.contains("295 more matching lines not shown (300 total)"), "{capped}");
}

/// Grepping for the spot and then editing it is how models work, so the lines a search showed
/// count as a read of their file. A search that showed no lines of it does not.
#[tokio::test]
async fn the_lines_a_search_shows_count_as_read() {
    use luxe_core::read_tracker::{ReadStatus, ReadTracker};
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    for name in ["src/a.py", "src/b.py", "src/c.py"] {
        std::fs::write(dir.path().join(name), "def needle():\n    pass\n").unwrap();
    }
    std::fs::write(dir.path().join("src/b.py"), "nothing here\n").unwrap();
    let cx = test_context(dir.path());
    let status =
        |tracker: &ReadTracker, name: &str| tracker.status(&cx.session_id, &dir.path().join(name));

    let tracker = std::sync::Arc::new(ReadTracker::new());
    let tool = GrepTool::new().with_tracker(tracker.clone());
    tool.run(json!({"pattern": "needle", "path": "src"}), &cx).await;
    assert_eq!(status(&tracker, "src/a.py"), ReadStatus::Current);
    assert_eq!(status(&tracker, "src/b.py"), ReadStatus::Unseen, "no line of it was shown");

    let tracker = std::sync::Arc::new(ReadTracker::new());
    let tool = GrepTool::new().with_tracker(tracker.clone());
    tool.run(json!({"pattern": "needle", "path": "src/c.py"}), &cx).await;
    assert_eq!(status(&tracker, "src/c.py"), ReadStatus::Current, "one file prints no name");

    let tracker = std::sync::Arc::new(ReadTracker::new());
    let tool = GrepTool::new().with_tracker(tracker.clone());
    tool.run(json!({"pattern": "needle", "path": "src", "output_mode": "files"}), &cx).await;
    assert_eq!(status(&tracker, "src/a.py"), ReadStatus::Unseen, "a list of names is not lines");
}

/// The coarse modes are what the schema points a model at, so they have to work end to end —
/// through whichever half this machine runs.
#[tokio::test]
async fn the_tool_answers_files_and_count_modes() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), "needle\nplain\nneedle\n").unwrap();
    std::fs::write(dir.path().join("b.txt"), "nothing\n").unwrap();
    let cx = test_context(dir.path());

    let files =
        GrepTool::new().run(json!({"pattern": "needle", "output_mode": "files"}), &cx).await;
    let ContentBlock::Text(text) = &files.content[0] else { panic!("expected text") };
    assert_eq!(text.trim(), "a.txt", "just the path, and only the file that matches");

    let counts =
        GrepTool::new().run(json!({"pattern": "needle", "outputMode": "count"}), &cx).await;
    let ContentBlock::Text(text) = &counts.content[0] else { panic!("expected text") };
    assert_eq!(text.trim(), "a.txt:2", "matching lines, not matches");
}

/// The advertised spelling is camelCase, but a model that has met this argument elsewhere sends
/// the other one — and an ignored mode answers the expensive question instead of the cheap one,
/// silently.
#[tokio::test]
async fn the_retired_spelling_of_the_mode_is_still_accepted() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), "needle\n").unwrap();
    let cx = test_context(dir.path());

    for spelling in ["outputMode", "output_mode"] {
        let result =
            GrepTool::new().run(json!({"pattern": "needle", spelling: "files"}), &cx).await;
        let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
        assert_eq!(text.trim(), "a.txt", "{spelling} selected the files mode");
    }
}

/// The defect the character budget exists for: 200 rows is not 200 rows' worth of tokens when
/// the rows are wide, and the schema's promise of a bounded result has to hold for both.
#[tokio::test]
async fn a_few_very_wide_matches_are_bounded_by_characters() {
    let dir = tempfile::TempDir::new().unwrap();
    let wide = (1..=60).map(|n| format!("needle {n} {}\n", "x".repeat(2_000))).collect::<String>();
    std::fs::write(dir.path().join("generated.js"), wide).unwrap();

    let result = GrepTool::new().run(json!({"pattern": "needle"}), &test_context(dir.path())).await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };

    assert!(
        text.chars().count() < GREP_MAX_CHARS + 500,
        "60 rows are under the line cap but not under the budget: {} chars",
        text.chars().count()
    );
    assert!(text.contains("not shown (60 total)"), "and it says so: {}", &text[text.len() - 200..]);
}

/// A search with no `path` argument searches the project.
///
/// The break this catches: every other test here passes `"path": "."`, so the default — which is
/// what a model actually sends — went unexercised.
#[tokio::test]
async fn a_search_with_no_path_searches_the_project() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("wordfreq.py"), "def rank(counts):\n    pass\n").unwrap();

    let result =
        GrepTool::new().run(json!({ "pattern": "def rank" }), &test_context(dir.path())).await;

    assert!(!result.is_error, "{:?}", result.content);
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("wordfreq.py"), "the file the match is in: {text}");
    assert!(text.contains("def rank"), "the matching line: {text}");
}

#[tokio::test]
async fn grep_tool_no_matches() {
    let tmp_dir_guard = tempfile::tempdir().expect("temp dir");
    let tmp_dir = tmp_dir_guard.path().to_path_buf();
    std::fs::write(tmp_dir.join("test.txt"), "line1\nline2").unwrap();

    let tool = GrepTool::new();
    let cx = test_context(&tmp_dir);

    let args = json!({
        "pattern": "nonexistent",
        "path": "."
    });
    let result = tool.run(args, &cx).await;

    assert!(!result.is_error);
    if let ContentBlock::Text(text) = &result.content[0] {
        assert!(text.contains("No matches found"));
    } else {
        panic!("expected Text block");
    }
}

#[tokio::test]
async fn grep_tool_missing_pattern() {
    let tmp_dir_guard = tempfile::tempdir().expect("temp dir");
    let tmp_dir = tmp_dir_guard.path().to_path_buf();

    let tool = GrepTool::new();
    let cx = test_context(&tmp_dir);

    let args = json!({});
    let result = tool.run(args, &cx).await;

    assert!(result.is_error);
}
