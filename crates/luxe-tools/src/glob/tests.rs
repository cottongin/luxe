#![allow(clippy::unwrap_used)]

use super::*;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::{mpsc, watch};

mod read_policy;

fn test_context(tmp_dir: &Path) -> ToolContext {
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

/// A repo with one real source file and a build directory full of look-alikes.
fn repo_with_build_output() -> TempDir {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src").join("main.rs"), "fn main() {}").unwrap();
    let build = dir.path().join("target").join("debug").join("build");
    std::fs::create_dir_all(&build).unwrap();
    for index in 0..30 {
        std::fs::write(build.join(format!("generated_{index}.rs")), "// generated").unwrap();
    }
    dir
}

/// The defect this tool shipped with: `--no-ignore` meant a `*.rs` search in a Rust repo
/// returned build output, and the 200-file cap filled with it before any source appeared.
#[tokio::test]
async fn build_output_is_skipped_by_default() {
    let dir = repo_with_build_output();
    let tool = GlobTool::new();
    let result = tool.run(json!({"pattern": "*.rs"}), &test_context(dir.path())).await;

    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("main.rs"), "the real source file is found: {text}");
    assert!(!text.contains("generated_"), "build output is not: {text}");
}

/// A strict-schema model sends every property, so "search everywhere" arrives as `path: ""`,
/// which `fd` refused with a usage error a real eval session tripped over.
#[tokio::test]
async fn an_empty_path_searches_the_project() {
    let dir = repo_with_build_output();
    let result = GlobTool::new()
        .run(json!({"pattern": "*.rs", "path": ""}), &test_context(dir.path()))
        .await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(!result.is_error && text.contains("main.rs"), "{text}");
}

#[tokio::test]
async fn ignored_files_are_reachable_on_request() {
    let dir = repo_with_build_output();
    let tool = GlobTool::new();
    let result = tool.run(json!({"pattern": "*.rs", "all": true}), &test_context(dir.path())).await;

    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("generated_"), "all:true reaches build output: {text}");
}

/// The walk agrees with what the old `find` version got wrong: ignore files are honored, `**/`
/// matches zero directories (so `src/**/*.rs` finds `src/main.rs`, the common case, which
/// `find -path` silently skipped), and paths come back project-relative.
#[test]
fn the_walk_honors_ignores_depth_and_relative_paths() {
    let dir = repo_with_build_output();
    let cwd = dir.path();
    assert_eq!(matches(cwd, ".", "*.rs"), ["src/main.rs"], "build output stays out, path relative");
    assert_eq!(matches(cwd, ".", "src/**/*.rs"), ["src/main.rs"], "`**/` matches zero directories");
    assert!(matches(cwd, ".", "*.nope").is_empty());
}

/// An empty result reads as "no such files". A pattern that will not compile is a different
/// thing entirely and has to say so, or the model retries the same broken glob.
#[tokio::test]
async fn a_broken_glob_is_reported_rather_than_finding_nothing() {
    let dir = repo_with_build_output();
    let result = GlobTool::new().run(json!({"pattern": "src/["}), &test_context(dir.path())).await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(result.is_error && text.contains("not a valid glob"), "{text}");
}

/// Absolute paths on every row are a tax the model pays twice: once to read them, once to
/// strip them before passing one to `read`.
#[tokio::test]
async fn results_are_project_relative() {
    let dir = repo_with_build_output();
    let tool = GlobTool::new();
    let result = tool.run(json!({"pattern": "*.rs"}), &test_context(dir.path())).await;

    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("src/main.rs"), "{text}");
    assert!(!text.contains(dir.path().to_string_lossy().as_ref()), "no absolute prefix: {text}");
}

#[tokio::test]
async fn a_directory_spanning_pattern_matches() {
    let dir = repo_with_build_output();
    let tool = GlobTool::new();
    let result = tool.run(json!({"pattern": "src/**/*.rs"}), &test_context(dir.path())).await;

    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("main.rs"), "path-spanning globs work: {text}");
}

#[tokio::test]
async fn no_match_says_that_ignored_paths_were_skipped() {
    let dir = repo_with_build_output();
    let tool = GlobTool::new();
    let result = tool.run(json!({"pattern": "*.nope"}), &test_context(dir.path())).await;

    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("all:true"), "the empty result names the escape hatch: {text}");
}

#[tokio::test]
async fn test_glob_missing_pattern() {
    let tool = GlobTool::new();
    let tmp_dir = TempDir::new().unwrap();
    let result = tool.run(json!({}), &test_context(tmp_dir.path())).await;
    assert!(result.is_error);
}
