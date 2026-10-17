use super::*;
use crate::permission::{PermissionAction, PermissionEngine, PermissionMode};
use PermissionAction::{Allow, Ask, Deny};

const SEPARATORS: [&str; 6] = ["&&", "||", ";", "|", "\n", "&"];

fn bash_decision(engine: &PermissionEngine, command: &str) -> PermissionAction {
    engine.decide(&build_request("bash", &json!({"command": command}), &root()))
}

fn allow_git(fallback: PermissionAction) -> PermissionEngine {
    PermissionEngine::assemble(
        PermissionMode::Default,
        vec![rule("bash", "*", fallback), rule("bash", "git *", Allow)],
        vec![],
        false,
    )
}

/// A git allowance must not authorize a different command before or after any shell separator.
#[test]
fn an_allow_rule_does_not_cover_a_chain_it_only_starts() {
    let engine = allow_git(Ask);
    for separator in SEPARATORS {
        for (first, second) in
            [("git status", "curl example.invalid"), ("curl example.invalid", "git status")]
        {
            let command = format!("{first} {separator} {second}");
            assert_eq!(bash_decision(&engine, &command), Ask, "{command:?}");
        }
    }
}

/// A specific allowance for one segment must not hide the catch-all denial of another.
#[test]
fn an_unallowed_segment_is_denied_even_beside_an_allowed_one() {
    let engine = allow_git(Deny);
    for command in ["git status && curl example.invalid", "curl example.invalid && git status"] {
        assert_eq!(bash_decision(&engine, command), Deny, "{command}");
    }
}

/// A chain remains allowed when every segment independently has an allowance.
#[test]
fn a_chain_of_individually_allowed_commands_is_allowed() {
    let mut engine = allow_git(Ask);
    engine.approve([rule("bash", "cargo *", Allow)]);
    for separator in SEPARATORS {
        for (first, second) in [("git status", "cargo test"), ("cargo test", "git status")] {
            let command = format!("{first} {separator} {second}");
            assert_eq!(bash_decision(&engine, &command), Allow, "{command:?}");
        }
    }
}

/// An explicit ask on any segment survives a more specific allowance on a different segment.
#[test]
fn an_explicit_segment_ask_is_not_hidden_by_an_allowance() {
    let engine = PermissionEngine::assemble(
        PermissionMode::Default,
        vec![rule("bash", "curl *", Ask), rule("bash", "git status *", Allow)],
        vec![],
        false,
    );
    for command in ["git status && curl example.invalid", "curl example.invalid && git status"] {
        assert_eq!(bash_decision(&engine, command), Ask, "{command}");
    }
}

/// Per-segment checking must not make ordinary chains prompt under the permissive defaults.
#[test]
fn permissive_chains_remain_allowed() {
    let engine = PermissionEngine::permissive();
    for command in ["echo ok && cargo test", "cargo test && echo ok"] {
        assert_eq!(bash_decision(&engine, command), Allow, "{command}");
    }
}

/// The whole-line input remains first for display and guards, with a check for each chained command.
#[test]
fn bash_requests_keep_the_whole_line_and_check_each_segment() {
    let command = "git status && cargo test";
    let request = build_request("bash", &json!({"command": command}), &root());
    let inputs: Vec<_> = request.checks.iter().map(|check| check.input.as_str()).collect();
    assert_eq!(inputs, [command, "git status", "cargo test"]);
    assert!(request.checks.iter().all(|check| check.permission == "bash"));
}

/// A single command needs only its original check, not a duplicate tokenized check.
#[test]
fn a_single_segment_keeps_one_check() {
    let request = build_request("bash", &json!({"command": "git status"}), &root());
    assert_eq!(request.checks.len(), 1);
    assert_eq!(request.checks[0].input, "git status");
}

/// A chained command must not offer a remembering rule for just its first command family.
#[test]
fn always_for_a_chain_offers_no_rule() {
    for separator in SEPARATORS {
        for (first, second) in [("cd app", "cargo test"), ("cargo test", "cd app")] {
            let command = format!("{first} {separator} {second}");
            let request = build_request("bash", &json!({"command": command}), &root());
            assert!(request.always.is_empty(), "a chain offered an Always rule: {command:?}");
        }
    }
}

/// Single commands, redirects and a trailing background marker still offer their command family.
#[test]
fn a_single_segment_still_offers_a_command_family_rule() {
    for command in [
        "cargo test",
        "  cargo test  ",
        "cargo test &",
        "cargo test 2>&1",
        "cargo test &>log",
        "cargo test <&3",
    ] {
        let request = build_request("bash", &json!({"command": command}), &root());
        assert_eq!(request.always.len(), 1, "{command}");
        assert_eq!(request.always[0].permission, "bash", "{command}");
        assert_eq!(request.always[0].pattern, "cargo *", "{command}");
        assert_eq!(request.always[0].action, Allow, "{command}");
    }
}

/// Empty input and separator-only input have no command family worth approving.
#[test]
fn a_command_with_no_segments_offers_no_rule() {
    for command in ["", "  ", ";", "&&", "|", "&"] {
        let request = build_request("bash", &json!({"command": command}), &root());
        assert!(request.always.is_empty(), "an empty command offered a rule: {command:?}");
    }
}

/// Remembering cd must allow cd alone without letting a later curl ride along in strict mode.
#[test]
fn strict_plus_always_cd_does_not_allow_a_later_curl() {
    let mut engine = PermissionEngine::assemble(PermissionMode::Strict, vec![], vec![], false);
    let request = build_request("bash", &json!({"command": "cd app"}), &root());
    engine.approve(request.always);
    assert_eq!(bash_decision(&engine, "cd app"), Allow);
    for command in ["cd app && curl example.invalid", "curl example.invalid && cd app"] {
        assert_eq!(bash_decision(&engine, command), Ask, "{command}");
    }
}

/// Shell separators in another tool's input must not manufacture bash permission checks.
#[test]
fn non_bash_inputs_are_not_split_into_commands() {
    let request = build_request("web_search", &json!({"query": "git && curl"}), &root());
    assert_eq!(request.checks.len(), 1);
    assert_eq!(request.checks[0].permission, "web_search");
    assert_eq!(request.checks[0].input, "git && curl");
}
