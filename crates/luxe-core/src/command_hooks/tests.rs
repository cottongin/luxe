use super::*;

mod unknown;

#[test]
fn test_hook_decision_serialization() {
    let decision = HookDecision {
        decision: HookDecisionKind::Allow,
        reason: Some("Test reason".to_string()),
        args: None,
        context: None,
    };
    let json = serde_json::to_string(&decision).unwrap();
    assert!(json.contains("allow"));
    assert!(json.contains("Test reason"));
}

#[test]
fn test_hook_decision_deserialization() {
    let json = r#"{ "decision": "deny", "reason": "Test" }"#;
    let decision: HookDecision = serde_json::from_str(json).unwrap();
    assert_eq!(decision.decision, HookDecisionKind::Deny);
    assert_eq!(decision.reason.unwrap(), "Test");
}

#[test]
fn test_command_hook_spec_serialization() {
    let spec = CommandHookSpec {
        point: HookPoint::BeforeToolUse,
        matcher: None,
        command: vec!["echo".to_string(), "hello".to_string()],
        timeout_ms: 5000,
        on_timeout: HookFailureAction::Deny,
    };
    let json = serde_json::to_string(&spec).unwrap();
    assert!(json.contains("echo"));
    assert!(json.contains("5000"));
}

#[test]
fn test_hook_event_serialization() {
    let event = HookEvent::ToolUse {
        tool_name: "bash".to_string(),
        args: serde_json::json!({"command": "ls"}),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("bash"));
}

/// One unrecognized key must cost that key, not the file.
///
/// The shape this guards is an update renaming a hook point: the caller's fail-safe for a
/// parse error is an empty `HooksFile`, so a strict map turned one stale key into "none of
/// your hooks run any more", reported only to a log nobody was watching.
#[test]
fn an_unknown_hook_point_does_not_take_the_rest_of_the_file_with_it() {
    let file = HooksFile::from_json(
        r#"{
                "BeforeToolUse": [ { "command": ["guard"] } ],
                "SubagentStop":  [ { "command": ["nope"] } ],
                "AfterToolUse":  [ { "command": ["lint"] } ]
            }"#,
    )
    .expect("an unknown point is not a parse failure");

    assert_eq!(file.unknown_points, vec!["SubagentStop".to_string()]);
    let specs = file.into_specs();
    assert!(specs.contains_key(&HookPoint::BeforeToolUse), "the good hooks survived");
    assert!(specs.contains_key(&HookPoint::AfterToolUse), "the good hooks survived");
    assert_eq!(specs.len(), 2);
}

/// Two spellings of one point are the same point, so neither silently shadows the other.
#[test]
fn hooks_written_under_different_spellings_of_a_point_are_merged() {
    let file = HooksFile::from_json(
        r#"{
                "BeforeToolUse":   [ { "command": ["first"] } ],
                "before_tool_use": [ { "command": ["second"] } ]
            }"#,
    )
    .unwrap();
    assert!(file.unknown_points.is_empty());
    assert_eq!(file.into_specs()[&HookPoint::BeforeToolUse].len(), 2);
}

/// Hooks at one point run in sequence, so a caller waiting on the point waits for all of them.
#[test]
fn a_points_budget_is_every_deadline_end_to_end() {
    let file = HooksFile::from_json(
        r#"{ "SessionEnd": [ { "command": ["a"], "timeout_ms": 1000 }, { "command": ["b"] } ] }"#,
    )
    .unwrap();
    let hook = CommandHook::new(file.into_specs());
    assert_eq!(hook.budget(HookPoint::SessionEnd), Duration::from_millis(6000));
    assert_eq!(hook.budget(HookPoint::SessionStart), Duration::ZERO);
}

