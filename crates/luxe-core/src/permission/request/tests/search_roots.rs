use super::*;
use crate::permission::{
    PermissionAction::{Allow, Ask, Deny},
    PermissionEngine, PermissionMode,
};

fn grep_request(extra: serde_json::Value) -> PermissionRequest {
    let mut args = json!({"pattern": "needle"});
    args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    build_request("grep", &args, &root())
}

/// A grep scope outside the project must ask just like a read of that scope.
#[test]
fn grep_outside_the_project_asks() {
    let request = grep_request(json!({"path": "/etc"}));
    assert_eq!(PermissionEngine::permissive().decide(&request), Ask);
    assert!(request
        .checks
        .iter()
        .any(|check| check.permission == EXTERNAL_DIRECTORY && check.input == "/etc"));
    assert_eq!(request.checks[0].permission, "grep");
    assert_eq!(request.checks[0].input, "needle");
}

/// Enabling ignored files must not make a named env file readable through grep.
#[test]
fn grep_of_a_named_env_file_is_denied_even_with_all() {
    for path in [".env", "config/.env.local", "../outside/.env"] {
        let request = grep_request(json!({"path": path, "all": true}));
        assert_eq!(PermissionEngine::permissive().decide(&request), Deny, "{path}");
    }
    assert_eq!(
        PermissionEngine::permissive()
            .decide(&grep_request(json!({"path": ".env.example", "all": true}))),
        Allow
    );
}

/// An internal scope remains allowed, and missing or blank path follows grep's cwd default.
#[test]
fn grep_internal_and_default_roots_remain_allowed() {
    for (extra, expected) in [
        (json!({"path": "src"}), "src"),
        (json!({}), "."),
        (json!({"path": "  "}), "."),
        (json!({"path": " src "}), "src"),
    ] {
        let request = grep_request(extra);
        assert_eq!(PermissionEngine::permissive().decide(&request), Allow);
        assert!(
            request
                .checks
                .iter()
                .any(|check| check.permission == "read" && check.input == expected),
            "{expected}"
        );
    }
}

/// A pattern that resembles a path is not a scope and must not prompt for that directory.
#[test]
fn grep_patterns_are_not_treated_as_search_roots() {
    let request = build_request("grep", &json!({"pattern": "/etc/hosts", "path": "src"}), &root());
    assert!(!request.checks.iter().any(|check| check.permission == EXTERNAL_DIRECTORY));
    assert_eq!(PermissionEngine::permissive().decide(&request), Allow);
}

/// Adding read checks must retain existing grep-pattern denials and honor new read-path rules.
#[test]
fn grep_keeps_both_its_own_rules_and_read_scope_policy() {
    let request = grep_request(json!({"path": "private"}));
    for denial in [rule("grep", "needle", Deny), rule("read", "private", Deny)] {
        let engine =
            PermissionEngine::assemble(PermissionMode::Default, vec![denial], vec![], false);
        assert_eq!(engine.decide(&request), Deny);
    }
}

/// File discovery outside the project must ask even though its primary input is only a glob.
#[test]
fn glob_outside_the_project_asks_without_changing_its_pattern_check() {
    let request = build_request("glob", &json!({"pattern": "*.rs", "path": "/etc"}), &root());
    assert_eq!(PermissionEngine::permissive().decide(&request), Ask);
    assert_eq!(request.checks[0].permission, "glob");
    assert_eq!(request.checks[0].input, "*.rs");
    assert!(request.checks.iter().any(|check| check.permission == EXTERNAL_DIRECTORY));
    assert!(request.always.iter().all(|rule| rule.permission == EXTERNAL_DIRECTORY));
}

/// A glob selector that resembles an absolute path must not become a search root.
#[test]
fn glob_selectors_are_not_mistaken_for_roots() {
    let request = build_request("glob", &json!({"pattern": "/etc/*.rs", "path": "src"}), &root());
    assert!(!request.checks.iter().any(|check| check.permission == EXTERNAL_DIRECTORY));
    assert_eq!(PermissionEngine::permissive().decide(&request), Allow);
}

