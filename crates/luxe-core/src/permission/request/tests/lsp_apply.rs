use super::*;
use crate::permission::{
    PermissionAction::{Allow, Ask, Deny},
    PermissionEngine, PermissionMode,
};

fn navigation(action: &str, extra: serde_json::Value) -> PermissionRequest {
    let mut args = json!({"action": action, "path": "src/a.rs"});
    args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    build_request("lsp_navigation", &args, &root())
}

/// Applying either LSP write action must deny in read-only and ask in strict mode.
#[test]
fn applied_lsp_write_actions_follow_the_edit_modes() {
    for action in ["rename", "code_action"] {
        let request = navigation(action, json!({"apply": true}));
        for (mode, expected) in [(PermissionMode::ReadOnly, Deny), (PermissionMode::Strict, Ask)] {
            let engine = PermissionEngine::assemble(mode, vec![], vec![], false);
            assert_eq!(engine.decide(&request), expected, "{mode:?}: {action}");
        }
        assert!(request
            .checks
            .iter()
            .any(|check| check.permission == "edit" && check.input == "src/a.rs"));
        assert_eq!(request.tool, "lsp_navigation");
    }
}

/// Previewing a rename or code action must not remove the tool's nonmutating modes.
#[test]
fn lsp_write_previews_are_not_edit_checks() {
    for action in ["rename", "code_action"] {
        for extra in [json!({}), json!({"apply": false}), json!({"apply": "true"})] {
            let request = navigation(action, extra);
            assert!(!request.checks.iter().any(|check| check.permission == "edit"));
            let engine =
                PermissionEngine::assemble(PermissionMode::ReadOnly, vec![], vec![], false);
            assert_eq!(engine.decide(&request), Allow, "{action}");
        }
    }
}

/// Read-only actions ignore apply; a stray flag must not become a blanket LSP deny.
#[test]
fn lsp_read_actions_do_not_acquire_edit_checks() {
    for action in
        ["go_to_definition", "hover", "find_references", "document_symbols", "workspace_symbol"]
    {
        let request = navigation(action, json!({"apply": true}));
        assert!(!request.checks.iter().any(|check| check.permission == "edit"), "{action}");
    }
}

/// The edit target must honor the same ordered file aliases the LSP tool accepts.
#[test]
fn lsp_apply_uses_the_actual_file_aliases() {
    for key in ["path", "file", "file_path", "filePath"] {
        let args = json!({"action": "rename", "apply": true, key: "src/b.rs"});
        let request = build_request("lsp_navigation", &args, &root());
        assert!(
            request
                .checks
                .iter()
                .any(|check| check.permission == "edit" && check.input == "src/b.rs"),
            "{key}"
        );
    }
}

/// User edit rules must target the path, while original LSP-specific denials still hold.
#[test]
fn applied_lsp_changes_keep_both_tool_and_path_policy() {
    let request = navigation("rename", json!({"apply": true}));
    for denial in [rule("edit", "src/a.rs", Deny), rule("lsp_navigation", "rename", Deny)] {
        let engine =
            PermissionEngine::assemble(PermissionMode::Default, vec![denial], vec![], false);
        assert_eq!(engine.decide(&request), Deny);
    }
}

/// The server's eventual target files are unknown before the call, so no action-wide Always rule is offered.
#[test]
fn applied_lsp_changes_offer_no_action_wide_remembering_rule() {
    for action in ["rename", "code_action"] {
        assert!(navigation(action, json!({"apply": true})).always.is_empty());
    }
}

/// Missing or empty file input cannot execute an LSP action and retains normal tool validation.
#[test]
fn lsp_apply_without_a_usable_file_does_not_add_an_edit_check() {
    for args in [
        json!({"action": "rename", "apply": true}),
        json!({"action": "rename", "apply": true, "path": "", "file": "src/a.rs"}),
    ] {
        let request = build_request("lsp_navigation", &args, &root());
        assert!(!request.checks.iter().any(|check| check.permission == "edit"));
    }
}
