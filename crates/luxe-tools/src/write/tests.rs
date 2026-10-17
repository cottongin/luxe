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

/// A written file that format-on-save then rewrote is shown as it now is.
#[tokio::test]
async fn a_reformatted_write_shows_the_file_as_it_is_now() {
    let dir = tempfile::tempdir().unwrap();
    let cx = crate::edit::tests::formatting_context(dir.path());
    let args = serde_json::json!({"path": "lib.rs", "content": "fn sub(a:i32) {}\n"});
    let result = WriteTool::new().run(args, &cx).await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.starts_with("Created 'lib.rs'"), "named from the project: {text}");
    assert!(text.contains("Format-on-save then reformatted the file"), "{text}");
    assert!(text.contains("+1 fn sub(a: i32) {}") && !text.contains("a:i32"), "{text}");
}

#[tokio::test]
async fn write_tool_basic() {
    let tmp_dir_guard = tempfile::tempdir().expect("temp dir");
    let tmp_dir = tmp_dir_guard.path().to_path_buf();

    let tool = WriteTool::new();
    let cx = test_context(&tmp_dir);

    let args = json!({
        "path": "test.txt",
        "content": "Hello, World!"
    });
    let result = tool.run(args, &cx).await;

    assert!(!result.is_error);
    if let ContentBlock::Text(text) = &result.content[0] {
        assert!(text.contains("Created"));
        assert!(text.contains("test.txt"));
        assert!(text.contains("+1 Hello, World!"), "shows the numbered additions: {text}");
    } else {
        panic!("expected Text block");
    }

    // Verify file was written.
    let content = std::fs::read_to_string(tmp_dir.join("test.txt")).unwrap();
    assert_eq!(content, "Hello, World!");
}

#[tokio::test]
async fn write_tool_creates_parent_dirs() {
    let tmp_dir_guard = tempfile::tempdir().expect("temp dir");
    let tmp_dir = tmp_dir_guard.path().to_path_buf();

    let tool = WriteTool::new();
    let cx = test_context(&tmp_dir);

    let args = json!({
        "path": "subdir1/subdir2/test.txt",
        "content": "Nested content"
    });
    let result = tool.run(args, &cx).await;

    assert!(!result.is_error);

    // Verify file was written.
    let content = std::fs::read_to_string(tmp_dir.join("subdir1/subdir2/test.txt")).unwrap();
    assert_eq!(content, "Nested content");
}

#[tokio::test]
async fn write_tool_missing_path() {
    let tmp_dir_guard = tempfile::tempdir().expect("temp dir");
    let tmp_dir = tmp_dir_guard.path().to_path_buf();

    let tool = WriteTool::new();
    let cx = test_context(&tmp_dir);

    let args = json!({"content": "Hello"});
    let result = tool.run(args, &cx).await;

    assert!(result.is_error);
}

#[tokio::test]
async fn write_tool_missing_content() {
    let tmp_dir_guard = tempfile::tempdir().expect("temp dir");
    let tmp_dir = tmp_dir_guard.path().to_path_buf();

    let tool = WriteTool::new();
    let cx = test_context(&tmp_dir);

    let args = json!({"path": "test.txt"});
    let result = tool.run(args, &cx).await;

    assert!(result.is_error);
}

#[tokio::test]
async fn write_tool_overwrite() {
    let tmp_dir_guard = tempfile::tempdir().expect("temp dir");
    let tmp_dir = tmp_dir_guard.path().to_path_buf();

    let tool = WriteTool::new();
    let cx = test_context(&tmp_dir);

    // Write initial content.
    let args1 = json!({
        "path": "test.txt",
        "content": "Initial content"
    });
    let result1 = tool.run(args1, &cx).await;
    assert!(!result1.is_error);

    // Overwrite with new content.
    let args2 = json!({
        "path": "test.txt",
        "content": "Overwritten content"
    });
    let result2 = tool.run(args2, &cx).await;
    assert!(!result2.is_error);

    // Verify file was overwritten.
    let content = std::fs::read_to_string(tmp_dir.join("test.txt")).unwrap();
    assert_eq!(content, "Overwritten content");
}
