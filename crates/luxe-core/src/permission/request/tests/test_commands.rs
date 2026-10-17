use super::*;
use crate::permission::{
    PermissionAction::{Allow, Ask, Deny},
    PermissionEngine, PermissionMode,
};

fn explicit_request(command: &str) -> PermissionRequest {
    build_request("test", &json!({"command": command}), &root())
}

/// The test tool must not bypass recursive-delete guards by using a different tool name.
#[test]
fn an_explicit_test_command_cannot_wipe_the_root() {
    let request = explicit_request("rm -rf /");
    assert_eq!(PermissionEngine::permissive().decide(&request), Deny);
    assert!(request.checks.iter().any(|check| check.permission == "bash"));
    assert!(request.checks.iter().any(|check| check.permission == "catastrophic_rm"));
    assert_eq!(request.tool, "test");
}

/// Fork-bomb detection must apply before a test override reaches its shell.
#[test]
fn an_explicit_test_command_cannot_run_a_fork_bomb() {
    let request = explicit_request(":(){ :|:& };:");
    assert_eq!(PermissionEngine::permissive().decide(&request), Deny);
}

/// Dangerous Bash operations keep their prompt when routed through the test tool.
#[test]
fn an_explicit_test_force_push_still_asks() {
    let request = explicit_request("git push --force origin main");
    assert_eq!(PermissionEngine::permissive().decide(&request), Ask);
}

/// A git allowance must not authorize another segment in a test override.
#[test]
fn explicit_test_chains_need_each_segment_allowed() {
    let engine = PermissionEngine::assemble(
        PermissionMode::Default,
        vec![rule("bash", "*", Ask), rule("bash", "git *", Allow)],
        vec![],
        false,
    );
    for command in ["git status && curl example.invalid", "curl example.invalid && git status"] {
        assert_eq!(engine.decide(&explicit_request(command)), Ask, "{command}");
    }
    assert!(explicit_request("git status && cargo test").always.is_empty());
}

/// Explicit shell overrides must remain mutating in strict and read-only modes.
#[test]
fn explicit_test_commands_follow_the_mutating_modes() {
    let request = explicit_request("cargo test");
    for (mode, expected) in [(PermissionMode::Strict, Ask), (PermissionMode::ReadOnly, Deny)] {
        let engine = PermissionEngine::assemble(mode, vec![], vec![], false);
        assert_eq!(engine.decide(&request), expected, "{mode:?}");
    }
}

/// Adding Bash checks must not discard a user's existing test-tool denial.
#[test]
fn test_specific_denials_still_apply_to_explicit_commands() {
    let engine = PermissionEngine::assemble(
        PermissionMode::Default,
        vec![rule("test", "*", Deny)],
        vec![],
        false,
    );
    assert_eq!(engine.decide(&explicit_request("cargo test")), Deny);
}

/// Without a command override, filters are not shell commands and must not add Bash checks.
#[test]
fn autodetected_tests_keep_their_existing_permission_request() {
    let request = build_request("test", &json!({"filter": "unit"}), &root());
    assert_eq!(request.permission, "test");
    assert!(!request.checks.iter().any(|check| check.permission == "bash"));
    assert_eq!(PermissionEngine::permissive().decide(&request), Allow);
}

/// A remembered test command relaxes exactly that command, never its whole Bash family.
#[test]
fn a_single_test_command_is_remembered_exactly() {
    let mut engine = PermissionEngine::assemble(
        PermissionMode::Strict,
        vec![rule("test", "*", Ask)],
        vec![],
        false,
    );
    let request = explicit_request("cargo test");
    assert_eq!(engine.decide(&request), Ask);
    assert!(request
        .always
        .iter()
        .any(|rule| rule.permission == "bash" && rule.pattern == "cargo test"));
    assert!(!request.always.iter().any(|rule| rule.pattern.contains('*')), "{:?}", request.always);
    engine.approve(request.always.clone());
    assert_eq!(engine.decide(&request), Allow);
    assert_eq!(engine.decide(&explicit_request("cargo publish")), Ask);
    let bash = build_request("bash", &json!({"command": "cargo publish"}), &root());
    assert_eq!(engine.decide(&bash), Ask);
    assert_eq!(engine.decide(&explicit_request("cargo test && curl example.invalid")), Ask);
}

/// A filter is appended to the test command unquoted, so a second command hidden in it is judged.
#[test]
fn a_test_filter_cannot_smuggle_a_second_command() {
    for args in [
        json!({"command": "pytest", "filter": "unit; rm -rf /"}),
        json!({"filter": "unit; rm -rf /"}),
        json!({"filter": "unit && rm -rf ~"}),
        json!({"filter": "; rm -rf /"}),
        json!({"filter": "&& rm -rf ~"}),
        json!({"filter": "\nrm -rf /"}),
    ] {
        let request = build_request("test", &args, &root());
        assert_eq!(PermissionEngine::permissive().decide(&request), Deny, "{args}");
        assert!(request.always.is_empty(), "{args}");
    }
    let plain = build_request("test", &json!({"command": "pytest", "filter": "unit"}), &root());
    assert_eq!(PermissionEngine::permissive().decide(&plain), Allow);
    assert!(plain.checks.iter().any(|check| check.input == "pytest unit"));
}
