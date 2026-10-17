//! Baseline — tests for the file-changed hook.

use super::*;
use crate::lsp::Diagnostic;
use std::collections::VecDeque;
use std::sync::Mutex;

/// Answers each diagnostics request with the next scripted list.
struct Scripted(Mutex<VecDeque<Vec<Diagnostic>>>);

#[async_trait]
impl crate::lsp::DiagnosticsProvider for Scripted {
    async fn run_diagnostics(
        &self,
        _path: &std::path::Path,
    ) -> Result<Vec<Diagnostic>, crate::lsp::DiagnosticsError> {
        Ok(self.0.lock().expect("lock").pop_front().unwrap_or_default())
    }
}

fn diag(line: u32, message: &str) -> Diagnostic {
    Diagnostic {
        severity: Some(crate::lsp::Severity::Error),
        line,
        character: 0,
        message: message.to_string(),
    }
}

fn hook(script: Vec<Vec<Diagnostic>>) -> FileChangedHook {
    let lsp: Arc<dyn crate::lsp::DiagnosticsProvider> =
        Arc::new(Scripted(Mutex::new(script.into())));
    FileChangedHook::new(None, Some(lsp))
}

fn cx() -> HookCtx {
    HookCtx {
        session_id: "test".to_string(),
        event_tx: tokio::sync::mpsc::channel(1).0,
        hook_registry: crate::hook::HookRegistry::new(),
    }
}

async fn change(hook: &FileChangedHook, path: &std::path::Path) -> Option<String> {
    let event = HookEvent::FileChanged {
        path: path.to_string_lossy().into_owned(),
        operation: "edit".to_string(),
    };
    match hook.on(HookPoint::FileChanged, &event, &cx()).await {
        HookOutcome::InjectContext(report) => Some(report),
        _ => None,
    }
}

async fn about_to(hook: &FileChangedHook, tool: &str, path: &std::path::Path) {
    let event = HookEvent::ToolUse {
        tool_name: tool.to_string(),
        args: serde_json::json!({ "path": path.to_string_lossy() }),
    };
    hook.on(HookPoint::BeforeToolUse, &event, &cx()).await;
}

/// What was wrong before the edit stays out of the report, wherever the edit moved it; what
/// the edit added is reported, and the rest is counted.
#[tokio::test]
async fn an_edit_reports_what_it_introduced_and_counts_the_rest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("views.py");
    std::fs::write(&file, "x\n").expect("write");
    let hook = hook(vec![
        vec![diag(4, "Import \"django\" could not be resolved"), diag(9, "old")],
        vec![diag(6, "Import \"django\" could not be resolved"), diag(11, "old"), diag(7, "new")],
        vec![diag(6, "Import \"django\" could not be resolved"), diag(11, "old"), diag(7, "new")],
    ]);

    about_to(&hook, "edit", &file).await;
    let report = change(&hook, &file).await.expect("the new error is reported");
    assert!(report.contains("(1 new, 1 error(s))"), "{report}");
    assert!(report.contains("error 8:1: new"), "{report}");
    assert!(!report.contains("could not be resolved") && !report.contains(": old"), "{report}");
    assert!(report.contains("2 more were in the file before this change"), "{report}");

    // The next edit is measured against the last one, so the same error is not news again
    about_to(&hook, "edit", &file).await;
    assert_eq!(change(&hook, &file).await, None);
}

/// A file `write` creates had nothing wrong with it, so everything in it is new.
#[tokio::test]
async fn a_new_file_reports_everything_as_new() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("new.py");
    let hook = hook(vec![vec![diag(0, "undefined name")]]);

    about_to(&hook, "write", &file).await;
    std::fs::write(&file, "x\n").expect("write");
    let report = change(&hook, &file).await.expect("reported");
    assert!(report.contains("(1 new, 1 error(s))") && !report.contains("more were"), "{report}");
}

/// Without a baseline — no hook saw the call coming — the report is the whole list, as before.
#[tokio::test]
async fn without_a_baseline_everything_is_reported() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("a.py");
    std::fs::write(&file, "x\n").expect("write");
    let hook = hook(vec![vec![diag(0, "old"), diag(1, "older")]]);
    let report = change(&hook, &file).await.expect("reported");
    assert!(report.contains("(2 total"), "{report}");
}
