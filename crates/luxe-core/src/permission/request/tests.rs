use super::*;
use serde_json::json;

mod ast_grep_apply;
mod bash_segments;
mod debug_launch;
mod lsp_apply;
mod search_roots;
mod test_commands;

fn root() -> PathBuf {
    PathBuf::from("/home/dev/project")
}

#[test]
fn bash_request_has_command_check_and_family_always() {
    let req = build_request("bash", &json!({"command": "cargo build --release"}), &root());
    assert_eq!(req.permission, "bash");
    assert_eq!(req.checks[0].input, "cargo build --release");
    assert!(req.always.iter().any(|r| r.pattern == "cargo *"));
}

/// Every file a multi-path read names is judged, so a secret anywhere in the list is denied
/// and a file outside the project anywhere in it asks — the same answers as reading it alone.
#[test]
fn a_read_of_several_files_is_judged_file_by_file() {
    let engine = crate::permission::PermissionEngine::new(
        crate::permission::defaults::default_ruleset(),
        Default::default(),
        false,
    );
    let decide = |args: serde_json::Value| engine.decide(&build_request("read", &args, &root()));
    assert_eq!(
        decide(json!({"paths": ["src/a.rs", "src/b.rs"]})),
        crate::permission::PermissionAction::Allow
    );
    assert_eq!(
        decide(json!({"paths": ["src/a.rs", ".env"]})),
        crate::permission::PermissionAction::Deny,
        "a secret second in the list"
    );
    assert_eq!(
        decide(json!({"path": "src/a.rs", "paths": ["config/.env"]})),
        crate::permission::PermissionAction::Deny,
        "or beside a path"
    );
    assert_eq!(
        decide(json!({"paths": ["src/a.rs", "/etc/hosts"]})),
        crate::permission::PermissionAction::Ask,
        "outside the project, wherever it sits"
    );
    let several = build_request("read", &json!({"paths": ["a.rs", "b.rs", "c.rs"]}), &root());
    assert_eq!(several.summary, "Read a.rs and 2 more");
    assert!(several.always.is_empty(), "no single rule covers them all");
}

#[test]
fn write_maps_to_edit_key() {
    let req = build_request("write", &json!({"path": "src/x.rs"}), &root());
    assert_eq!(req.permission, "edit");
}

#[test]
fn external_path_adds_the_guard_check() {
    let inside = build_request("edit", &json!({"path": "src/x.rs"}), &root());
    assert!(!inside.checks.iter().any(|c| c.permission == EXTERNAL_DIRECTORY));

    let outside = build_request("edit", &json!({"path": "/etc/hosts"}), &root());
    assert!(outside.checks.iter().any(|c| c.permission == EXTERNAL_DIRECTORY));

    let escape = build_request("read", &json!({"path": "../../../etc/passwd"}), &root());
    assert!(escape.checks.iter().any(|c| c.permission == EXTERNAL_DIRECTORY));
}

#[test]
fn an_external_path_offers_directory_scoped_always_rules() {
    // Approving one file in someone else's directory must cover the rest of it — and must
    // cover the guard that actually fired, not just the tool's own key.
    let request = build_request("read", &json!({"path": "/home/dev/notes/one.md"}), &root());
    let guard = request
        .always
        .iter()
        .find(|rule| rule.permission == EXTERNAL_DIRECTORY)
        .expect("an external read offers an external_directory rule");
    assert_eq!(guard.pattern, "/home/dev/notes/*");
    assert!(
        request
            .always
            .iter()
            .any(|rule| rule.permission == "read" && rule.pattern == "/home/dev/notes/*"),
        "the tool key is generalized to the same directory: {:?}",
        request.always
    );

    // Approving it allows the next file in that directory without another prompt.
    let mut engine = super::super::PermissionEngine::permissive();
    engine.approve(request.always.clone());
    let next = build_request("read", &json!({"path": "/home/dev/notes/two.md"}), &root());
    assert_eq!(engine.decide(&next), super::super::PermissionAction::Allow);
    // A different external directory still asks.
    let elsewhere = build_request("read", &json!({"path": "/etc/hosts"}), &root());
    assert_eq!(engine.decide(&elsewhere), super::super::PermissionAction::Ask);
}

#[test]
fn a_relative_escape_scopes_both_the_written_and_resolved_directory() {
    let request = build_request("edit", &json!({"path": "../sibling/src/x.rs"}), &root());
    let patterns = request.always.iter().map(|rule| rule.pattern.as_str()).collect::<Vec<_>>();
    assert!(patterns.contains(&"/home/dev/sibling/src/*"), "resolved dir: {patterns:?}");
    assert!(patterns.contains(&"../sibling/src/*"), "written dir: {patterns:?}");
}

