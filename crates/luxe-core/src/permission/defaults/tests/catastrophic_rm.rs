use super::*;
use crate::permission::{
    PermissionAction::{Allow, Deny},
    PermissionRequest,
};

fn deletion_request(command: &str) -> PermissionRequest {
    build_request("bash", &serde_json::json!({"command": command}), Path::new("/home/dev/project"))
}

fn deletion_decision(command: &str, mode: PermissionMode) -> PermissionAction {
    PermissionEngine::assemble(mode, vec![], vec![], false).decide(&deletion_request(command))
}

/// Literal glob targets and home spellings must not slip past the old string-pattern guard.
#[test]
fn recursive_deletion_of_root_or_home_is_denied_in_either_command_order() {
    for target in [
        "/",
        "/*",
        "~",
        "~/",
        "~/*",
        "$HOME",
        "${HOME}",
        "$HOME/",
        "${HOME}/",
        "$HOME/*",
        "${HOME}/*",
        "\"$HOME\"",
        "\"${HOME}\"",
        "\"~/\"",
    ] {
        let deletion = format!("rm -rf {target}");
        for command in
            [deletion.clone(), format!("echo ok && {deletion}"), format!("{deletion} && echo ok")]
        {
            assert_eq!(deletion_decision(&command, PermissionMode::Default), Deny, "{command}");
        }
    }
}

/// Long, combined and separate recursive flags must protect the root even without force.
#[test]
fn recursive_flag_spellings_and_end_of_options_do_not_bypass_the_guard() {
    for flags in [
        "-r",
        "-R",
        "-rf",
        "-fr",
        "-r -f",
        "-f -r",
        "--recursive --force",
        "--force --recursive",
        "-r -f --",
        "--recursive --force --",
    ] {
        let deletion = format!("rm {flags} /");
        for command in
            [deletion.clone(), format!("echo ok && {deletion}"), format!("{deletion} && echo ok")]
        {
            assert_eq!(deletion_decision(&command, PermissionMode::Default), Deny, "{command}");
        }
    }
    assert_eq!(deletion_decision("rm / --recursive --force", PermissionMode::Default), Deny);
}

/// Protected system directories and their contents remain guarded under the long flag spelling.
#[test]
fn protected_system_roots_and_their_contents_are_denied() {
    for root in PROTECTED_ROOTS.iter().filter(|root| **root != "/") {
        for target in
            [root.to_string(), format!("{root}/"), format!("{root}/*"), format!("{root}/cache")]
        {
            let deletion = format!("rm --recursive --force {target}");
            for command in [format!("echo ok && {deletion}"), format!("{deletion} && echo ok")] {
                assert_eq!(deletion_decision(&command, PermissionMode::Default), Deny, "{command}");
            }
        }
    }
}

/// A dedicated guard must not be overridden by a Bash allowance or session approval.
#[test]
fn bash_allowances_cannot_approve_a_catastrophic_deletion() {
    let mut engine = PermissionEngine::assemble(
        PermissionMode::Default,
        vec![rule("bash", "*", Allow)],
        vec![],
        false,
    );
    engine.approve([rule("bash", "rm *", Allow)]);
    let request = deletion_request("rm -rf /*");
    assert!(request.checks.iter().any(|check| check.permission == "catastrophic_rm"));
    assert_eq!(engine.decide(&request), Deny);
}

/// Protected-looking targets without recursive flags, and ordinary build cleanups, stay allowed.
#[test]
fn ordinary_deletions_and_non_deletion_heads_stay_allowed() {
    for command in [
        "rm -rf target",
        "rm -r ./build",
        "rm --recursive --force /tmp/luxe-build",
        "rm -rf ~/project/target",
        "rm -rf $HOME/project/target",
        "rm -rf ${HOME}/project/target",
        "rm -f /",
        "rm --force /",
        "rm -- -r /",
        "rm -rf -- -r",
        "rm --recursive -- --no-preserve-root",
        "echo rm -rf /*",
        "git rm -rf /",
    ] {
        let request = deletion_request(command);
        assert!(
            !request.checks.iter().any(|check| check.permission == "catastrophic_rm"),
            "{command}"
        );
        assert_eq!(deletion_decision(command, PermissionMode::Default), Allow, "{command}");
    }
}

/// A protected-root prefix must not turn a neighboring directory into a new hard-deny target.
#[test]
fn root_lookalike_paths_do_not_trigger_the_literal_guard() {
    for target in ["/etc-backup", "/usr-src", "/var-build", "/lib64-backup"] {
        let command = format!("rm -rf {target}");
        let request = deletion_request(&command);
        assert!(
            !request.checks.iter().any(|check| check.permission == "catastrophic_rm"),
            "{command}"
        );
    }
}

/// Legacy prefix globs must not refuse safe directories that merely resemble protected roots.
#[test]
fn root_lookalike_deletions_stay_allowed() {
    for target in ["/etc-backup", "/usr-src", "/var-build", "/lib64-backup"] {
        let deletion = format!("rm -rf {target}");
        for command in
            [deletion.clone(), format!("echo ok && {deletion}"), format!("{deletion} && echo ok")]
        {
            assert_eq!(deletion_decision(&command, PermissionMode::Default), Allow, "{command}");
        }
    }
}

/// Yolo remains the explicit opt-out from default guards; strict mode retains the hard denial.
#[test]
fn the_rm_guard_is_kept_in_strict_mode_but_disabled_by_yolo() {
    assert_eq!(deletion_decision("rm --recursive --force /*", PermissionMode::Strict), Deny);
    assert_eq!(deletion_decision("rm --recursive --force /*", PermissionMode::Yolo), Allow);
}

/// Disabling root preservation must be caught even when options follow an ordinary target.
#[test]
fn no_preserve_root_is_guarded_when_recursive() {
    for command in ["rm --recursive --no-preserve-root target", "rm -rf target --no-preserve-root"]
    {
        assert_eq!(deletion_decision(command, PermissionMode::Default), Deny, "{command}");
    }
}
