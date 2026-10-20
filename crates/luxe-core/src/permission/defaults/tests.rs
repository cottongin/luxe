use super::super::{build_request, evaluate, PermissionAction, PermissionEngine};
use super::*;
use std::path::Path;

mod catastrophic_rm;

/// Dropping a canonical mutator or its write alias must not let strict/read-only modes allow it.
#[test]
fn both_mutating_overlays_cover_the_canonical_keys_and_write_alias() {
    let root = Path::new("/home/dev/project");
    let calls = [
        ("edit", serde_json::json!({"path": "src/a.rs"})),
        ("write", serde_json::json!({"path": "src/a.rs"})),
        ("bash", serde_json::json!({"command": "echo ok"})),
        ("git", serde_json::json!({"action": "status"})),
        ("debug", serde_json::json!({"action": "launch"})),
    ];
    for (mode, expected) in [
        (PermissionMode::Strict, PermissionAction::Ask),
        (PermissionMode::ReadOnly, PermissionAction::Deny),
    ] {
        let engine = PermissionEngine::assemble(mode, vec![], vec![], false);
        for (tool, args) in &calls {
            assert_eq!(
                engine.decide(&build_request(tool, args, root)),
                expected,
                "{mode:?}: {tool}"
            );
        }
        let read = build_request("read", &serde_json::json!({"path": "src/a.rs"}), root);
        assert_eq!(engine.decide(&read), PermissionAction::Allow, "{mode:?}: read");
    }
}

#[test]
fn catastrophic_list_denies_root_and_system_wipes() {
    let engine = PermissionEngine::permissive();
    for cmd in [
        "rm -rf /",
        "rm -fr /",
        "rm -rf /etc",
        "rm -rf /usr/local",
        "rm -rf ~",
        "mkfs.ext4 /dev/sda1",
        "dd if=/dev/zero of=/dev/sda",
    ] {
        assert_eq!(
            engine.decide(&build_request(
                "bash",
                &serde_json::json!({"command": cmd}),
                Path::new("/home/dev/project"),
            )),
            PermissionAction::Deny,
            "should deny: {cmd}"
        );
    }
}

#[test]
fn ordinary_commands_and_relative_deletes_are_allowed() {
    let engine = PermissionEngine::permissive();
    for cmd in ["ls -la", "cargo build", "rm -rf target", "rm -rf ./build", "git status"] {
        assert_eq!(
            engine.decide(&build_request(
                "bash",
                &serde_json::json!({"command": cmd}),
                Path::new("/home/dev/project"),
            )),
            PermissionAction::Allow,
            "should allow: {cmd}"
        );
    }
}

#[test]
fn force_push_asks() {
    let rules = default_ruleset();
    assert_eq!(
        evaluate("bash", "git push --force origin main", &rules),
        Some(PermissionAction::Ask)
    );
}

/// Every route to root is refused, however the command is dressed; a word that only looks
/// like one is not.
#[test]
fn running_as_root_is_denied_and_lookalikes_are_not() {
    let rules = default_ruleset();
    for command in [
        "sudo apt install x",
        "sudo",
        "sudo -v",
        "cd /tmp && sudo make install",
        "doas pkg_add vim",
        "su -c 'id'",
        "su",
        "pkexec systemctl restart foo",
        "run0 pacman -Syu",
        "gsudo choco install git",
        "runas /user:Administrator cmd",
    ] {
        assert_eq!(
            evaluate("bash", command, &rules),
            Some(PermissionAction::Deny),
            "should deny: {command}"
        );
    }
    for command in
        ["sum file.txt", "suspend-check", "echo sudo", "git log --grep sudo", "grep -r sudo docs"]
    {
        assert_eq!(
            evaluate("bash", command, &rules),
            Some(PermissionAction::Allow),
            "should allow: {command}"
        );
    }
}

/// The person can still decide otherwise ahead of time: a global `allow` rule (passwordless
/// sudo is the case it serves), a narrower one, or `--yolo`.
#[test]
fn a_person_can_still_allow_running_as_root_ahead_of_time() {
    let root = Path::new("/home/dev/project");
    let install = build_request("bash", &serde_json::json!({"command": "sudo pacman -S jq"}), root);
    assert_eq!(PermissionEngine::permissive().decide(&install), PermissionAction::Deny);
    assert!(runs_as_root(&install));

    let broad =
        PermissionEngine::new(vec![rule("bash", "sudo *", PermissionAction::Allow)], vec![], false);
    assert_eq!(broad.decide(&install), PermissionAction::Allow, "a global rule wins the tie");

    let narrow = PermissionEngine::new(
        vec![rule("bash", "sudo pacman *", PermissionAction::Allow)],
        vec![],
        false,
    );
    assert_eq!(narrow.decide(&install), PermissionAction::Allow, "a narrower rule wins");
    let other = build_request("bash", &serde_json::json!({"command": "sudo rm -rf /opt/x"}), root);
    assert_eq!(narrow.decide(&other), PermissionAction::Deny, "and covers only itself");

    let yolo = PermissionEngine::assemble(PermissionMode::Yolo, vec![], vec![], false);
    assert_eq!(yolo.decide(&install), PermissionAction::Allow, "--yolo already means this");

    let project_only = PermissionEngine::assemble(
        PermissionMode::Default,
        vec![],
        vec![rule("bash", "sudo *", PermissionAction::Allow)],
        false,
    );
    assert_eq!(
        project_only.decide(&install),
        PermissionAction::Deny,
        "a repository cannot grant itself root"
    );
    assert!(!runs_as_root(&build_request("bash", &serde_json::json!({"command": "ls"}), root)));
}

