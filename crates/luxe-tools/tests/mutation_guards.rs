//! Read-before-edit + staleness + blind-write guards, exercised with a shared `ReadTracker`.

#![allow(clippy::unwrap_used)]

use luxe_core::read_tracker::ReadTracker;
use luxe_core::{ContentBlock, Tool, ToolContext, ToolResultData};
use luxe_tools::edit::EditTool;
use luxe_tools::read::ReadTool;
use luxe_tools::write::WriteTool;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

fn ctx(cwd: &std::path::Path) -> ToolContext {
    let (event_tx, _rx) = mpsc::channel(16);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    ToolContext {
        cwd: cwd.to_path_buf(),
        session_id: "session".to_string(),
        cancelled: Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    }
}

fn text(result: &ToolResultData) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn edit_requires_a_prior_read_then_allows_follow_up_edits() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("f.txt"), "hello world").unwrap();

    let tracker = Arc::new(ReadTracker::new());
    let read = ReadTool::new().with_tracker(tracker.clone());
    let edit = EditTool::new().with_tracker(tracker.clone());
    let cx = ctx(tmp.path());

    // Editing an unread file is rejected (read-before-edit).
    let r = edit
        .run(json!({ "path": "f.txt", "edits": [{ "oldText": "world", "newText": "there" }] }), &cx)
        .await;
    assert!(r.is_error, "unread edit must be rejected: {}", text(&r));
    assert!(text(&r).contains("read-before-edit"), "message: {}", text(&r));

    // After a read, the edit applies.
    read.run(json!({ "path": "f.txt" }), &cx).await;
    let r = edit
        .run(json!({ "path": "f.txt", "edits": [{ "oldText": "world", "newText": "there" }] }), &cx)
        .await;
    assert!(!r.is_error, "read+edit should succeed: {}", text(&r));
    assert_eq!(std::fs::read_to_string(tmp.path().join("f.txt")).unwrap(), "hello there");

    // A follow-up edit works without re-reading: the tool refreshed the fingerprint on write.
    let r = edit
        .run(
            json!({ "path": "f.txt", "edits": [{ "oldText": "there", "newText": "everyone" }] }),
            &cx,
        )
        .await;
    assert!(!r.is_error, "follow-up edit should succeed: {}", text(&r));
    assert_eq!(std::fs::read_to_string(tmp.path().join("f.txt")).unwrap(), "hello everyone");
}

#[tokio::test]
async fn edit_rejects_a_file_changed_on_disk_since_read() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("f.txt"), "one").unwrap();

    let tracker = Arc::new(ReadTracker::new());
    let read = ReadTool::new().with_tracker(tracker.clone());
    let edit = EditTool::new().with_tracker(tracker.clone());
    let cx = ctx(tmp.path());

    read.run(json!({ "path": "f.txt" }), &cx).await;

    // Something else changes the file (different size ⇒ different fingerprint).
    std::fs::write(tmp.path().join("f.txt"), "one-two-three").unwrap();

    let r = edit
        .run(json!({ "path": "f.txt", "edits": [{ "oldText": "one", "newText": "1" }] }), &cx)
        .await;
    assert!(r.is_error, "stale edit must be rejected: {}", text(&r));
    assert!(text(&r).contains("changed on disk"), "message: {}", text(&r));
}

#[tokio::test]
async fn write_guards_blind_overwrite_but_allows_new_files() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("exists.txt"), "old content").unwrap();

    let tracker = Arc::new(ReadTracker::new());
    let read = ReadTool::new().with_tracker(tracker.clone());
    let write = WriteTool::new().with_tracker(tracker.clone());
    let cx = ctx(tmp.path());

    // A brand-new file writes freely (create, no guard).
    let r = write.run(json!({ "path": "new.txt", "content": "fresh" }), &cx).await;
    assert!(!r.is_error, "new file write should succeed: {}", text(&r));

    // Overwriting an existing, unread file is rejected (blind-write).
    let r = write.run(json!({ "path": "exists.txt", "content": "clobbered" }), &cx).await;
    assert!(r.is_error, "blind overwrite must be rejected: {}", text(&r));
    assert_eq!(std::fs::read_to_string(tmp.path().join("exists.txt")).unwrap(), "old content");

    // After reading it, the overwrite is allowed.
    read.run(json!({ "path": "exists.txt" }), &cx).await;
    let r = write.run(json!({ "path": "exists.txt", "content": "intentional" }), &cx).await;
    assert!(!r.is_error, "read+overwrite should succeed: {}", text(&r));
    assert_eq!(std::fs::read_to_string(tmp.path().join("exists.txt")).unwrap(), "intentional");
}

