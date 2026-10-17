//! Search scopes — read-policy and external-directory checks for tool search roots.

use std::path::Path;

use super::{is_external, normalize, rule, Check, Rule, EXTERNAL_DIRECTORY};
use crate::permission::PermissionAction;

pub(super) fn search_roots(tool: &str, args: &serde_json::Value) -> Vec<String> {
    if tool == "ast_grep" {
        return ast_grep_roots(args);
    }
    if matches!(tool, "module_report" | "read_symbol" | "lsp_navigation") {
        return lsp_file_roots(args);
    }
    if tool == "lsp_diagnostics" {
        return diagnostics_roots(args);
    }
    if !matches!(tool, "grep" | "glob") {
        return Vec::new();
    }
    let path = args
        .get("path")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .unwrap_or(".");
    vec![path.into()]
}

pub(super) fn ast_grep_roots(args: &serde_json::Value) -> Vec<String> {
    let mut paths = match args.get("paths").or_else(|| args.get("path")) {
        Some(serde_json::Value::String(path)) => vec![path.clone()],
        Some(serde_json::Value::Array(paths)) => {
            paths.iter().filter_map(|path| path.as_str().map(str::to_string)).collect()
        }
        _ => Vec::new(),
    };
    if paths.is_empty() {
        paths.push(".".into());
    }
    paths
}

pub(super) fn lsp_file_roots(args: &serde_json::Value) -> Vec<String> {
    ["path", "file", "file_path", "filePath"]
        .into_iter()
        .find_map(|key| args.get(key).and_then(serde_json::Value::as_str))
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .into_iter()
        .collect()
}

fn diagnostics_roots(args: &serde_json::Value) -> Vec<String> {
    if let Some(paths) = args.get("paths").and_then(serde_json::Value::as_array) {
        return paths.iter().filter_map(|path| path.as_str().map(str::to_string)).collect();
    }
    lsp_file_roots(args)
}

pub(super) fn grep_selector_checks(
    tool: &str,
    args: &serde_json::Value,
    paths: &[String],
) -> Vec<Check> {
    if tool != "grep" {
        return Vec::new();
    }
    let Some(selector) = args.get("glob").and_then(serde_json::Value::as_str) else {
        return Vec::new();
    };
    if selector.is_empty()
        || matches!(selector, "." | "..")
        || selector.contains(['*', '?', '[', ']', '{', '}', '!', '/', '\\'])
    {
        return Vec::new();
    }
    paths
        .iter()
        .map(|path| Check {
            permission: "read".into(),
            input: exact_selector_target(path, selector),
        })
        .collect()
}

fn exact_selector_target(scope: &str, selector: &str) -> String {
    let scope_path = Path::new(scope);
    if scope_path.file_name().and_then(|name| name.to_str()) == Some(selector) {
        return scope.into();
    }
    crate::paths::display(&scope_path.join(selector))
}

pub(super) fn root_checks(paths: &[String], root: &Path) -> Vec<Check> {
    let mut checks = Vec::new();
    for path in paths {
        checks.push(Check { permission: "read".into(), input: path.clone() });
        if is_external(path, root) {
            checks.push(Check {
                permission: EXTERNAL_DIRECTORY.into(),
                input: crate::paths::display(&normalize(path, root)),
            });
        }
    }
    checks
}

/// The rules that remember one out-of-project scope: the scope itself and everything under it.
///
/// Only the `external_directory` guard; a pattern-wide rule on the tool could not honestly cover
/// every file the scope holds.
pub(super) fn external_scope_rules(scope: &str, root: &Path) -> Vec<Rule> {
    if !is_external(scope, root) {
        return Vec::new();
    }
    let absolute = crate::paths::display(&normalize(scope, root));
    let subtree = format!("{}/*", absolute.trim_end_matches('/'));
    [absolute, subtree]
        .iter()
        .map(|pattern| rule(EXTERNAL_DIRECTORY, pattern, PermissionAction::Allow))
        .collect()
}
