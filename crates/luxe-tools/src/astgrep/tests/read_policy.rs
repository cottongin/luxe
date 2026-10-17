use super::*;
use luxe_core::permission::PermissionAction::Deny;
#[cfg(unix)]
use luxe_core::permission::{
    PermissionAction::{Allow, Ask},
    PermissionEngine, PermissionMode, Rule,
};
use luxe_core::tool::FileReadHook;

#[cfg(unix)]
fn policy_rule(
    permission: &str,
    pattern: &str,
    action: luxe_core::permission::PermissionAction,
) -> Rule {
    Rule { permission: permission.into(), pattern: pattern.into(), action }
}

fn context(cwd: &std::path::Path) -> ToolContext {
    let (_, cancelled) = tokio::sync::watch::channel(false);
    let (event_tx, _) = tokio::sync::mpsc::channel(8);
    ToolContext {
        cwd: cwd.into(),
        session_id: "ast-read-policy".into(),
        cancelled: std::sync::Arc::new(cancelled),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    }
}

#[cfg(unix)]
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("data/nested")).unwrap();
    for (path, text) in [
        (".env", "ROOT_PRIVATE"),
        ("nested/.env", "CHILD_PRIVATE"),
        ("nested/.env.example", "PUBLIC_TEMPLATE"),
        ("safe.rs", "SAFE_TEXT"),
        ("review.rs", "REVIEW_TEXT"),
    ] {
        std::fs::write(dir.path().join("data").join(path), text).unwrap();
    }
    fake_binary(
        dir.path(),
        "ast-policy-cli",
        r#"#!/bin/sh
for arg in "$@"; do
  if [ -d "$arg" ]; then find "$arg" -type f
  elif [ -f "$arg" ]; then printf '%s\n' "$arg"
  fi
done | while IFS= read -r file; do
  text=$(cat "$file")
  printf '{"file":"%s","text":"%s","range":{"byteOffset":{"start":0,"end":1},"start":{"line":0,"column":0}}}\n' "$file" "$text"
done
"#,
    );
    BINARY.set(dir.path().join("ast-policy-cli").to_string_lossy().into_owned()).unwrap();
    dir
}

/// A directory matcher must not see denied/Ask content before its output is filtered.
#[cfg(unix)]
#[tokio::test]
async fn the_matcher_receives_only_policy_approved_ast_files() {
    let dir = fixture();
    let data = dir.path().join("data");
    let policy = PermissionEngine::assemble(
        PermissionMode::Default,
        vec![policy_rule("read", ".env", Allow), policy_rule("read", "review.rs", Ask)],
        vec![],
        false,
    );
    for action in ["search", "replace"] {
        let args = serde_json::json!({"action": action, "pattern": "foo($A)", "rewrite": "bar($A)", "lang": "rust", "paths": [data]});
        let result = AstGrepTool::new().run_with_read_hook(args, &context(&data), &policy).await;
        assert!(!result.is_error, "{:?}", result.content);
        let text = result
            .content
            .iter()
            .filter_map(|block| match block {
                luxe_core::ContentBlock::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("ROOT_PRIVATE")
                && text.contains("PUBLIC_TEMPLATE")
                && text.contains("SAFE_TEXT"),
            "{text}"
        );
        assert!(!text.contains("CHILD_PRIVATE") && !text.contains("REVIEW_TEXT"), "{text}");
        assert!(text.contains("skipped") && text.contains("approval"), "{text}");
    }
}

/// Approved files go to the matcher absolute, but the model reads them project-relative, as an
/// unguarded run shows them.
#[cfg(unix)]
#[tokio::test]
async fn guarded_matches_are_reported_relative_to_the_working_directory() {
    let dir = fixture();
    let data = dir.path().join("data");
    let args = serde_json::json!({"action": "search", "pattern": "foo($A)", "lang": "rust"});
    let result = AstGrepTool::new()
        .run_with_read_hook(args, &context(&data), &PermissionEngine::permissive())
        .await;
    let luxe_core::ContentBlock::Text(text) = &result.content[0] else { panic!("{result:?}") };
    assert!(text.contains("\nsafe.rs:1:0"), "{text}");
    assert!(!text.contains(&data.display().to_string()), "{text}");
}

struct DenyEveryFile;
impl FileReadHook for DenyEveryFile {
    fn decision(&self, _: &std::path::Path) -> luxe_core::permission::PermissionAction {
        Deny
    }
}

/// With no approved files, no matcher invocation may fall back to the current directory.
#[tokio::test]
async fn a_fully_denied_ast_scope_never_starts_the_matcher() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("safe.rs"), "foo(1)").unwrap();
    let args = serde_json::json!({"action": "search", "pattern": "foo($A)", "lang": "rust", "paths": [dir.path()]});
    let result =
        AstGrepTool::new().run_with_read_hook(args, &context(dir.path()), &DenyEveryFile).await;
    assert!(!result.is_error, "{:?}", result.content);
    let luxe_core::ContentBlock::Text(body) = &result.content[0] else { panic!("{result:?}") };
    assert_eq!(body, "no policy-approved files to search");
    assert!(
        result.content.iter().any(
            |block| matches!(block, luxe_core::ContentBlock::Text(text) if text.contains("skipped"))
        ),
        "{:?}",
        result.content
    );
}