#[test]
fn an_in_project_path_still_offers_only_that_file() {
    let request = build_request("edit", &json!({"path": "src/x.rs"}), &root());
    assert_eq!(request.always.len(), 1);
    assert_eq!(request.always[0].pattern, "src/x.rs");
}

/// The ledger of work done outside the project and the guard that prompts about it must agree
/// on what "outside" means, so they share this one answer rather than each having their own.
#[test]
fn the_external_target_matches_the_guard_that_fires_on_it() {
    for (tool, path) in
        [("edit", "/etc/hosts"), ("write", "../sibling/x.rs"), ("read", "~nope/../../etc/x")]
    {
        let args = json!({ "path": path });
        let fired = build_request(tool, &args, &root())
            .checks
            .iter()
            .any(|check| check.permission == EXTERNAL_DIRECTORY);
        assert_eq!(
            fired,
            external_target(tool, &args, &root()).is_some(),
            "{tool} {path}: the guard and the ledger disagree"
        );
    }

    // Resolved, not as written: a ledger keyed on the written path would record the same file
    // twice under two spellings.
    assert_eq!(
        external_target("edit", &json!({"path": "../sibling/src/x.rs"}), &root()),
        Some(PathBuf::from("/home/dev/sibling/src/x.rs"))
    );
    assert_eq!(external_target("edit", &json!({"path": "src/x.rs"}), &root()), None);
    // Not a path tool: a grep pattern that looks like one is not a file anybody touched.
    assert_eq!(external_target("grep", &json!({"pattern": "/etc/hosts"}), &root()), None);
}

/// Reading a file outside the project is worth a prompt but is not a change, so only the
/// mutating tools reach the ledger.
#[test]
fn only_mutating_tools_count_as_work() {
    for tool in ["write", "edit", "patch", "apply_patch"] {
        assert!(is_mutating_path_tool(tool), "{tool} changes the file it names");
    }
    for tool in ["read", "grep", "glob", "bash"] {
        assert!(!is_mutating_path_tool(tool), "{tool} does not");
    }
}

#[test]
fn fork_bomb_adds_the_guard_check() {
    let req = build_request("bash", &json!({"command": ":(){ :|:& };:"}), &root());
    assert!(req.checks.iter().any(|c| c.permission == FORKBOMB));
    let normal = build_request("bash", &json!({"command": "echo hi"}), &root());
    assert!(!normal.checks.iter().any(|c| c.permission == FORKBOMB));
}

/// The guard is three independent patterns, and the canonical bomb satisfies all three at
/// once — so testing only that one proves nothing about any of them. Each spelling here trips
/// a different `contains`.
#[test]
fn each_fork_bomb_spelling_trips_the_guard_on_its_own() {
    for command in [
        ":(){ :|:& };:",         // the canonical form
        ": ( ) { : | : & } ; :", // spaced out; whitespace is stripped before matching
        "bomb(){ :|:& };bomb",   // renamed function, same body
        "x() { :|:& }",          // the body alone, no invocation yet
    ] {
        let request = build_request("bash", &json!({ "command": command }), &root());
        assert!(
            request.checks.iter().any(|check| check.permission == FORKBOMB),
            "not recognized as a fork bomb: {command:?}"
        );
    }
}

/// Ordinary shell that happens to contain braces, pipes or backgrounding must not trip it — a
/// guardrail that cries wolf is one people learn to approve without reading.
#[test]
fn ordinary_commands_do_not_trip_the_fork_bomb_guard() {
    for command in [
        "echo hi",
        "cargo test --workspace",
        "find . -name '*.rs' | xargs wc -l",
        "sleep 10 &",
        "awk '{ print $1 }' file",
        "git log --format='%h %s'",
    ] {
        let request = build_request("bash", &json!({ "command": command }), &root());
        assert!(
            !request.checks.iter().any(|check| check.permission == FORKBOMB),
            "false positive on ordinary shell: {command:?}"
        );
    }
}

/// The guard is bash-only: another tool carrying the same text is not running it as shell.
#[test]
fn the_fork_bomb_guard_is_scoped_to_bash() {
    let written = build_request("write", &json!({"path": ":(){ :|:& };:"}), &root());
    assert!(!written.checks.iter().any(|check| check.permission == FORKBOMB));
}

#[test]
fn mcp_call_is_keyed_by_its_server_tool_target() {
    // A call carries a target + args → gated by `server/name`.
    let call = build_request("mcp", &json!({"tool": "github/create_issue", "args": "{}"}), &root());
    assert_eq!(call.checks[0].permission, "mcp");
    assert_eq!(call.checks[0].input, "github/create_issue");
    // Discovery (no args) has no target, so it isn't gated by a specific tool.
    let search = build_request("mcp", &json!({"search": "issue"}), &root());
    assert_eq!(search.checks[0].input, "");
}