/// Missing, empty and whitespace glob scopes must match the real tool's cwd fallback.
#[test]
fn glob_default_and_trimmed_roots_are_read_checks() {
    for (path, expected) in
        [(None, "."), (Some(""), "."), (Some("  "), "."), (Some(" src "), "src")]
    {
        let mut args = json!({"pattern": "*.rs"});
        if let Some(path) = path {
            args["path"] = json!(path);
        }
        let request = build_request("glob", &args, &root());
        assert!(
            request
                .checks
                .iter()
                .any(|check| check.permission == "read" && check.input == expected),
            "{expected}"
        );
        assert_eq!(PermissionEngine::permissive().decide(&request), Allow);
    }
}

/// Scope-read denials must not replace the original glob-specific pattern policy.
#[test]
fn glob_keeps_both_pattern_and_scope_rules() {
    let request = build_request("glob", &json!({"pattern": "*.rs", "path": "private"}), &root());
    for denial in [rule("glob", "*.rs", Deny), rule("read", "private", Deny)] {
        let engine =
            PermissionEngine::assemble(PermissionMode::Default, vec![denial], vec![], false);
        assert_eq!(engine.decide(&request), Deny);
    }
}

/// An explicitly named secret scope stays denied even when ignored-file discovery is enabled.
#[test]
fn glob_of_a_named_env_scope_obeys_read_policy() {
    for (path, expected) in [(".env", Deny), ("config/.env.local", Deny), (".env.example", Allow)] {
        let request =
            build_request("glob", &json!({"pattern": "*", "path": path, "all": true}), &root());
        assert_eq!(PermissionEngine::permissive().decide(&request), expected, "{path}");
    }
}

/// Structural search and replace preview must not bypass an external-directory prompt.
#[test]
fn ast_grep_search_and_preview_outside_the_project_ask() {
    for action in ["search", "replace"] {
        let request = build_request(
            "ast_grep",
            &json!({"action": action, "paths": ["/etc"], "apply": false}),
            &root(),
        );
        assert_eq!(PermissionEngine::permissive().decide(&request), Ask, "{action}");
        assert!(!request.always.iter().any(|rule| rule.permission == "ast_grep"));
    }
}

/// Every structural scope must be checked, including an outside path after an internal one.
#[test]
fn ast_grep_roots_include_all_supplied_paths() {
    let request =
        build_request("ast_grep", &json!({"action": "search", "paths": ["src", "/etc"]}), &root());
    assert_eq!(PermissionEngine::permissive().decide(&request), Ask);
    let roots: Vec<_> = request
        .checks
        .iter()
        .filter(|check| check.permission == "read")
        .map(|check| check.input.as_str())
        .collect();
    assert_eq!(roots, ["src", "/etc"]);
}

/// The structural tool's actual aliases and empty-array default must not be dropped by extraction.
#[test]
fn ast_grep_search_roots_honor_aliases_and_defaults() {
    for (extra, expected) in [
        (json!({"path": "src/a.rs"}), "src/a.rs"),
        (json!({"paths": "src/b.rs"}), "src/b.rs"),
        (json!({}), "."),
        (json!({"paths": []}), "."),
    ] {
        let mut args = json!({"action": "search"});
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let request = build_request("ast_grep", &args, &root());
        assert!(
            request
                .checks
                .iter()
                .any(|check| check.permission == "read" && check.input == expected),
            "{expected}"
        );
    }
}

/// A secret file cannot be opened for structural search just because no edits are applied.
#[test]
fn ast_grep_named_env_searches_follow_read_policy() {
    for (path, expected) in [(".env", Deny), ("config/.env.local", Deny), (".env.example", Allow)] {
        let request =
            build_request("ast_grep", &json!({"action": "search", "path": path}), &root());
        assert_eq!(PermissionEngine::permissive().decide(&request), expected, "{path}");
    }
}

