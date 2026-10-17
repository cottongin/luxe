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

/// Several files in one call, each under its header and each recorded for the edit guards;
/// one that cannot be read says so without failing the rest.
#[tokio::test]
async fn several_files_read_in_one_call() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "alpha").unwrap();
    std::fs::write(dir.path().join("b.txt"), "beta").unwrap();
    let tracker = Arc::new(ReadTracker::new());
    let tool = ReadTool::new().with_tracker(tracker.clone());
    let cx = test_context(dir.path());

    let result = tool.run(json!({"paths": ["a.txt", "b.txt", "missing.txt"]}), &cx).await;
    assert!(!result.is_error, "two of three read");
    let ContentBlock::Text(text) = &result.content[0] else { panic!("text") };
    assert!(text.contains("── a.txt ──\n1: alpha"), "{text}");
    assert!(text.contains("── b.txt ──\n1: beta"), "{text}");
    assert!(text.contains("── missing.txt ──\nError reading file"), "{text}");
    for name in ["a.txt", "b.txt"] {
        assert_eq!(
            tracker.status("test-session", &dir.path().join(name)),
            luxe_core::read_tracker::ReadStatus::Current,
            "{name} counts as read"
        );
    }

    let none = tool.run(json!({"paths": ["missing.txt"]}), &cx).await;
    assert!(none.is_error, "nothing read is a failure");
    let both = tool.run(json!({"path": "a.txt", "paths": ["b.txt"]}), &cx).await;
    let ContentBlock::Text(text) = &both.content[0] else { panic!("text") };
    assert!(text.contains("── a.txt ──") && text.contains("── b.txt ──"), "{text}");
}

#[tokio::test]
async fn read_tool_basic() {
    let tmp_dir_guard = tempfile::tempdir().expect("temp dir");
    let tmp_dir = tmp_dir_guard.path().to_path_buf();
    std::fs::write(tmp_dir.join("test.txt"), "line1\nline2\nline3").unwrap();

    let tool = ReadTool::new();
    let cx = test_context(&tmp_dir);

    let args = json!({"path": "test.txt"});
    let result = tool.run(args, &cx).await;

    assert!(!result.is_error);
    if let ContentBlock::Text(text) = &result.content[0] {
        assert_eq!(text, "1: line1\n2: line2\n3: line3");
    } else {
        panic!("expected Text block");
    }
}

#[tokio::test]
async fn read_tool_caps_a_huge_file_without_an_explicit_limit() {
    let dir = tempfile::tempdir().expect("temp dir");
    let big = (1..=(READ_DEFAULT_LIMIT + 500)).map(|n| n.to_string()).collect::<Vec<_>>();
    std::fs::write(dir.path().join("big.txt"), big.join("\n")).unwrap();
    let result = ReadTool::new().run(json!({"path": "big.txt"}), &test_context(dir.path())).await;
    assert!(!result.is_error);
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected Text block") };
    assert!(text.starts_with("1: 1\n2: 2\n"), "{}", &text[..20]);
    assert!(
        text.ends_with(
            "-- 500 more of 2500 lines (a read without `limit` stops at 2000 lines); \
                 continue with offset 2001 --"
        ),
        "default cap fires and names the next offset: {}",
        &text[text.len().saturating_sub(120)..]
    );
    assert!(text.lines().any(|l| l == "2000: 2000"), "last kept line present");
    assert!(!text.lines().any(|l| l.starts_with("2001: ")), "line past the cap dropped");
}

#[tokio::test]
async fn read_tool_with_offset_limit() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("test.txt"), "line1\nline2\nline3\nline4\nline5").unwrap();
    // 1-based: offset 2 starts at line2, limit 2 reads line2+line3
    let args = json!({"path": "test.txt", "offset": 2, "limit": 2});
    let result = ReadTool::new().run(args, &test_context(dir.path())).await;
    assert!(!result.is_error);
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected Text block") };
    assert_eq!(text, "2: line2\n3: line3\n-- 2 more of 5 lines; continue with offset 4 --");
}

#[tokio::test]
async fn read_tool_missing_path() {
    let tmp_dir_guard = tempfile::tempdir().expect("temp dir");
    let tmp_dir = tmp_dir_guard.path().to_path_buf();

    let tool = ReadTool::new();
    let cx = test_context(&tmp_dir);

    let args = json!({});
    let result = tool.run(args, &cx).await;

    assert!(result.is_error);
}

#[tokio::test]
async fn read_tool_nonexistent_file() {
    let tmp_dir_guard = tempfile::tempdir().expect("temp dir");
    let tmp_dir = tmp_dir_guard.path().to_path_buf();

    let tool = ReadTool::new();
    let cx = test_context(&tmp_dir);

    let args = json!({"path": "nonexistent.txt"});
    let result = tool.run(args, &cx).await;

    assert!(result.is_error);
}