/// The summary is the consent surface: it is the whole of what a person reads before deciding.
/// Nothing asserted on it, so it could have returned an empty string and every test still
/// passed — which is an approval prompt that says nothing.
#[test]
fn the_prompt_says_what_is_about_to_happen() {
    // Whole strings, not `contains`. Every arm's fallback also embeds the input, so a
    // containment check still passes when a guard is inverted and the prompt silently drops
    // to the generic `tool: input` form — the verb is most of what the reader is deciding on.
    let cases = [
        ("bash", json!({"command": "rm -rf build"}), "Run: rm -rf build"),
        ("read", json!({"path": "src/main.rs"}), "Read src/main.rs"),
        ("write", json!({"path": "src/new.rs"}), "Modify src/new.rs"),
        ("edit", json!({"path": "src/old.rs"}), "Modify src/old.rs"),
        ("git", json!({"action": "push", "args": ["--force"]}), "git push --force"),
        ("web_fetch", json!({"url": "https://example.com"}), "Fetch https://example.com"),
        (
            "mcp",
            json!({"tool": "github/create_issue", "args": "{}"}),
            "MCP call: github/create_issue",
        ),
        ("screenshot", json!({"url": "http://localhost:3000"}), "Capture http://localhost:3000"),
    ];
    for (tool, args, expected) in cases {
        assert_eq!(build_request(tool, &args, &root()).summary, expected, "{tool}'s prompt");
    }
}

/// "git commit" describes every commit ever made, so it is not what the reader is deciding on.
#[test]
fn a_commit_prompt_carries_the_subject_being_committed() {
    let request = build_request(
        "git",
        &json!({"action": "commit", "message": "Recognize Insert chords\n\nlong body…"}),
        &root(),
    );
    assert_eq!(request.summary, "git commit — Recognize Insert chords", "body stays out");

    // Bounded: a prompt is one line, whatever the model wrote.
    let long =
        build_request("git", &json!({"action": "commit", "message": "s".repeat(200)}), &root());
    assert!(long.summary.chars().count() < 90, "got {:?}", long.summary);
    assert!(long.summary.ends_with('…'));

    // Only for a commit, and only when there is a subject to carry.
    assert_eq!(build_request("git", &json!({"action": "commit"}), &root()).summary, "git commit");
    assert_eq!(
        build_request("git", &json!({"action": "log", "message": "x"}), &root()).summary,
        "git log"
    );
}

/// Approving a delegation without seeing the task is approving nothing in particular — and the
/// "always" rule this card offers is scoped to the tool, so the arguments are the only part of
/// the decision that is about *this* call.
#[test]
fn an_mcp_prompt_carries_the_arguments_it_is_about_to_send() {
    let request = build_request(
        "mcp",
        &json!({
            "tool": "delegate/delegate",
            "args": "{\n  \"task\": \"review src/parser.rs\",\n  \"role\": \"reviewer\"\n}"
        }),
        &root(),
    );
    assert_eq!(
        request.summary,
        "MCP call: delegate/delegate {\"task\":\"review src/parser.rs\",\"role\":\"reviewer\"}",
        "one line, in the order the model wrote it"
    );

    // Bounded, like the commit subject: a card is one line whatever the model wrote.
    let long = build_request(
        "mcp",
        &json!({
            "tool": "fs/write_file",
            "args": format!("{{\"text\":\"{}\"}}", "x".repeat(400)),
        }),
        &root(),
    );
    assert!(long.summary.chars().count() < 140, "got {} chars", long.summary.chars().count());
    assert!(long.summary.ends_with('\u{2026}'));

    // A secret in an argument must not be put on screen — or into the denial message and the
    // RPC frame that carry the same summary.
    let secret = build_request(
        "mcp",
        &json!({
            "tool": "deploy/run",
            "args": "{\"key\": \"sk-abcdefghijklmnopqrstuvwxyz0123\"}"
        }),
        &root(),
    );
    assert!(!secret.summary.contains("sk-abcdefghijklmnopqrstuvwxyz0123"), "{}", secret.summary);
    assert!(secret.summary.contains("REDACTED"), "{}", secret.summary);

    // Arguments the proxy itself would refuse are still what the server would be sent, so they
    // are shown rather than dropped.
    let unparseable = build_request("mcp", &json!({"tool": "x/y", "args": "not json"}), &root());
    assert_eq!(unparseable.summary, "MCP call: x/y not json");

    // And a call that takes no arguments says so by having none, not by drawing `{}`.
    assert_eq!(
        build_request("mcp", &json!({"tool": "github/list", "args": "{}"}), &root()).summary,
        "MCP call: github/list"
    );
}

