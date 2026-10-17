use super::*;
use crate::permission::{
    PermissionAction::{Allow, Ask, Deny},
    PermissionEngine, PermissionMode,
};

fn replacement(extra: serde_json::Value) -> PermissionRequest {
    let mut args =
        json!({"action": "replace", "pattern": "foo($A)", "rewrite": "bar($A)", "lang": "rust"});
    args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    build_request("ast_grep", &args, &root())
}

/// Applied structural replacements must be denied in read-only mode and ask in strict mode.
#[test]
fn an_applied_ast_grep_replacement_is_a_mutating_edit() {
    let request = replacement(json!({"apply": true, "paths": ["src"]}));
    for (mode, expected) in [(PermissionMode::ReadOnly, Deny), (PermissionMode::Strict, Ask)] {
        let engine = PermissionEngine::assemble(mode, vec![], vec![], false);
        assert_eq!(engine.decide(&request), expected, "{mode:?}");
    }
    assert!(request.checks.iter().any(|check| check.permission == "edit" && check.input == "src"));
    assert_eq!(request.tool, "ast_grep");
}

/// Previewing a replacement must not acquire a write check or lose the tool's read mode.
#[test]
fn ast_grep_previews_are_not_mutating_edits() {
    for extra in [json!({}), json!({"apply": false}), json!({"apply": "true"})] {
        let request = replacement(extra);
        assert!(!request.checks.iter().any(|check| check.permission == "edit"));
        for mode in [PermissionMode::ReadOnly, PermissionMode::Strict] {
            let engine = PermissionEngine::assemble(mode, vec![], vec![], false);
            assert_eq!(engine.decide(&request), Allow, "{mode:?}");
        }
    }
}

/// The search action ignores apply, so a stray apply field must not turn search into an edit.
#[test]
fn ast_grep_search_does_not_acquire_edit_checks() {
    let request = build_request(
        "ast_grep",
        &json!({"action": "search", "apply": true, "paths": ["src"]}),
        &root(),
    );
    assert!(!request.checks.iter().any(|check| check.permission == "edit"));
}

/// Every supplied root needs its own edit check; judging only the first loses later denials.
#[test]
fn applied_ast_grep_checks_every_supplied_path() {
    let request = replacement(json!({"apply": true, "paths": ["src", "tests"]}));
    let engine = PermissionEngine::assemble(
        PermissionMode::Default,
        vec![rule("edit", "tests", Deny)],
        vec![],
        false,
    );
    assert_eq!(engine.decide(&request), Deny);
    let paths: Vec<_> = request
        .checks
        .iter()
        .filter(|check| check.permission == "edit")
        .map(|check| check.input.as_str())
        .collect();
    assert_eq!(paths, ["src", "tests"]);
}

/// The actual tool accepts a single path alias and a default whole-project scope.
#[test]
fn applied_ast_grep_accepts_path_aliases_and_defaults() {
    for (extra, target) in [
        (json!({"apply": true, "path": "src/a.rs"}), "src/a.rs"),
        (json!({"apply": true, "paths": "src/b.rs"}), "src/b.rs"),
        (json!({"apply": true}), "."),
        (json!({"apply": true, "paths": []}), "."),
    ] {
        let request = replacement(extra);
        assert!(
            request.checks.iter().any(|check| check.permission == "edit" && check.input == target),
            "{target}"
        );
    }
}

/// Adding edit policy must not discard an existing ast-grep tool-specific refusal.
#[test]
fn ast_grep_specific_denials_still_apply() {
    let engine = PermissionEngine::assemble(
        PermissionMode::Default,
        vec![rule("ast_grep", "replace", Deny)],
        vec![],
        false,
    );
    assert_eq!(engine.decide(&replacement(json!({"apply": true}))), Deny);
}

/// A bulk replacement's eventual files are unknown before scanning, so no action-wide Always rule is honest.
#[test]
fn applied_ast_grep_offers_no_action_wide_remembering_rule() {
    assert!(replacement(json!({"apply": true, "paths": ["src", "tests"]})).always.is_empty());
}
