//! `glob` tool — find files by name or path shape.
//!
//! Better than shelling out, not merely different: it respects the project's ignore files, skips
//! VCS and build output, filters every path through read policy, caps the result
//! deterministically, and speaks one glob dialect on every platform. It walks the tree itself
//! ([`crate::walk`]), which is what lets it ask read policy about each path before returning it.

use async_trait::async_trait;
use luxe_core::{ContentBlock, Tool, ToolContext, ToolResultData, ToolSchema};
use serde_json::json;

mod guarded;

/// JSON-schema for the glob tool arguments.
fn glob_schema() -> ToolSchema {
    ToolSchema {
        name: "glob".to_string(),
        description: "Find files by name or path shape (`*.rs`, `src/**/*.ts`, `**/test_*.py`). \
                      Skips ignored paths — `.gitignore`, `.git/`, build output — and returns at \
                      most 200 paths, so it cannot bury the answer the way `find`/`ls -R` can. Set \
                      `all: true` to include ignored files (build output, `.env`, vendored deps). \
                      Searching file *contents* is `grep`; finding a symbol's definition is \
                      `symbol_search`."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match (e.g., '*.rs', 'src/**/*.ts')"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (defaults to cwd)"
                },
                "all": {
                    "type": "boolean",
                    "description": "Include ignored files (build output, vendored deps). Default false."
                }
            },
            "required": ["pattern"]
        }),
    }
}

/// Max file paths returned to the model before the result is capped.
const GLOB_MAX_FILES: usize = 200;

/// The `glob` tool implementation.
pub struct GlobTool;

impl GlobTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GlobTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn schema(&self) -> ToolSchema {
        glob_schema()
    }

    async fn run_with_read_hook(
        &self,
        args: serde_json::Value,
        cx: &ToolContext,
        read_hook: &dyn luxe_core::tool::FileReadHook,
    ) -> ToolResultData {
        guarded::run(args, cx, read_hook).await
    }

    async fn run(&self, args: serde_json::Value, cx: &ToolContext) -> ToolResultData {
        guarded::run(args, cx, &crate::NoReadPolicy).await
    }
}

#[cfg(test)]
mod tests;

/// What a walk from `path` matches, as the tool would list it.
#[cfg(test)]
fn matches(cwd: &std::path::Path, path: &str, pattern: &str) -> Vec<String> {
    let matcher = crate::walk::matcher(pattern).unwrap();
    crate::walk::files(cwd, std::path::Path::new(path), false)
        .into_iter()
        .filter(|found| matcher.is_match(found))
        .map(|found| luxe_core::paths::display(&found))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod clones {
    use super::*;

    /// A project with a fetched repository in it: `.luxe/` ignores everything, the clone is a git
    /// repository of its own and ignores its own `node_modules/`.
    fn project_with_a_clone() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let clone = root.join(".luxe/gh-cache/o/r");
        std::fs::create_dir_all(clone.join(".git")).unwrap();
        std::fs::create_dir_all(clone.join("docs")).unwrap();
        std::fs::create_dir_all(clone.join("node_modules/x")).unwrap();
        std::fs::write(root.join(".luxe/.gitignore"), "*\n").unwrap();
        std::fs::write(clone.join(".gitignore"), "node_modules/\n").unwrap();
        std::fs::write(clone.join("docs/a.md"), "needle\n").unwrap();
        std::fs::write(clone.join("node_modules/x/b.md"), "needle\n").unwrap();
        dir
    }

    /// `web_fetch` says to explore a clone with `glob`, so `.luxe/.gitignore` must not hide every
    /// file in it. The clone's own ignore file still counts.
    #[test]
    fn a_fetched_repository_is_searchable_by_its_own_rules() {
        let dir = project_with_a_clone();
        let found = matches(dir.path(), ".luxe/gh-cache/o/r", "**/*.md");
        assert_eq!(found, [".luxe/gh-cache/o/r/docs/a.md"]);
        assert!(
            matches(dir.path(), ".luxe", "**/*.md").is_empty(),
            "the rest of .luxe/ stays ignored"
        );
    }

    /// Only a clone searches by its own rules: a subdirectory of the project still answers to the
    /// project's `.gitignore`.
    #[test]
    fn a_subdirectory_of_the_project_still_honors_the_projects_ignore_file() {
        let dir = project_with_a_clone();
        let cwd = dir.path();
        std::fs::write(cwd.join(".gitignore"), "src/gen/\n").unwrap();
        std::fs::create_dir_all(cwd.join("src/gen")).unwrap();
        std::fs::write(cwd.join("src/a.rs"), "").unwrap();
        std::fs::write(cwd.join("src/gen/b.rs"), "").unwrap();
        assert_eq!(matches(cwd, "src", "**/*.rs"), ["src/a.rs"]);
    }
}