/// A tool with no input still has to say which tool is asking, or the prompt is a bare verb.
#[test]
fn a_prompt_with_no_input_still_names_its_tool() {
    let summary = build_request("activate_tools", &json!({}), &root()).summary;
    assert!(summary.contains("activate_tools"), "got {summary:?}");
}

/// The verb is the difference between reading a file and rewriting it, and it is the only part
/// of the prompt that says which one this is.
#[test]
fn the_prompt_distinguishes_reading_from_modifying() {
    let read = build_request("read", &json!({"path": "notes.md"}), &root()).summary;
    let write = build_request("write", &json!({"path": "notes.md"}), &root()).summary;
    assert_ne!(read, write, "read and write must not read alike");
    assert!(read.starts_with("Read"), "got {read:?}");
    assert!(write.starts_with("Modify"), "got {write:?}");
}

/// The desktop modes have to be *nameable*, or the only rule that reaches them also gates the
/// browser. Keying on the URL alone gave `mode=fullscreen` an empty target, which the
/// permissive base allows outright — so photographing the screen was ungated and unwritable at
/// the same time. An omitted mode is `fullscreen`, so it must not fall through to empty either.
#[test]
fn photographing_the_screen_is_a_target_a_rule_can_name() {
    let target = |args: serde_json::Value| {
        build_request("screenshot", &args, &root()).checks[0].input.clone()
    };

    assert_eq!(target(json!({"mode": "fullscreen"})), "fullscreen");
    assert_eq!(target(json!({"mode": "window", "window": "luxe"})), "window");
    assert_eq!(target(json!({})), "fullscreen", "an omitted mode is the desktop, not nothing");
    assert_eq!(
        target(json!({"mode": "fullscreen", "url": "http://localhost:3000"})),
        "fullscreen",
        "a url beside a desktop mode does not make it a page"
    );
    assert_eq!(
        target(json!({"url": "http://localhost:3000"})),
        "http://localhost:3000",
        "a url with no mode is a page, as the tool reads it"
    );
    assert_eq!(
        target(json!({"mode": "url", "url": "http://localhost:3000"})),
        "http://localhost:3000",
        "a rendered page is still gated on the page"
    );
}

/// Each tool names the argument a rule is written against. Without its own arm a tool falls to
/// the generic "first string value" case, which sorts the object and picks whichever key comes
/// first alphabetically — that is how `screenshot` rules once matched the literal word "url".
#[test]
fn each_tool_is_gated_on_the_argument_worth_a_rule() {
    let cases = [
        ("bash", json!({"command": "ls", "timeout": 5}), "ls"),
        ("read", json!({"path": "a.rs", "limit": 10}), "a.rs"),
        ("read", json!({"file_path": "b.rs"}), "b.rs"),
        ("glob", json!({"pattern": "**/*.rs"}), "**/*.rs"),
        ("grep", json!({"pattern": "TODO", "glob": "*.rs"}), "TODO"),
        ("web_fetch", json!({"url": "https://x.dev", "format": "text"}), "https://x.dev"),
        ("web_search", json!({"query": "rust", "count": 3}), "rust"),
        // `mode` sorts before `url`, so the generic case picks the wrong one.
        ("screenshot", json!({"mode": "url", "url": "http://localhost"}), "http://localhost"),
    ];
    for (tool, args, expected) in cases {
        let request = build_request(tool, &args, &root());
        assert_eq!(request.checks[0].input, expected, "{tool} is gated on the wrong argument");
    }
}

/// The external-directory guard is for tools that take a path. A `grep` pattern or a search
/// query is not a path, and treating one as a path asks the user to approve a directory that
/// was never touched.
#[test]
fn only_path_tools_get_the_external_directory_guard() {
    let path_tool = build_request("read", &json!({"path": "/etc/hosts"}), &root());
    assert!(path_tool.checks.iter().any(|check| check.permission == EXTERNAL_DIRECTORY));

    for (tool, args) in [
        ("bash", json!({"command": "/etc/hosts"})),
        ("grep", json!({"pattern": "/etc/hosts"})),
        ("web_search", json!({"query": "/etc/hosts"})),
    ] {
        let request = build_request(tool, &args, &root());
        assert!(
            !request.checks.iter().any(|check| check.permission == EXTERNAL_DIRECTORY),
            "{tool} is not a path tool but got the external-directory guard"
        );
    }
}

#[test]
fn git_input_joins_action_and_args() {
    let req =
        build_request("git", &json!({"action": "push", "args": ["--force", "origin"]}), &root());
    assert_eq!(req.checks[0].input, "push --force origin");
}