#[test]
fn hooks_file_parses_with_defaults() {
    let file = HooksFile::from_json(r#"{ "BeforeToolUse": [ { "command": ["true"] } ] }"#).unwrap();
    let specs = file.into_specs();
    let point = specs.get(&HookPoint::BeforeToolUse).expect("point present");
    assert_eq!(point.len(), 1);
    assert_eq!(point[0].point, HookPoint::BeforeToolUse);
    assert_eq!(point[0].timeout_ms, 5000, "default timeout");
    assert_eq!(point[0].on_timeout, HookFailureAction::Allow, "default fail-safe");
}

// Every caller is a `#[cfg(unix)]` test: the hook scripts they configure are `sh -c` one-liners.
#[cfg(unix)]
fn test_ctx() -> HookCtx {
    HookCtx {
        session_id: "test".to_string(),
        event_tx: tokio::sync::mpsc::channel(1).0,
        hook_registry: crate::hook::HookRegistry::new(),
    }
}

#[cfg(unix)]
fn tool_use_event() -> HookEvent {
    HookEvent::ToolUse { tool_name: "bash".to_string(), args: serde_json::json!({"command": "ls"}) }
}

#[cfg(unix)]
#[tokio::test]
async fn command_hook_denies_when_script_says_deny() {
    let file = HooksFile::from_json(
        r#"{ "BeforeToolUse": [ { "command": ["sh", "-c", "echo '{\"decision\":\"deny\",\"reason\":\"blocked\"}'"] } ] }"#,
    )
    .unwrap();
    let hook = CommandHook::new(file.into_specs());
    let ev = tool_use_event();
    assert!(matches!(
        hook.on(HookPoint::BeforeToolUse, &ev, &test_ctx()).await,
        HookOutcome::Deny { .. }
    ));
    // A point with no configured hook is a no-op.
    assert!(matches!(
        hook.on(HookPoint::AfterToolUse, &ev, &test_ctx()).await,
        HookOutcome::Continue
    ));
}

/// The three answers a gating hook can give, and the one field that composes with them.
///
/// Pinned against literals rather than round-tripped, because these strings are what users
/// have already written in scripts on machines nobody here can see. Renaming `ask` to
/// `confirm` would break every one of them and no compiler would say so — the parse would
/// keep succeeding and the escalation would quietly become a no-op.
#[test]
fn the_decision_vocabulary_is_pinned() {
    let outcome = |json: &str| {
        serde_json::from_str::<HookDecision>(json)
            .expect("valid decision")
            .into_outcome(HookPoint::BeforeToolUse)
    };

    assert!(matches!(outcome(r#"{"decision":"allow"}"#), HookOutcome::Continue));
    assert!(matches!(outcome(r#"{"decision":"deny"}"#), HookOutcome::Deny { .. }));
    assert!(matches!(outcome(r#"{"decision":"block"}"#), HookOutcome::Deny { .. }));

    match outcome(r#"{"decision":"ask","reason":"looks risky"}"#) {
        HookOutcome::Ask(reason) => assert_eq!(reason, "looks risky"),
        other => panic!("expected Ask, got {other:?}"),
    }
    match outcome(r#"{"decision":"allow","args":{"command":"bun test"}}"#) {
        HookOutcome::Modify { args, ask } => {
            assert_eq!(args["command"], "bun test");
            assert!(ask.is_none(), "a bare rewrite does not prompt");
        }
        other => panic!("expected Modify, got {other:?}"),
    }
    // A rewrite and an escalation in one decision: the user confirms what will actually run.
    match outcome(r#"{"decision":"ask","reason":"rewrote it","args":{"command":"x"}}"#) {
        HookOutcome::Modify { args, ask } => {
            assert_eq!(args["command"], "x");
            assert_eq!(ask.as_deref(), Some("rewrote it"));
        }
        other => panic!("expected Modify carrying the ask, got {other:?}"),
    }
    // A refusal outranks its own rewrite — there is no call left to patch.
    assert!(matches!(
        outcome(r#"{"decision":"deny","args":{"command":"x"}}"#),
        HookOutcome::Deny { .. }
    ));
}

/// A guard's reason quotes the call it objected to, so it is a way for a secret to travel.
///
/// Both directions matter and they have different destinations: a deny's reason is written
/// into the transcript the model keeps reading, and an `ask`'s is drawn on the approval card.
/// `context` has always been scrubbed on this trip; `reason` reaching two more surfaces
/// unscrubbed was the asymmetry.
#[test]
fn a_reason_is_scrubbed_before_it_leaves_the_hook() {
    crate::redact::process_secrets().add("plain-hook-reason-secret");
    let decision = |verdict: &str| {
        serde_json::from_str::<HookDecision>(&format!(
            r#"{{"decision":"{verdict}","reason":"refusing: curl -H 'api_key: sk-live-4471abcd' plain-hook-reason-secret"}}"#
        ))
        .expect("valid decision")
        .into_outcome(HookPoint::BeforeToolUse)
    };
    let text = match decision("deny") {
        HookOutcome::Deny { reason } => reason,
        other => panic!("expected Deny, got {other:?}"),
    };
    assert!(text.contains("[REDACTED]"), "the credential is gone: {text}");
    assert!(!text.contains("sk-live-4471abcd"), "{text}");
    assert!(!text.contains("plain-hook-reason-secret"), "a resolved key is gone too: {text}");
    assert!(text.contains("refusing"), "the explanation survives: {text}");

    let text = match decision("ask") {
        HookOutcome::Ask(reason) => reason,
        other => panic!("expected Ask, got {other:?}"),
    };
    assert!(!text.contains("sk-live-4471abcd"), "the prompt is a surface too: {text}");
}

/// `args` and `ask` describe a tool call, so they mean nothing where there is no call.
///
/// Returning them anyway would reach `dispatch` as an outcome it can only warn about, and a
/// hook author would be left reading a log to find out their patch went nowhere.
#[test]
fn a_rewrite_outside_before_tool_use_is_dropped_not_half_applied() {
    let decision: HookDecision =
        serde_json::from_str(r#"{"decision":"ask","args":{"command":"x"},"context":"note"}"#)
            .expect("valid decision");
    match decision.into_outcome(HookPoint::AfterToolUse) {
        HookOutcome::InjectContext(ctx) => assert_eq!(ctx, "note"),
        other => panic!("expected the context and nothing else, got {other:?}"),
    }
}

/// A broken guard must be able to put the question to the reader.
///
/// `ask` was accepted by the config parser and behaved as `allow`, so the one setting that
/// says "I do not trust this hook's silence" was the setting that did nothing.
#[test]
fn a_failed_hook_can_escalate_instead_of_waving_the_call_through() {
    let spec = |point, on_timeout| CommandHookSpec {
        point,
        matcher: None,
        command: vec!["true".to_string()],
        timeout_ms: 5000,
        on_timeout,
    };
    let reason = || "hook exploded".to_string();

    assert!(matches!(
        timeout_outcome(&spec(HookPoint::BeforeToolUse, HookFailureAction::Ask), reason()),
        HookOutcome::Ask(_)
    ));
    // Nowhere else holds a call open for an answer, so it stays the permissive default.
    assert!(matches!(
        timeout_outcome(&spec(HookPoint::AfterToolUse, HookFailureAction::Ask), reason()),
        HookOutcome::Continue
    ));
    assert!(matches!(
        timeout_outcome(&spec(HookPoint::BeforeToolUse, HookFailureAction::Allow), reason()),
        HookOutcome::Continue
    ));
    assert!(matches!(
        timeout_outcome(&spec(HookPoint::BeforeToolUse, HookFailureAction::Deny), reason()),
        HookOutcome::Deny { .. }
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn command_hook_injects_context_from_a_decision() {
    let file = HooksFile::from_json(
        r#"{ "AfterToolUse": [ { "command": ["sh", "-c", "echo '{\"decision\":\"allow\",\"context\":\"lint failed: fix line 3\"}'"] } ] }"#,
    )
    .unwrap();
    let hook = CommandHook::new(file.into_specs());
    let ev = HookEvent::ToolResult {
        tool_name: "edit".to_string(),
        result: "ok".to_string(),
        is_error: false,
    };
    match hook.on(HookPoint::AfterToolUse, &ev, &test_ctx()).await {
        HookOutcome::InjectContext(ctx) => assert!(ctx.contains("lint failed")),
        other => panic!("expected InjectContext, got {other:?}"),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn command_hook_ignoring_stdin_still_parses_its_decision() {
    // A hook that never reads stdin is legitimate. Pair it with an event larger than the OS
    // pipe buffer (~64KB) so the parent's `write_all` blocks and then hits EPIPE the instant
    // the stdin-ignoring child exits. That EPIPE must NOT be treated as a hook failure — the
    // child still emitted a valid decision on stdout. Regression for the stdin-write race that
    // made stdin-ignoring hooks non-deterministically fall back to the on_timeout action.
    let big = "x".repeat(256 * 1024);
    let ev = HookEvent::ToolUse {
        tool_name: "bash".to_string(),
        args: serde_json::json!({ "command": big }),
    };
    let file = HooksFile::from_json(
        r#"{ "BeforeToolUse": [ { "command": ["sh", "-c", "echo '{\"decision\":\"deny\",\"reason\":\"blocked\"}'"], "on_timeout": "allow" } ] }"#,
    )
    .unwrap();
    let hook = CommandHook::new(file.into_specs());
    // on_timeout=allow, so the buggy EPIPE-bail path returns Continue; the correct path parses
    // the child's `deny` from stdout. Deny must win regardless of the stdin race.
    assert!(matches!(
        hook.on(HookPoint::BeforeToolUse, &ev, &test_ctx()).await,
        HookOutcome::Deny { .. }
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn command_hook_allows_when_script_says_allow() {
    let file = HooksFile::from_json(
        r#"{ "BeforeToolUse": [ { "command": ["sh", "-c", "echo '{\"decision\":\"allow\"}'"] } ] }"#,
    )
    .unwrap();
    let hook = CommandHook::new(file.into_specs());
    assert!(matches!(
        hook.on(HookPoint::BeforeToolUse, &tool_use_event(), &test_ctx()).await,
        HookOutcome::Continue
    ));
}

/// No output is no opinion, which the guide promises and a fail-closed guard depends on.
///
/// It used to be a parse failure, so a hook that printed nothing took its `on_timeout` — and
/// with `"deny"` that refused every call it matched, reporting "invalid JSON" to the model.
#[cfg(unix)]
#[tokio::test]
async fn a_hook_that_prints_nothing_has_no_opinion() {
    let file = HooksFile::from_json(
        r#"{ "BeforeToolUse": [ { "command": ["sh", "-c", "cat >/dev/null"], "on_timeout": "deny" } ] }"#,
    )
    .unwrap();
    let hook = CommandHook::new(file.into_specs());
    assert!(matches!(
        hook.on(HookPoint::BeforeToolUse, &tool_use_event(), &test_ctx()).await,
        HookOutcome::Continue
    ));
}

/// Observe-only points ignore output, so whatever a notifier prints is not a broken decision.
#[cfg(unix)]
#[tokio::test]
async fn an_observer_can_print_anything() {
    let file = HooksFile::from_json(
        r#"{ "AgentEnd": [ { "command": ["sh", "-c", "echo sent"], "on_timeout": "deny" } ] }"#,
    )
    .unwrap();
    let hook = CommandHook::new(file.into_specs());
    let ev = HookEvent::Other(serde_json::Value::Null);
    assert!(matches!(hook.on(HookPoint::AgentEnd, &ev, &test_ctx()).await, HookOutcome::Continue));
}

/// `"command": []` parses, and indexing it panicked the worker mid-turn.
///
/// The turn died with its tool call committed and no result, and a headless run exited 0
/// having printed nothing. It is a program that cannot spawn, so it gets that fail-safe.
#[tokio::test]
async fn an_empty_command_fails_safe_instead_of_panicking() {
    let spec = |on_timeout| CommandHookSpec {
        point: HookPoint::BeforeToolUse,
        matcher: None,
        command: Vec::new(),
        timeout_ms: 5000,
        on_timeout,
    };
    let event = HookEvent::ToolUse {
        tool_name: "bash".to_string(),
        args: serde_json::json!({"command": "ls"}),
    };
    assert!(matches!(
        run_command_hook(&spec(HookFailureAction::Deny), &event).await,
        HookOutcome::Deny { .. }
    ));
    assert!(matches!(
        run_command_hook(&spec(HookFailureAction::Allow), &event).await,
        HookOutcome::Continue
    ));
}

/// Sequential stdin/stdout handling deadlocks when an echoing hook fills both pipe buffers.
#[cfg(unix)]
#[tokio::test]
async fn a_hook_that_echoes_a_large_event_still_answers() {
    let (shell, flag) = crate::program::shell();
    let spec = CommandHookSpec {
        point: HookPoint::AgentEnd,
        matcher: None,
        command: vec![crate::paths::display(&shell), flag.into(), "cat".into()],
        timeout_ms: 2000,
        on_timeout: HookFailureAction::Deny,
    };
    for bytes in [200 * 1024, 2 * 1024 * 1024] {
        let event = HookEvent::Other(serde_json::json!({"content": "x".repeat(bytes)}));
        let outcome = run_command_hook(&spec, &event).await;
        assert!(
            matches!(outcome, HookOutcome::Continue),
            "the {bytes}-byte echo stalled: {outcome:?}"
        );
    }
}

/// Omitting the stdin write or leaving its pipe open must not pass as a successful delivery.
#[cfg(unix)]
#[tokio::test]
async fn feeding_stdin_delivers_the_event_and_closes_the_pipe() {
    let (shell, flag) = crate::program::shell();
    let mut child = Command::new(shell)
        .args([flag, "cat"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let stdin = child.stdin.take();
    let event = br#"{"Other":{"content":"the whole event"}}"#;
    let (written, output) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(feed_stdin(stdin, event), child.wait_with_output())
    })
    .await
    .expect("delivery must close stdin so the reader can exit");
    written.unwrap();
    let output = output.unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, event);
}

/// `timeout_ms` bounds the whole hook, not just the wait.
///
/// A pipe holds about 64KB, so an event larger than that fills it and `write_all` then waits for
/// a reader. A hook that never reads its stdin is legitimate, and while it lives the write
/// cannot progress — so with the write outside the timeout, `on_timeout` meant nothing at the
/// one moment it exists for. Measured before the fix: 30s against a configured 100ms.
///
/// The bound here is deliberately an order of magnitude above the timeout and an order below
/// the child's lifetime, so it catches the defect returning without measuring the machine.
#[cfg(unix)]
#[tokio::test]
async fn a_hook_that_never_reads_stdin_still_honors_its_timeout() {
    let oversized_for_a_pipe = "x".repeat(256 * 1024);
    let event = HookEvent::ToolUse {
        tool_name: "bash".to_string(),
        args: serde_json::json!({ "command": oversized_for_a_pipe }),
    };
    let file = HooksFile::from_json(
        r#"{ "BeforeToolUse": [ { "command": ["sh", "-c", "sleep 30"], "timeout_ms": 100, "on_timeout": "deny" } ] }"#,
    )
    .unwrap();
    let hook = CommandHook::new(file.into_specs());

    let start = std::time::Instant::now();
    let outcome = hook.on(HookPoint::BeforeToolUse, &event, &test_ctx()).await;
    let elapsed = start.elapsed();

    assert!(matches!(outcome, HookOutcome::Deny { .. }), "the fail-safe applies: {outcome:?}");
    assert!(elapsed < std::time::Duration::from_secs(10), "took {elapsed:?} for a 100ms timeout");
}

#[cfg(unix)]
#[tokio::test]
async fn command_hook_timeout_uses_the_fail_safe() {
    let file = HooksFile::from_json(
        r#"{ "BeforeToolUse": [ { "command": ["sh", "-c", "sleep 5"], "timeout_ms": 100, "on_timeout": "deny" } ] }"#,
    )
    .unwrap();
    let hook = CommandHook::new(file.into_specs());
    assert!(matches!(
        hook.on(HookPoint::BeforeToolUse, &tool_use_event(), &test_ctx()).await,
        HookOutcome::Deny { .. }
    ));
}

/// Three hooks at one point, each with a different opinion, all of which have to survive.
///
/// Only one outcome reaches the caller, so a precedence table that simply keeps the winner
/// throws the others away — a rewriter configured beside a classifier would have its patch
/// silently dropped the moment the classifier asked, and the tool would run the original
/// command while the prompt described a different one.
#[cfg(unix)]
#[tokio::test]
async fn a_rewrite_and_an_escalation_both_survive_the_fold() {
    let emit = |json: &str| {
        format!(r#"{{ "command": ["sh", "-c", "echo '{}'"] }}"#, json.replace('"', "\\\""))
    };
    let file = HooksFile::from_json(&format!(
        r#"{{ "BeforeToolUse": [ {}, {}, {} ] }}"#,
        emit(r#"{"decision":"allow","args":{"command":"bun test","cwd":"/tmp"}}"#),
        emit(r#"{"decision":"ask","reason":"unfamiliar command"}"#),
        emit(r#"{"decision":"allow","args":{"command":"bun test --bail"}}"#),
    ))
    .unwrap();

    let hook = CommandHook::new(file.into_specs());
    match hook.on(HookPoint::BeforeToolUse, &tool_use_event(), &test_ctx()).await {
        HookOutcome::Modify { args, ask } => {
            assert_eq!(args["command"], "bun test --bail", "last writer wins a collision");
            assert_eq!(args["cwd"], "/tmp", "a key the later patch omitted survives");
            assert_eq!(ask.as_deref(), Some("unfamiliar command"));
        }
        other => panic!("expected a folded Modify, got {other:?}"),
    }
}

/// A matcher's whole job is to stop the process from being spawned at all.
///
/// Asserted by side effect rather than by outcome, because a hook that runs and answers
/// `allow` is indistinguishable from one that never ran — which is exactly the bug a matcher
/// would have if it filtered the *answer* instead of the spawn, and it would cost a process
/// per tool call while looking perfectly correct.
#[cfg(unix)]
#[tokio::test]
async fn a_matcher_stops_the_spawn_rather_than_the_answer() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ran = dir.path().join("ran");
    let file = HooksFile::from_json(&format!(
        r#"{{ "BeforeToolUse": [
                {{ "matcher": "web_*", "command": ["sh", "-c", "touch {}"] }}
            ] }}"#,
        ran.display()
    ))
    .expect("valid hooks file");
    let hook = CommandHook::new(file.into_specs());

    hook.on(HookPoint::BeforeToolUse, &tool_use_event(), &test_ctx()).await;
    assert!(!ran.exists(), "a bash call must not spawn a hook matched to web_*");

    let web = HookEvent::ToolUse {
        tool_name: "web_fetch".to_string(),
        args: serde_json::json!({ "url": "https://example.com" }),
    };
    hook.on(HookPoint::BeforeToolUse, &web, &test_ctx()).await;
    assert!(ran.exists(), "the tool it was matched to still runs it");
}

/// The argument half speaks the same language as `permissions.json`, segment-aware bash and
/// all — which is the reason it reuses that matcher instead of growing a second one.
#[test]
fn a_matcher_reads_arguments_the_way_a_permission_rule_does() {
    let matcher: HookMatcher = serde_json::from_str(r#"{"bash": "rm *"}"#).expect("valid matcher");
    let call = |command: &str| HookEvent::ToolUse {
        tool_name: "bash".to_string(),
        args: serde_json::json!({ "command": command }),
    };

    assert!(matcher.matches(&call("rm -rf target")));
    // The lone `&` that once hid a command from every bash rule: segment-aware matching is
    // what sees it, and a hand-rolled glob is what would not.
    assert!(matcher.matches(&call("cd /tmp && rm -rf x")));
    assert!(!matcher.matches(&call("cargo test")));
    // A different tool is out of scope however destructive it reads.
    assert!(!matcher.matches(&HookEvent::ToolUse {
        tool_name: "read".to_string(),
        args: serde_json::json!({ "path": "rm -rf" }),
    }));
    // No arguments to read (AfterToolUse): the tool-name half still decides, rather than the
    // matcher quietly never firing — and it still decides *against* the wrong tool, which is
    // the half that separates "filter on the name" from "give up and match everything".
    let result = |tool: &str| HookEvent::ToolResult {
        tool_name: tool.to_string(),
        result: String::new(),
        is_error: false,
    };
    assert!(matcher.matches(&result("bash")));
    assert!(!matcher.matches(&result("read")));
    // An event with no tool call at all is never filtered.
    assert!(matcher.matches(&HookEvent::Other(serde_json::json!({}))));
}

/// Which matchers are worth warning about when they land where arguments never arrive.
///
/// `"*"` is the one keyed pattern that means the same thing with or without them, so warning
/// about it would train the reader to ignore the warning that matters.
#[test]
fn only_a_matcher_that_reads_arguments_is_worth_a_warning() {
    let matcher = |json: &str| {
        serde_json::from_str::<HookMatcher>(json).expect("valid matcher").needs_arguments()
    };
    assert!(matcher(r#"{"bash": "rm *"}"#));
    assert!(!matcher(r#"{"bash": "*"}"#));
    assert!(!matcher(r#""bash""#), "a tool-name glob works everywhere");
}

/// A refusal ends the point: the hooks after it never run.
///
/// Each one is a process spawn on the turn's critical path, and none of them can change an
/// answer that is already no.
#[cfg(unix)]
#[tokio::test]
async fn a_deny_short_circuits_the_hooks_behind_it() {
    let marker = std::env::temp_dir().join("luxe-hook-short-circuit-probe");
    let _ = std::fs::remove_file(&marker);
    let file = HooksFile::from_json(&format!(
        r#"{{ "BeforeToolUse": [
                {{ "command": ["sh", "-c", "echo '{{\"decision\":\"deny\",\"reason\":\"no\"}}'"] }},
                {{ "command": ["sh", "-c", "touch {}"] }}
            ] }}"#,
        marker.display()
    ))
    .unwrap();

    let hook = CommandHook::new(file.into_specs());
    assert!(matches!(
        hook.on(HookPoint::BeforeToolUse, &tool_use_event(), &test_ctx()).await,
        HookOutcome::Deny { .. }
    ));
    assert!(!marker.exists(), "the hook behind a deny must not have run");
}
