use super::*;
use luxe_core::permission::{
    PermissionAction::{Allow, Ask, Deny},
    PermissionEngine, PermissionMode, Rule,
};

fn tree() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("nested")).unwrap();
    for name in [".env", "nested/.env", "nested/.env.example", "safe.rs", "review.rs"] {
        std::fs::write(dir.path().join(name), "content is not needed for metadata").unwrap();
    }
    dir
}

fn read_rule(pattern: &str, action: luxe_core::permission::PermissionAction) -> Rule {
    Rule { permission: "read".into(), pattern: pattern.into(), action }
}

async fn guarded(dir: &Path, args: serde_json::Value, policy: &PermissionEngine) -> String {
    let result = GlobTool::new().run_with_read_hook(args, &test_context(dir), policy).await;
    assert!(!result.is_error, "{:?}", result.content);
    result
        .content
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Metadata discovery must consult concrete path policy rather than expose denied names from a directory.
#[tokio::test]
async fn guarded_glob_withholds_secret_descendants_and_keeps_templates() {
    let dir = tree();
    let text = guarded(
        dir.path(),
        serde_json::json!({"pattern": "*", "all": true}),
        &PermissionEngine::permissive(),
    )
    .await;
    assert!(!text.lines().any(|line| line == ".env" || line == "nested/.env"), "{text}");
    assert!(text.lines().any(|line| line == "nested/.env.example"), "{text}");
    assert!(text.contains("safe.rs") && text.contains("skipped"), "{text}");
}

/// A root-file exception must not approve another directory's env name, and Ask is still withheld.
#[tokio::test]
async fn guarded_glob_honors_actual_path_exceptions_and_ask() {
    let dir = tree();
    let policy = PermissionEngine::assemble(
        PermissionMode::Default,
        vec![read_rule(".env", Allow), read_rule("review.rs", Ask)],
        vec![],
        false,
    );
    let text = guarded(dir.path(), serde_json::json!({"pattern": "*", "all": true}), &policy).await;
    assert!(text.lines().any(|line| line == ".env"), "{text}");
    assert!(!text.lines().any(|line| line == "nested/.env" || line == "review.rs"), "{text}");
    assert!(text.contains("approval"), "{text}");
}

/// Yolo opts out of default filters while preserving an explicit user path denial.
#[tokio::test]
async fn guarded_glob_keeps_user_denials_under_yolo() {
    let dir = tree();
    let policy = PermissionEngine::assemble(
        PermissionMode::Yolo,
        vec![read_rule("safe.rs", Deny)],
        vec![],
        false,
    );
    let text = guarded(dir.path(), serde_json::json!({"pattern": "*", "all": true}), &policy).await;
    assert!(text.lines().any(|line| line == "nested/.env"), "{text}");
    assert!(!text.lines().any(|line| line == "safe.rs"), "{text}");
}

/// Ignores and glob matching must happen without changing the policy-free tool's ordinary behavior.
#[tokio::test]
async fn guarded_glob_preserves_ignored_file_and_pattern_behavior() {
    let dir = tree();
    std::fs::create_dir_all(dir.path().join("target")).unwrap();
    std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
    std::fs::write(dir.path().join("target/build.rs"), "").unwrap();
    let policy = PermissionEngine::permissive();
    let normal = guarded(dir.path(), serde_json::json!({"pattern": "*.rs"}), &policy).await;
    assert!(!normal.contains("target/build.rs"), "{normal}");
    let all =
        guarded(dir.path(), serde_json::json!({"pattern": "*.rs", "all": true}), &policy).await;
    assert!(all.contains("target/build.rs"), "{all}");
    assert!(!all.contains(".env"), "{all}");
}