/// The read record lives in the process, not in the transcript, so a resumed session legitimately
/// starts with none. The rejection has to say that: a model told only "you haven't read this" while
/// looking at its own earlier read concludes the tool is broken and starts hand-editing around it.
#[tokio::test]
async fn a_rejection_explains_that_the_read_record_is_per_process() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("f.txt"), "hello").unwrap();
    let cx = ctx(tmp.path());

    // A fresh tracker is exactly the state a resumed session starts in.
    let tracker = Arc::new(ReadTracker::new());
    let edit = EditTool::new().with_tracker(tracker.clone());
    let write = WriteTool::new().with_tracker(tracker.clone());

    let rejected = edit
        .run(json!({ "path": "f.txt", "edits": [{ "oldText": "hello", "newText": "hi" }] }), &cx)
        .await;
    let message = text(&rejected);
    assert!(message.contains("this Luxe run"), "names the scope: {message}");
    assert!(message.contains("resumed session"), "names the resume case: {message}");
    assert!(message.contains("nothing was written"), "says the file is untouched: {message}");

    let rejected = write.run(json!({ "path": "f.txt", "content": "clobber" }), &cx).await;
    let message = text(&rejected);
    assert!(message.contains("read-before-write guard"), "names the guard: {message}");
    assert!(message.contains("resumed session"), "names the resume case: {message}");
    assert_eq!(std::fs::read_to_string(tmp.path().join("f.txt")).unwrap(), "hello");
}

#[tokio::test]
async fn guards_are_skipped_without_a_tracker() {
    // Default tools (no tracker) preserve the old permissive behavior — edit an unread file.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("f.txt"), "hello").unwrap();
    let edit = EditTool::new();
    let cx = ctx(tmp.path());
    let r = edit
        .run(json!({ "path": "f.txt", "edits": [{ "oldText": "hello", "newText": "hi" }] }), &cx)
        .await;
    assert!(!r.is_error, "no-tracker edit should be permissive: {}", text(&r));
}

/// A file the model printed with `cat` has been looked at, so it may be edited; one it only piped
/// somewhere, or one a failed command named, has not.
#[cfg(unix)]
#[tokio::test]
async fn a_file_printed_by_cat_counts_as_read() {
    use luxe_tools::bash::BashTool;
    let tmp = tempfile::tempdir().unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(tmp.path().join(name), "hello world").unwrap();
    }
    let tracker = Arc::new(ReadTracker::new());
    let bash = BashTool::new().with_tracker(tracker.clone());
    let edit = EditTool::new().with_tracker(tracker.clone());
    let cx = ctx(tmp.path());
    let change =
        |path: &str| json!({ "path": path, "edits": [{ "oldText": "world", "newText": "there" }] });

    bash.run(json!({ "command": "cat a.txt; cat b.txt | wc -c" }), &cx).await;
    let r = edit.run(change("a.txt"), &cx).await;
    assert!(!r.is_error, "printed, so seen: {}", text(&r));
    let r = edit.run(change("b.txt"), &cx).await;
    assert!(r.is_error, "piped into wc, so never shown: {}", text(&r));

    bash.run(json!({ "command": "cat c.txt missing.txt" }), &cx).await;
    let r = edit.run(change("c.txt"), &cx).await;
    assert!(r.is_error, "the command failed, so nothing counts: {}", text(&r));
}
