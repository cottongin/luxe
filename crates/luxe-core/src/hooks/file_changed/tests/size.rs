//! Size — tests for the file-changed hook.

use super::*;

/// Every line `def name` opens a function running to the next one or the end.
struct Defs;

impl OutlineSource for Defs {
    fn functions(&self, _path: &std::path::Path, source: &str) -> Option<Vec<FunctionSize>> {
        let lines: Vec<&str> = source.lines().collect();
        let starts: Vec<usize> =
            (0..lines.len()).filter(|i| lines[*i].starts_with("def ")).collect();
        Some(
            starts
                .iter()
                .enumerate()
                .map(|(n, start)| FunctionSize {
                    name: lines[*start][4..].to_string(),
                    lines: starts.get(n + 1).unwrap_or(&lines.len()) - start,
                })
                .collect(),
        )
    }
}

fn cx() -> HookCtx {
    HookCtx {
        session_id: "test".to_string(),
        event_tx: tokio::sync::mpsc::channel(1).0,
        hook_registry: crate::hook::HookRegistry::new(),
    }
}

fn body(name: &str, lines: usize) -> String {
    format!("def {name}\n{}", "    pass\n".repeat(lines - 1))
}

/// The edit that pushes a function past 20 lines hears about it, alongside nothing else; the
/// next edit to the same file, crossing nothing new, hears nothing.
#[tokio::test]
async fn an_edit_that_crosses_a_limit_is_told_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("views.py");
    std::fs::write(&file, body("short", 10)).expect("write");
    let hook = FileChangedHook::new(None, None).with_outline(Arc::new(Defs));
    let path = file.to_string_lossy().into_owned();
    let about_to = HookEvent::ToolUse {
        tool_name: "edit".to_string(),
        args: serde_json::json!({ "path": path }),
    };
    let changed = HookEvent::FileChanged { path: path.clone(), operation: "edit".to_string() };

    hook.on(HookPoint::BeforeToolUse, &about_to, &cx()).await;
    std::fs::write(&file, body("short", 30)).expect("write");
    let HookOutcome::InjectContext(note) = hook.on(HookPoint::FileChanged, &changed, &cx()).await
    else {
        panic!("the crossing is reported");
    };
    assert!(note.contains("`short` (30 lines)"), "{note}");

    hook.on(HookPoint::BeforeToolUse, &about_to, &cx()).await;
    std::fs::write(&file, body("short", 35)).expect("write");
    assert!(matches!(
        hook.on(HookPoint::FileChanged, &changed, &cx()).await,
        HookOutcome::Continue
    ));
}