/// Reading one file outside the project root already prompts, so photographing the whole
/// screen — which is not the project, may not be the reader's to share, and travels to a
/// provider inline — cannot be the one thing that does not.
#[test]
fn photographing_the_screen_asks_and_rendering_a_page_does_not() {
    let rules = default_ruleset();
    for mode in ["fullscreen", "region", "window"] {
        assert_eq!(
            evaluate("screenshot", mode, &rules),
            Some(PermissionAction::Ask),
            "mode={mode} photographs the desktop"
        );
    }
    assert_eq!(
        evaluate("screenshot", "http://localhost:3000", &rules),
        Some(PermissionAction::Allow),
        "a page luxe was asked to render is the mode visual work wants, and stays frictionless"
    );
}

/// A commit prompts on every route to it, because the model has already used the second one.
#[test]
fn committing_asks_however_it_is_spelled() {
    let rules = default_ruleset();
    for (key, input) in [
        ("git", "commit"),
        ("git", "commit --amend --no-edit"),
        ("bash", "git commit -m 'subject'"),
        // The literal shape of the fallback taken when the `git` tool refused.
        ("bash", "cd /home/dev/project && git add -A && git commit -m \"subject\""),
    ] {
        assert_eq!(
            evaluate(key, input, &rules),
            Some(PermissionAction::Ask),
            "should ask: {key} {input}"
        );
    }
}

/// Every public GitHub write prompts, whichever route it takes — including a `gh` write at the
/// end of a chain, which is where a model puts it after the `git` tool asked.
#[test]
fn github_writes_ask_on_both_routes() {
    let rules = default_ruleset();
    for (key, input) in [
        ("git", "pr create --title t --body b"),
        ("git", "pr merge 3 --squash"),
        ("git", "pr close 3"),
        ("git", "pr reopen 3"),
        ("git", "pr comment 3 --body hi"),
        ("git", "pr review 3 --approve"),
        ("git", "pr edit 3 --title t"),
        ("git", "pr ready 3"),
        ("bash", "gh pr create --fill"),
        ("bash", "gh pr merge 3 --squash"),
        ("bash", "gh pr merge"),
        ("bash", "gh pr close 3"),
        ("bash", "gh pr reopen 3"),
        ("bash", "gh pr comment 3 --body hi"),
        ("bash", "gh pr review 3 --request-changes -b no"),
        ("bash", "gh pr edit 3 --add-label bug"),
        ("bash", "gh pr ready 3"),
        ("bash", "gh issue create --title t --body b"),
        ("bash", "gh issue comment 9 --body hi"),
        ("bash", "gh issue close 9"),
        ("bash", "gh issue reopen 9"),
        ("bash", "gh issue edit 9 --add-label bug"),
        ("bash", "gh run rerun 123 --failed"),
        ("bash", "gh workflow run ci.yml"),
        ("bash", "cd /home/dev/project && git push && gh pr merge 3 --auto"),
    ] {
        assert_eq!(
            evaluate(key, input, &rules),
            Some(PermissionAction::Ask),
            "should ask: {key} {input}"
        );
    }
}

/// Looking at GitHub is not writing to it, and a prompt on every `pr view` is a prompt people
/// learn to dismiss without reading.
#[test]
fn reading_github_is_still_allowed() {
    let rules = default_ruleset();
    for (key, input) in [
        ("git", "pr"),
        ("git", "pr list --state open"),
        ("git", "pr view 3 --comments"),
        ("git", "pr diff 3"),
        ("git", "pr checks 3"),
        ("git", "pr status"),
        ("bash", "gh pr view 3 --json title,body"),
        ("bash", "gh pr list --search review-requested:@me"),
        ("bash", "gh pr checkout 3"),
        ("bash", "gh issue view 9 --comments"),
        ("bash", "gh issue list"),
        ("bash", "gh run view 123 --log-failed"),
        ("bash", "gh repo view --json nameWithOwner"),
    ] {
        assert_eq!(
            evaluate(key, input, &rules),
            Some(PermissionAction::Allow),
            "should allow: {key} {input}"
        );
    }
}

/// Driven through [`build_request`] and the engine, because a rule that matches the pattern
/// but not the input the `git` tool actually produces would pass [`evaluate`] and fire on
/// nobody.
#[test]
fn a_real_pr_merge_call_asks() {
    let root = Path::new("/home/dev/project");
    let merge = build_request(
        "git",
        &serde_json::json!({"action": "pr", "args": ["merge", "3", "--squash"]}),
        root,
    );
    assert_eq!(PermissionEngine::permissive().decide(&merge), PermissionAction::Ask);
    let view =
        build_request("git", &serde_json::json!({"action": "pr", "args": ["view", "3"]}), root);
    assert_eq!(PermissionEngine::permissive().decide(&view), PermissionAction::Allow);
}