/// Internal structural reads remain available in strict/read-only modes without an apply flag.
#[test]
fn ast_grep_internal_searches_stay_nonmutating() {
    let request =
        build_request("ast_grep", &json!({"action": "search", "paths": ["src"]}), &root());
    for mode in [PermissionMode::Strict, PermissionMode::ReadOnly] {
        let engine = PermissionEngine::assemble(mode, vec![], vec![], false);
        assert_eq!(engine.decide(&request), Allow, "{mode:?}");
    }
    assert!(!request.checks.iter().any(|check| check.permission == "edit"));
}

/// File outlines and symbol bodies must not bypass external-directory prompts.
#[test]
fn discovery_file_scopes_outside_the_project_ask() {
    for tool in ["module_report", "read_symbol"] {
        let request =
            build_request(tool, &json!({"path": "/etc/hosts", "symbol": "main"}), &root());
        assert_eq!(PermissionEngine::permissive().decide(&request), Ask, "{tool}");
        assert_eq!(request.checks[0].permission, tool);
        assert!(request
            .checks
            .iter()
            .any(|check| check.permission == "read" && check.input == "/etc/hosts"));
        assert!(request.always.iter().all(|rule| rule.permission == EXTERNAL_DIRECTORY));
    }
}

/// Discovery tools read file content even when they return only an outline, so env files stay guarded.
#[test]
fn discovery_named_env_files_follow_read_policy() {
    for tool in ["module_report", "read_symbol"] {
        for (path, expected) in
            [(".env", Deny), ("config/.env.local", Deny), (".env.example", Allow)]
        {
            let request = build_request(tool, &json!({"path": path, "symbol": "name"}), &root());
            assert_eq!(PermissionEngine::permissive().decide(&request), expected, "{tool}: {path}");
        }
    }
}

/// The four LSP file aliases must be honored by file discovery in the same order as execution.
#[test]
fn discovery_scope_uses_the_actual_file_aliases() {
    for tool in ["module_report", "read_symbol"] {
        for key in ["path", "file", "file_path", "filePath"] {
            let request = build_request(tool, &json!({key: "src/a.rs", "symbol": "main"}), &root());
            assert!(
                request
                    .checks
                    .iter()
                    .any(|check| check.permission == "read" && check.input == "src/a.rs"),
                "{tool}: {key}"
            );
        }
    }
}

/// A missing or empty first file alias must not manufacture a whole-project scan scope.
#[test]
fn discovery_without_a_usable_file_keeps_tool_validation() {
    for tool in ["module_report", "read_symbol"] {
        for args in
            [json!({"symbol": "main"}), json!({"path": "", "file": "/etc/hosts", "symbol": "main"})]
        {
            let request = build_request(tool, &args, &root());
            assert!(
                !request
                    .checks
                    .iter()
                    .any(|check| check.permission == "read"
                        || check.permission == EXTERNAL_DIRECTORY),
                "{tool}"
            );
        }
    }
}

/// Internal discovery remains available in nonmutating modes while honoring both tool and read rules.
#[test]
fn discovery_keeps_nonmutating_modes_and_both_policy_keys() {
    for tool in ["module_report", "read_symbol"] {
        let request = build_request(tool, &json!({"path": "src/a.rs", "symbol": "main"}), &root());
        for mode in [PermissionMode::Strict, PermissionMode::ReadOnly] {
            let engine = PermissionEngine::assemble(mode, vec![], vec![], false);
            assert_eq!(engine.decide(&request), Allow, "{tool}: {mode:?}");
        }
        for denial in [rule(tool, "*", Deny), rule("read", "src/a.rs", Deny)] {
            let engine =
                PermissionEngine::assemble(PermissionMode::Default, vec![denial], vec![], false);
            assert_eq!(engine.decide(&request), Deny, "{tool}");
        }
    }
}

/// Navigation and diagnostics open their source file, so an external file must prompt.
#[test]
fn lsp_file_reads_outside_the_project_ask() {
    for tool in ["lsp_navigation", "lsp_diagnostics"] {
        let request =
            build_request(tool, &json!({"path": "/etc/hosts", "action": "hover"}), &root());
        assert_eq!(PermissionEngine::permissive().decide(&request), Ask, "{tool}");
        assert!(request
            .checks
            .iter()
            .any(|check| check.permission == "read" && check.input == "/etc/hosts"));
        assert!(request.always.iter().all(|rule| rule.permission == EXTERNAL_DIRECTORY));
    }
}

