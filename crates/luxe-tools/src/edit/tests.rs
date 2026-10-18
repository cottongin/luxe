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

/// A stand-in formatter: on `FileChanged` it rewrites the file the way rustfmt would.
struct Spacer;

#[async_trait::async_trait]
impl luxe_core::hook::Hook for Spacer {
    fn name(&self) -> &'static str {
        "spacer"
    }
    fn interest(&self) -> &[luxe_core::hook::HookPoint] {
        &[luxe_core::hook::HookPoint::FileChanged]
    }
    async fn on(
        &self,
        _: luxe_core::hook::HookPoint,
        ev: &luxe_core::hook::HookEvent,
        _: &luxe_core::hook::HookCtx,
    ) -> luxe_core::hook::HookOutcome {
        if let luxe_core::hook::HookEvent::FileChanged { path, .. } = ev {
            let text = std::fs::read_to_string(path).unwrap();
            std::fs::write(path, text.replace("a:i32", "a: i32")).unwrap();
        }
        luxe_core::hook::HookOutcome::Continue
    }
}

pub(crate) fn formatting_context(tmp_dir: &std::path::Path) -> ToolContext {
    let mut hook_registry = luxe_core::hook::HookRegistry::new();
    hook_registry.register(Arc::new(Spacer));
    let mut cx = test_context(tmp_dir);
    cx.hook_ctx = Some(luxe_core::hook::HookCtx {
        session_id: cx.session_id.clone(),
        event_tx: cx.event_tx.clone(),
        hook_registry,
    });
    cx
}

/// When format-on-save rewrites the edit, the result shows the file as it now is and says so,
/// because the next edit is matched against that text and not the text the call sent.
#[tokio::test]
async fn a_reformatted_edit_shows_the_file_as_it_is_now() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.rs"), "fn a() {}\nfn sub() {}\n").unwrap();
    let args = json!({"path": "lib.rs", "edits": [
        {"oldText": "fn sub() {}", "newText": "fn sub(a:i32) {}"}
    ]});
    let result = EditTool::new().run(args, &formatting_context(dir.path())).await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.starts_with("Applied 1 edit(s) to 'lib.rs'"), "named from the project: {text}");
    assert!(text.contains("Format-on-save then reformatted the file"), "{text}");
    assert!(text.contains("+2 fn sub(a: i32) {}"), "the formatted line: {text}");
    assert!(!text.contains("a:i32"), "not the line as sent: {text}");

    // a formatter with nothing to change says nothing
    let args = json!({"path": "lib.rs", "edits": [{"oldText": "fn a()", "newText": "fn b()"}]});
    let result = EditTool::new().run(args, &formatting_context(dir.path())).await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(!text.contains("Format-on-save"), "{text}");
    assert!(text.contains("+1 fn b() {}"), "{text}");
}

/// An edited script stays executable. Saving by rename replaced the file with a new one at the
/// default mode, so every edit of a script left it unrunnable.
#[cfg(unix)]
#[tokio::test]
async fn an_edited_script_stays_executable() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("run.sh");
    std::fs::write(&script, "#!/bin/sh\necho one\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let args = json!({"path": "run.sh", "edits": [{"oldText": "one", "newText": "two"}]});
    let result = EditTool::new().run(args, &test_context(dir.path())).await;
    assert!(!result.is_error, "{:?}", result.content);
    let mode = std::fs::metadata(&script).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "still executable after the edit");
}