/// Merging, closing and submitting a review need a person; opening a PR and commenting do not,
/// because the loop legitimately does both.
#[test]
fn merging_closing_and_reviewing_need_a_person() {
    let root = Path::new("/home/dev/project");
    let git = |args: &[&str]| {
        build_request("git", &serde_json::json!({"action": "pr", "args": args}), root)
    };
    let bash =
        |command: &str| build_request("bash", &serde_json::json!({"command": command}), root);

    for (request, act) in [
        (git(&["merge", "3"]), "Merging a pull request"),
        (git(&["close", "3"]), "Closing a pull request"),
        (git(&["review", "3", "--approve"]), "Submitting a pull request review"),
        (bash("gh pr merge 3 --squash"), "Merging a pull request"),
        (bash("gh pr close 3"), "Closing a pull request"),
        (bash("gh pr review 3 --approve"), "Submitting a pull request review"),
        (bash("gh issue close 9"), "Closing an issue"),
        (bash("git push && gh pr merge 3 --auto"), "Merging a pull request"),
        (bash("gh issue close 9 && gh pr merge 3"), "Merging a pull request"),
        (bash("gh pr merge 3 && gh issue close 9"), "Merging a pull request"),
    ] {
        assert_eq!(needs_a_person(&request), Some(act), "{:?}", request.checks);
    }

    for request in [
        git(&["create", "--title", "t", "--body", "b"]),
        git(&["comment", "3", "--body", "hi"]),
        git(&["view", "3"]),
        bash("gh pr create --fill"),
        bash("gh issue comment 9 --body hi"),
        bash("gh pr view 3"),
        bash("echo gh pr merge 3"),
    ] {
        assert_eq!(needs_a_person(&request), None, "{:?}", request.checks);
    }
}

/// Reading history is not writing it. A guard that fires on `log` teaches people to dismiss it.
#[test]
fn reading_the_repository_is_still_allowed() {
    let rules = default_ruleset();
    for (key, input) in [
        ("git", "status"),
        ("git", "log --format=%s"),
        ("git", "diff --stat"),
        ("git", "add ."),
        ("bash", "git log --grep commit"),
        ("bash", "git show HEAD"),
    ] {
        assert_eq!(
            evaluate(key, input, &rules),
            Some(PermissionAction::Allow),
            "should allow: {key} {input}"
        );
    }
}

/// Turning the prompt off is one config line, and `--yolo` already means it.
///
/// Driven through [`build_request`] and the engine rather than [`evaluate`], because that is
/// the path a real call takes — a rule that matched the pattern but not the input the tool
/// actually produces would pass the cheaper test and fire on nobody.
#[test]
fn a_real_commit_call_asks_and_one_rule_retires_the_prompt() {
    let root = Path::new("/home/dev/project");
    let commit = build_request("git", &serde_json::json!({"action": "commit"}), root);
    assert_eq!(PermissionEngine::permissive().decide(&commit), PermissionAction::Ask);

    let allowed =
        PermissionEngine::new(vec![rule("git", "commit*", PermissionAction::Allow)], vec![], false);
    assert_eq!(allowed.decide(&commit), PermissionAction::Allow, "one config line retires it");

    let yolo = PermissionEngine::assemble(PermissionMode::Yolo, vec![], vec![], false);
    assert_eq!(yolo.decide(&commit), PermissionAction::Allow, "--yolo already means this");
}

/// Approving a commit once has to cover the next one, or "always" means "once more".
#[test]
fn approving_a_commit_covers_the_commits_after_it() {
    let root = Path::new("/home/dev/project");
    let mut engine = PermissionEngine::permissive();
    let first = build_request("git", &serde_json::json!({"action": "commit"}), root);
    engine.approve(first.always.clone());

    let amended = build_request(
        "git",
        &serde_json::json!({"action": "commit", "args": ["--amend", "--no-edit"]}),
        root,
    );
    assert_eq!(engine.decide(&amended), PermissionAction::Allow, "{:?}", first.always);
    // Approving commits is not approving everything the tool can do.
    let force = build_request(
        "bash",
        &serde_json::json!({"command": "git push --force origin main"}),
        root,
    );
    assert_eq!(engine.decide(&force), PermissionAction::Ask);
}

#[test]
fn readonly_overlay_denies_mutators_via_evaluate() {
    let mut rules = default_ruleset();
    rules.extend(readonly_overlay());
    assert_eq!(evaluate("edit", "src/x.rs", &rules), Some(PermissionAction::Deny));
    assert_eq!(
        evaluate(super::super::permission_key("write"), "src/x.rs", &rules),
        Some(PermissionAction::Deny)
    );
    assert_eq!(evaluate("read", "src/x.rs", &rules), Some(PermissionAction::Allow));
}