/// Every diagnostics batch entry must be checked, not just a separately supplied single file.
#[test]
fn lsp_diagnostics_uses_all_batch_paths_with_batch_precedence() {
    let request = build_request(
        "lsp_diagnostics",
        &json!({"path": "safe.rs", "paths": ["src/a.rs", "/etc/hosts"]}),
        &root(),
    );
    assert_eq!(PermissionEngine::permissive().decide(&request), Ask);
    let paths: Vec<_> = request
        .checks
        .iter()
        .filter(|check| check.permission == "read")
        .map(|check| check.input.as_str())
        .collect();
    assert_eq!(paths, ["src/a.rs", "/etc/hosts"]);
    let secret = build_request("lsp_diagnostics", &json!({"paths": ["src/a.rs", ".env"]}), &root());
    assert_eq!(PermissionEngine::permissive().decide(&secret), Deny);
}

/// LSP file aliases must not allow a named env file to bypass read policy.
#[test]
fn lsp_read_file_aliases_keep_env_protection() {
    for tool in ["lsp_navigation", "lsp_diagnostics"] {
        for key in ["path", "file", "file_path", "filePath"] {
            let request = build_request(tool, &json!({key: ".env", "action": "hover"}), &root());
            assert_eq!(PermissionEngine::permissive().decide(&request), Deny, "{tool}: {key}");
        }
    }
}

/// Empty diagnostics batches and missing files must not invent a directory scan or fall back to path.
#[test]
fn lsp_missing_file_and_empty_batches_keep_validation_behavior() {
    for (tool, args) in [
        ("lsp_navigation", json!({"action": "hover"})),
        ("lsp_diagnostics", json!({})),
        ("lsp_diagnostics", json!({"paths": [], "path": "/etc/hosts"})),
        ("lsp_navigation", json!({"path": "", "file": "/etc/hosts", "action": "hover"})),
    ] {
        let request = build_request(tool, &args, &root());
        assert!(
            !request
                .checks
                .iter()
                .any(|check| check.permission == "read" || check.permission == EXTERNAL_DIRECTORY),
            "{tool}"
        );
    }
}

/// Adding read scopes must leave internal navigation and diagnostics available in nonmutating modes.
#[test]
fn lsp_internal_read_actions_stay_nonmutating() {
    for tool in ["lsp_navigation", "lsp_diagnostics"] {
        let request = build_request(tool, &json!({"path": "src/a.rs", "action": "hover"}), &root());
        for mode in [PermissionMode::Strict, PermissionMode::ReadOnly] {
            let engine = PermissionEngine::assemble(mode, vec![], vec![], false);
            assert_eq!(engine.decide(&request), Allow, "{tool}: {mode:?}");
        }
        assert!(!request.checks.iter().any(|check| check.permission == "edit"));
    }
}

/// LSP-specific rules and explicit read-path refusals must both survive scope extraction.
#[test]
fn lsp_reads_keep_their_tool_policy_and_read_path_policy() {
    for tool in ["lsp_navigation", "lsp_diagnostics"] {
        let request = build_request(tool, &json!({"path": "src/a.rs", "action": "hover"}), &root());
        for denial in [rule(tool, "*", Deny), rule("read", "src/a.rs", Deny)] {
            let engine =
                PermissionEngine::assemble(PermissionMode::Default, vec![denial], vec![], false);
            assert_eq!(engine.decide(&request), Deny, "{tool}");
        }
    }
}

/// An exact secret filename selector must not hide behind a safe directory read scope.
#[test]
fn grep_exact_env_filters_are_denied_for_directory_scopes() {
    for path in [".", "src"] {
        for selector in [".env", ".env.local"] {
            let request = grep_request(json!({"path": path, "glob": selector, "all": true}));
            assert_eq!(PermissionEngine::permissive().decide(&request), Deny, "{path}: {selector}");
        }
    }
}