#[tokio::test]
async fn edit_tool_basic() {
    let tmp_dir_guard = tempfile::tempdir().expect("temp dir");
    let tmp_dir = tmp_dir_guard.path().to_path_buf();
    std::fs::write(tmp_dir.join("test.txt"), "Hello, World!").unwrap();

    let tool = EditTool::new();
    let cx = test_context(&tmp_dir);

    let args = json!({
        "path": "test.txt",
        "edits": [
            {
                "oldText": "World",
                "newText": "Rust"
            }
        ]
    });
    let result = tool.run(args, &cx).await;

    assert!(!result.is_error);
    if let ContentBlock::Text(text) = &result.content[0] {
        assert!(text.contains("Applied 1 edit(s)"));
        // The preview shows whole numbered lines, not just the replaced fragment.
        assert!(
            text.contains("-1 Hello, World!") && text.contains("+1 Hello, Rust!"),
            "shows the line-numbered diff: {text}"
        );
    } else {
        panic!("expected Text block");
    }

    // Verify file was edited.
    let content = std::fs::read_to_string(tmp_dir.join("test.txt")).unwrap();
    assert_eq!(content, "Hello, Rust!");
}

#[tokio::test]
async fn edit_tool_multi_edit() {
    let tmp_dir_guard = tempfile::tempdir().expect("temp dir");
    let tmp_dir = tmp_dir_guard.path().to_path_buf();
    std::fs::write(tmp_dir.join("test.txt"), "line1\nline2\nline3").unwrap();

    let tool = EditTool::new();
    let cx = test_context(&tmp_dir);

    let args = json!({
        "path": "test.txt",
        "edits": [
            {
                "oldText": "line1",
                "newText": "first"
            },
            {
                "oldText": "line3",
                "newText": "third"
            }
        ]
    });
    let result = tool.run(args, &cx).await;

    assert!(!result.is_error);

    // Verify file was edited.
    let content = std::fs::read_to_string(tmp_dir.join("test.txt")).unwrap();
    assert_eq!(content, "first\nline2\nthird");
}

#[tokio::test]
async fn edit_tool_not_found() {
    let tmp_dir_guard = tempfile::tempdir().expect("temp dir");
    let tmp_dir = tmp_dir_guard.path().to_path_buf();
    std::fs::write(tmp_dir.join("test.txt"), "Hello, World!").unwrap();

    let tool = EditTool::new();
    let cx = test_context(&tmp_dir);

    let args = json!({
        "path": "test.txt",
        "edits": [
            {
                "oldText": "nonexistent",
                "newText": "replacement"
            }
        ]
    });
    let result = tool.run(args, &cx).await;

    assert!(result.is_error);
}

#[tokio::test]
async fn edit_tool_ambiguous_match() {
    let tmp_dir_guard = tempfile::tempdir().expect("temp dir");
    let tmp_dir = tmp_dir_guard.path().to_path_buf();
    std::fs::write(tmp_dir.join("test.txt"), "line\nline\nline").unwrap();

    let tool = EditTool::new();
    let cx = test_context(&tmp_dir);

    let args = json!({
        "path": "test.txt",
        "edits": [
            {
                "oldText": "line",
                "newText": "LINE"
            }
        ]
    });
    let result = tool.run(args, &cx).await;

    assert!(result.is_error);
    if let ContentBlock::Text(text) = &result.content[0] {
        assert!(text.contains("matched 3 times"));
    } else {
        panic!("expected Text block");
    }
}

#[tokio::test]
async fn edit_tool_atomic_failure() {
    let tmp_dir_guard = tempfile::tempdir().expect("temp dir");
    let tmp_dir = tmp_dir_guard.path().to_path_buf();
    std::fs::write(tmp_dir.join("test.txt"), "line1\nline2").unwrap();

    let tool = EditTool::new();
    let cx = test_context(&tmp_dir);

    // First edit succeeds, second fails — should be atomic (no changes).
    let args = json!({
        "path": "test.txt",
        "edits": [
            {
                "oldText": "line1",
                "newText": "first"
            },
            {
                "oldText": "nonexistent",
                "newText": "second"
            }
        ]
    });
    let result = tool.run(args, &cx).await;

    assert!(result.is_error);

    // Verify file was NOT edited (atomic failure).
    let content = std::fs::read_to_string(tmp_dir.join("test.txt")).unwrap();
    assert_eq!(content, "line1\nline2");
}