/// A minified bundle or a large JSON blob is *one line*, so the line cap never fires and a
/// single read returned half a megabyte straight into the context window.
#[tokio::test]
async fn a_minified_single_line_file_is_bounded() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("bundle.js"), "x".repeat(500_000)).expect("write");
    let tool = ReadTool::new();
    let result =
        tool.run(serde_json::json!({"path": "bundle.js"}), &test_context(dir.path())).await;

    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.chars().count() < READ_MAX_CHARS + 300, "bounded: {} chars", text.chars().count());
    assert!(text.contains("line 1 is 500000 characters"), "and says so");
    assert!(
        text.contains("grep"),
        "with a way to get what you need: {}",
        &text[text.len() - 160..]
    );
}

/// A file of ordinary lines that outruns the character cap stops at a whole line, says so, and
/// names the line to continue from, which is where the old note gave a wrong total and no offset.
#[tokio::test]
async fn the_character_cap_stops_at_a_line_and_says_where_to_continue() {
    let dir = tempfile::tempdir().expect("temp dir");
    let line = "y".repeat(99);
    let lines = vec![line.as_str(); 1000];
    std::fs::write(dir.path().join("wide.txt"), lines.join("\n")).expect("write");
    let result = ReadTool::new()
        .run(serde_json::json!({"path": "wide.txt"}), &test_context(dir.path()))
        .await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    // each row is `N: `, 99 characters and a newline
    let (mut shown, mut spent) = (0, 0);
    while spent + format!("{}: ", shown + 1).len() + 100 <= READ_MAX_CHARS {
        spent += format!("{}: ", shown + 1).len() + 100;
        shown += 1;
    }
    assert!(text.starts_with("1: yyy"), "{}", &text[..40]);
    assert!(
        text.ends_with(&format!(
            "-- {} more of 1000 lines (stopped at the {READ_MAX_CHARS}-character cap); \
                 continue with offset {} --",
            1000 - shown,
            shown + 1
        )),
        "{}",
        &text[text.len() - 120..]
    );
    assert!(
        text.lines().all(|row| row.starts_with("--") || row.ends_with(&"y".repeat(99))),
        "whole lines only"
    );
}

/// `offset` and `limit` apply to each of `paths`, rather than being dropped in silence.
#[tokio::test]
async fn a_range_applies_to_each_of_several_files() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("a.txt"), "a1\na2\na3\na4").expect("write");
    std::fs::write(dir.path().join("b.txt"), "b1\nb2").expect("write");
    let args = serde_json::json!({"paths": ["a.txt", "b.txt"], "limit": 1});
    let result = ReadTool::new().run(args, &test_context(dir.path())).await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(
        text.contains("── a.txt ──\n1: a1\n-- 3 more of 4 lines; continue with offset 2 --"),
        "{text}"
    );
    assert!(
        text.contains("── b.txt ──\n1: b1\n-- 1 more of 2 lines; continue with offset 2 --"),
        "{text}"
    );
}

/// A line that exactly fills the budget is shown whole, and the cap stops at the line after it.
#[tokio::test]
async fn a_line_exactly_the_size_of_the_cap_is_shown() {
    let dir = tempfile::tempdir().expect("temp dir");
    // `1: `, the line and its newline come to exactly the cap
    let exact = "z".repeat(READ_MAX_CHARS - 4);
    std::fs::write(dir.path().join("exact.txt"), format!("{exact}\nnext")).expect("write");
    let result = ReadTool::new()
        .run(serde_json::json!({"path": "exact.txt"}), &test_context(dir.path()))
        .await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(
        text.starts_with(&format!("1: {exact}\n-- 1 more of 2 lines")),
        "{}",
        &text[text.len() - 100..]
    );
}

/// An empty file is a fact, not a bad argument.
#[tokio::test]
async fn an_empty_file_is_not_an_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("empty.txt"), "").expect("write");
    let result = ReadTool::new()
        .run(serde_json::json!({"path": "empty.txt"}), &test_context(dir.path()))
        .await;

    assert!(!result.is_error, "an empty file reads fine");
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("empty"), "{text}");
    assert!(!text.contains("offset"), "and never blames an offset the caller never sent: {text}");
}

#[tokio::test]
async fn a_binary_file_says_it_is_binary() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("bin.dat"), [0u8, 159, 146, 150, 255]).expect("write");
    let result = ReadTool::new()
        .run(serde_json::json!({"path": "bin.dat"}), &test_context(dir.path()))
        .await;

    assert!(result.is_error);
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("binary file"), "{text}");
    assert!(!text.contains("valid UTF-8"), "not the stream-decoder's words: {text}");
}

#[tokio::test]
async fn an_offset_past_the_end_names_the_real_length() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("three.txt"), "a\nb\nc\n").expect("write");
    let result = ReadTool::new()
        .run(serde_json::json!({"path": "three.txt", "offset": 99}), &test_context(dir.path()))
        .await;

    assert!(result.is_error);
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("99") && text.contains("3 lines"), "{text}");
}
