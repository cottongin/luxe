use super::*;
use luxe_core::permission::{
    PermissionAction,
    PermissionAction::{Allow, Ask, Deny},
    PermissionEngine, PermissionMode, Rule,
};

fn rule(permission: &str, pattern: &str, action: PermissionAction) -> Rule {
    Rule { permission: permission.into(), pattern: pattern.into(), action }
}

fn context(cwd: &Path) -> ToolContext {
    let (_, cancelled) = tokio::sync::watch::channel(false);
    let (event_tx, _) = tokio::sync::mpsc::channel(8);
    ToolContext {
        cwd: cwd.into(),
        session_id: "grep-read-policy".into(),
        cancelled: std::sync::Arc::new(cancelled),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    }
}

fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("nested")).unwrap();
    for (path, text) in [
        (".env", "needle ROOT_PRIVATE\n"),
        ("nested/.env.local", "needle NESTED_PRIVATE\n"),
        ("nested/.env", "needle CHILD_PRIVATE\n"),
        ("nested/.env.example", "needle PUBLIC_TEMPLATE\n"),
        ("safe.txt", "needle SAFE_TEXT\n"),
        ("review.txt", "needle REVIEW_TEXT\n"),
    ] {
        std::fs::write(dir.path().join(path), text).unwrap();
    }
    dir
}

async fn guarded(dir: &Path, args: serde_json::Value, policy: &PermissionEngine) -> (String, bool) {
    let result = GrepTool::new().run_with_read_hook(args, &context(dir), policy).await;
    let text = result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    (text, result.is_error)
}

/// A broad ignored-file scan must consult actual paths before opening any env content.
#[tokio::test]
async fn guarded_grep_skips_secret_descendants_and_keeps_templates() {
    let dir = tree();
    let (text, error) = guarded(
        dir.path(),
        json!({"pattern": "needle", "all": true}),
        &PermissionEngine::permissive(),
    )
    .await;
    assert!(!error, "{text}");
    assert!(text.contains("SAFE_TEXT") && text.contains("PUBLIC_TEMPLATE"), "{text}");
    assert!(!text.contains("PRIVATE"), "{text}");
    assert!(text.contains("skipped"), "{text}");
}

/// An exception for the root env file must not approve a secret env file in a child directory.
#[tokio::test]
async fn guarded_grep_evaluates_scope_specific_exceptions_per_file() {
    let dir = tree();
    let policy = PermissionEngine::assemble(
        PermissionMode::Default,
        vec![rule("read", ".env", Allow)],
        vec![],
        false,
    );
    let (text, error) =
        guarded(dir.path(), json!({"pattern": "needle", "glob": "**/.env*", "all": true}), &policy)
            .await;
    assert!(!error, "{text}");
    assert!(text.contains("ROOT_PRIVATE"), "{text}");
    assert!(!text.contains("NESTED_PRIVATE") && !text.contains("CHILD_PRIVATE"), "{text}");
    assert!(text.contains("PUBLIC_TEMPLATE"), "{text}");
}

/// Ask is not a read allowance; skipped approval-required files must be reported, not searched.
#[tokio::test]
async fn guarded_grep_does_not_read_ask_paths() {
    let dir = tree();
    let policy = PermissionEngine::assemble(
        PermissionMode::Default,
        vec![rule("read", "review.txt", Ask)],
        vec![],
        false,
    );
    let (text, _) = guarded(dir.path(), json!({"pattern": "needle", "all": true}), &policy).await;
    assert!(!text.contains("REVIEW_TEXT"), "{text}");
    assert!(text.contains("approval"), "{text}");
}

/// Files/count output must obey the same decisions even when it does not return content lines.
#[tokio::test]
async fn guarded_grep_applies_policy_in_coarse_output_modes() {
    let dir = tree();
    for mode in ["files", "count"] {
        let (text, _) = guarded(
            dir.path(),
            json!({"pattern": "needle", "all": true, "outputMode": mode}),
            &PermissionEngine::permissive(),
        )
        .await;
        assert!(!text.contains(".env.local") && !text.contains("nested/.env:"), "{mode}: {text}");
        assert!(text.contains(".env.example"), "{mode}: {text}");
    }
}

/// Guarded grep honors a project's own `.ignore` file unless all is requested.
#[tokio::test]
async fn guarded_grep_honors_ignore_files_until_all_is_enabled() {
    let dir = tree();
    std::fs::write(dir.path().join(".ignore"), "ignored.txt\n").unwrap();
    std::fs::write(dir.path().join("ignored.txt"), "needle IGNORED_BY_FILE\n").unwrap();
    let policy = PermissionEngine::permissive();
    let (normal, _) = guarded(dir.path(), json!({"pattern": "needle"}), &policy).await;
    assert!(!normal.contains("IGNORED_BY_FILE"), "{normal}");
    let (all, _) = guarded(dir.path(), json!({"pattern": "needle", "all": true}), &policy).await;
    assert!(all.contains("IGNORED_BY_FILE"), "{all}");
}

/// Yolo removes default exclusions without ignoring an explicit user read denial.
#[tokio::test]
async fn guarded_grep_honors_yolo_and_user_denials() {
    let dir = tree();
    let policy = PermissionEngine::assemble(
        PermissionMode::Yolo,
        vec![rule("read", "safe.txt", Deny)],
        vec![],
        false,
    );
    let (text, _) = guarded(dir.path(), json!({"pattern": "needle", "all": true}), &policy).await;
    assert!(text.contains("ROOT_PRIVATE") && text.contains("CHILD_PRIVATE"), "{text}");
    assert!(!text.contains("SAFE_TEXT"), "{text}");
}
