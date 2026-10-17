use super::*;
use crate::permission::{
    PermissionAction::{Allow, Ask, Deny},
    PermissionEngine, PermissionMode,
};

fn launch(program: &str, arguments: &[&str]) -> PermissionRequest {
    build_request(
        "debug",
        &json!({"action": "launch", "program": program, "args": arguments}),
        &root(),
    )
}

/// A debugger must not bypass a Bash rule refusing a shell program.
#[test]
fn a_debug_shell_launch_obeys_the_bash_denial() {
    let engine = PermissionEngine::assemble(
        PermissionMode::Default,
        vec![rule("bash", "/bin/sh *", Deny)],
        vec![],
        false,
    );
    let request = launch("/bin/sh", &["-c", "echo ok"]);
    assert_eq!(engine.decide(&request), Deny);
    assert_eq!(request.tool, "debug");
    assert!(request
        .checks
        .iter()
        .any(|check| check.permission == "bash" && check.input == "/bin/sh -c echo ok"));
}

/// Launch arguments must participate in Bash pattern matching, not just the program name.
#[test]
fn debug_launch_arguments_are_checked_by_bash_rules() {
    let engine = PermissionEngine::assemble(
        PermissionMode::Default,
        vec![rule("bash", "app --dangerous *", Deny)],
        vec![],
        false,
    );
    assert_eq!(engine.decide(&launch("app", &["--dangerous", "value"])), Deny);
    assert_eq!(engine.decide(&launch("app", &["--safe"])), Allow);
}

/// A launch must not discard the existing debug-key policy when adding execution checks.
#[test]
fn debug_specific_launch_denials_still_apply() {
    let engine = PermissionEngine::assemble(
        PermissionMode::Default,
        vec![rule("debug", "launch", Deny)],
        vec![],
        false,
    );
    assert_eq!(engine.decide(&launch("app", &[])), Deny);
}

/// Inspecting or attaching to a session must not manufacture Bash execution checks.
#[test]
fn debug_non_launch_actions_keep_their_existing_checks() {
    for action in ["attach", "stack_trace", "variables", "output", "sessions"] {
        let request =
            build_request("debug", &json!({"action": action, "program": "/bin/sh"}), &root());
        assert!(!request.checks.iter().any(|check| check.permission == "bash"), "{action}");
        assert_eq!(PermissionEngine::permissive().decide(&request), Allow, "{action}");
    }
}

/// A missing program cannot be launched and should retain the tool's normal validation path.
#[test]
fn a_launch_without_a_program_does_not_add_bash_checks() {
    let request = build_request("debug", &json!({"action": "launch"}), &root());
    assert!(!request.checks.iter().any(|check| check.permission == "bash"));
}

/// A remembered launch relaxes exactly that launch, never the program's whole Bash family.
#[test]
fn a_debug_launch_is_remembered_exactly() {
    let mut engine = PermissionEngine::assemble(PermissionMode::Strict, vec![], vec![], false);
    let request = launch("app", &["--safe"]);
    assert_eq!(engine.decide(&request), Ask);
    assert!(request
        .always
        .iter()
        .any(|rule| rule.permission == "bash" && rule.pattern == "app --safe"));
    engine.approve(request.always.clone());
    assert_eq!(engine.decide(&request), Allow);
    assert_eq!(engine.decide(&launch("app", &["--other"])), Ask);
    assert_eq!(engine.decide(&launch("other-app", &[])), Ask);
}

/// The default recursive-delete guard applies to a launched program with no user rules at all.
#[test]
fn a_debug_launch_hits_the_default_rm_guard() {
    assert_eq!(PermissionEngine::permissive().decide(&launch("rm", &["-rf", "/"])), Deny);
}

/// A shell launched with `-c` runs its script, so the script is judged as a command of its own.
#[test]
fn a_debug_shell_script_is_judged_as_a_command() {
    for (program, arguments) in [
        ("/bin/sh", &["-c", "rm -rf ~"][..]),
        ("bash", &["-lc", "rm -rf /"][..]),
        ("/usr/bin/zsh", &["-c", ":(){ :|:& };:"][..]),
        ("/usr/bin/env", &["sh", "-c", "rm -rf /"][..]),
        ("env", &["FOO=1", "-i", "bash", "-c", "rm -rf ~"][..]),
        ("busybox", &["sh", "-c", "rm -rf /"][..]),
        ("C:\\Windows\\bash.exe", &["-c", "rm -rf /"][..]),
    ] {
        let request = launch(program, arguments);
        assert_eq!(PermissionEngine::permissive().decide(&request), Deny, "{program}");
        assert!(request.always.is_empty(), "{program}");
    }
    assert_eq!(PermissionEngine::permissive().decide(&launch("/bin/sh", &["-c", "ls"])), Allow);
}
