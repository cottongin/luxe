use super::*;

fn unknown_decision(action: HookFailureAction, response: &str, point: HookPoint) -> HookOutcome {
    let spec = CommandHookSpec {
        point,
        matcher: None,
        command: vec![],
        timeout_ms: 5000,
        on_timeout: action,
    };
    serde_json::from_str::<HookDecision>(response).unwrap().resolve(&spec)
}

/// A misspelled refusal must use the hook's deny fail-safe rather than let the call through.
#[test]
fn an_unrecognized_decision_uses_the_deny_fail_safe() {
    for decision in ["denied", "Deny", "reject"] {
        let response = serde_json::json!({"decision": decision}).to_string();
        let outcome =
            unknown_decision(HookFailureAction::Deny, &response, HookPoint::BeforeToolUse);
        match outcome {
            HookOutcome::Deny { reason } => {
                assert!(reason.contains("unrecognized decision"), "{reason}")
            }
            other => panic!("unknown decision {decision:?} allowed a call: {other:?}"),
        }
    }
}

/// A misspelled decision must escalate when the hook's fail-safe is ask.
#[test]
fn an_unrecognized_decision_uses_the_ask_fail_safe() {
    let outcome = unknown_decision(
        HookFailureAction::Ask,
        r#"{"decision":"reject"}"#,
        HookPoint::BeforeToolUse,
    );
    match outcome {
        HookOutcome::Ask(reason) => assert!(reason.contains("unrecognized decision"), "{reason}"),
        other => panic!("unknown decision did not ask: {other:?}"),
    }
}

/// Allowing a failed hook must discard its unrecognized response, not apply its patch or context.
#[test]
fn an_unrecognized_decision_uses_allow_without_applying_the_response() {
    for response in [
        r#"{"decision":"reject","args":{"command":"changed"}}"#,
        r#"{"decision":"reject","context":"unusable decision"}"#,
    ] {
        let outcome =
            unknown_decision(HookFailureAction::Allow, response, HookPoint::BeforeToolUse);
        assert!(matches!(outcome, HookOutcome::Continue), "{outcome:?}");
    }
}

/// Ask has no prompt outside BeforeToolUse, so an unknown response must be discarded there.
#[test]
fn an_unrecognized_decision_does_not_inject_context_when_ask_cannot_prompt() {
    let outcome = unknown_decision(
        HookFailureAction::Ask,
        r#"{"decision":"reject","context":"unusable decision"}"#,
        HookPoint::AfterToolUse,
    );
    assert!(matches!(outcome, HookOutcome::Continue), "{outcome:?}");
}

/// The subprocess runner must apply the typed decision's fail-safe, not bypass its resolver.
#[cfg(unix)]
#[tokio::test]
async fn the_runner_uses_the_unknown_decision_fail_safe() {
    let (shell, flag) = crate::program::shell();
    let spec = CommandHookSpec {
        point: HookPoint::BeforeToolUse,
        matcher: None,
        command: vec![
            crate::paths::display(&shell),
            flag.into(),
            r#"printf '%s' '{"decision":"reject"}'"#.into(),
        ],
        timeout_ms: 5000,
        on_timeout: HookFailureAction::Deny,
    };
    let outcome = run_command_hook(&spec, &HookEvent::Other(serde_json::Value::Null)).await;
    match outcome {
        HookOutcome::Deny { reason } => {
            assert!(reason.contains("unrecognized decision"), "{reason}")
        }
        other => panic!("the runner bypassed the fail-safe: {other:?}"),
    }
}