/// The template exception must remain allowed when selected from a directory.
#[test]
fn grep_exact_template_filters_keep_the_read_exception() {
    let request = grep_request(json!({"path": "src", "glob": ".env.example", "all": true}));
    assert!(request
        .checks
        .iter()
        .any(|check| check.permission == "read" && check.input == "src/.env.example"));
    assert_eq!(PermissionEngine::permissive().decide(&request), Allow);
}

/// A literal filename filter must participate in scoped read-path rules, not just env defaults.
#[test]
fn grep_exact_file_filters_obey_read_path_rules() {
    let request = grep_request(json!({"path": "src", "glob": "README.md"}));
    let engine = PermissionEngine::assemble(
        PermissionMode::Default,
        vec![rule("read", "src/README.md", Deny)],
        vec![],
        false,
    );
    assert_eq!(engine.decide(&request), Deny);
}

/// A filter matching an explicitly named file must not invent a child path inside that file.
#[test]
fn grep_exact_filters_do_not_append_to_a_matching_file_scope() {
    let request = grep_request(json!({"path": "src/README.md", "glob": "README.md"}));
    let engine = PermissionEngine::assemble(
        PermissionMode::Default,
        vec![rule("read", "*", Deny), rule("read", "src/README.md", Allow)],
        vec![],
        false,
    );
    assert_eq!(engine.decide(&request), Allow);
}

/// Wildcard and exclusion filters are not a single known file and must not be treated as one.
#[test]
fn grep_nonliteral_filters_do_not_invent_a_literal_target() {
    for selector in ["*.rs", "!README.md"] {
        let request = grep_request(json!({"path": "src", "glob": selector}));
        let paths: Vec<_> = request
            .checks
            .iter()
            .filter(|check| check.permission == "read")
            .map(|check| check.input.as_str())
            .collect();
        assert_eq!(paths, ["src"], "{selector}");
    }
}

/// Unrelated tools must not acquire filesystem checks merely because they carry a path field.
#[test]
fn unrelated_tools_do_not_gain_grep_root_checks() {
    let request = build_request("web_search", &json!({"query": "rust", "path": "/etc"}), &root());
    assert!(!request
        .checks
        .iter()
        .any(|check| check.permission == EXTERNAL_DIRECTORY || check.permission == "read"));
}

/// A pattern-wide approval cannot cover all files in a search scope, so it must not be offered.
#[test]
fn grep_search_scopes_offer_no_pattern_wide_always_rule() {
    for request in [grep_request(json!({"path": "/etc"})), grep_request(json!({"path": "src"}))] {
        assert!(request.always.iter().all(|rule| rule.permission == EXTERNAL_DIRECTORY));
    }
    assert!(grep_request(json!({"path": "src"})).always.is_empty());
}

/// One search outside the project can be remembered as that directory, like a read of it.
#[test]
fn a_single_external_search_root_can_be_remembered() {
    for tool in ["grep", "glob", "ast_grep", "read_symbol", "lsp_navigation"] {
        let args = json!({"pattern": "x", "path": "/srv/other", "action": "hover"});
        let request = build_request(tool, &args, &root());
        let mut engine = PermissionEngine::permissive();
        assert_eq!(engine.decide(&request), Ask, "{tool}");
        engine.approve(request.always.clone());
        assert_eq!(engine.decide(&request), Allow, "{tool}");
        let deeper = json!({"pattern": "x", "path": "/srv/other/sub", "action": "hover"});
        assert_eq!(engine.decide(&build_request(tool, &deeper, &root())), Allow, "{tool}");
        let sibling = json!({"pattern": "x", "path": "/srv/elsewhere", "action": "hover"});
        assert_eq!(engine.decide(&build_request(tool, &sibling, &root())), Ask, "{tool}");
    }
    let two = json!({"pattern": "x", "paths": ["/srv/a", "/srv/b"]});
    assert!(build_request("ast_grep", &two, &root()).always.is_empty());
}
