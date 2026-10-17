use super::*;

/// Two hooks, two answers, and which one the caller gets — pinned in both orders.
///
/// Both orders, because the table's fallthrough keeps whichever outcome arrived first, so an
/// arm that is missing in one direction looks correct from the other. That is not
/// hypothetical: a refusal arriving *after* a context injection used to be dropped on the
/// floor, and every single-direction assertion in the world would have agreed it was fine.
#[test]
fn the_precedence_table_is_pinned_in_both_directions() {
    let deny = || HookOutcome::Deny { reason: "no".into() };
    let ask = || HookOutcome::Ask("confirm".into());
    let modify = || HookOutcome::Modify { args: serde_json::json!({"a": 1}), ask: None };
    let context = || HookOutcome::InjectContext("note".into());
    let stop = || HookOutcome::StopLoop { reason: "done".into() };

    // `label(winner)` on each side, so a failure names what came back.
    let label = |outcome: &HookOutcome| match outcome {
        HookOutcome::Continue => "continue",
        HookOutcome::Deny { .. } => "deny",
        HookOutcome::Modify { .. } => "modify",
        HookOutcome::Ask(_) => "ask",
        HookOutcome::InjectContext(_) => "context",
        HookOutcome::ContinueLoop { .. } => "continue-loop",
        HookOutcome::StopLoop { .. } => "stop-loop",
    };
    type Case = (&'static str, fn() -> HookOutcome, fn() -> HookOutcome);
    let table: &[Case] = &[
        ("deny", deny, ask),
        ("deny", deny, modify),
        ("deny", deny, context),
        ("deny", deny, || HookOutcome::Continue),
        ("modify", modify, ask),
        ("modify", modify, context),
        ("ask", ask, context),
        ("ask", ask, || HookOutcome::Continue),
        ("stop-loop", stop, deny),
    ];
    for (expected, first, second) in table {
        for (a, b) in [(first(), second()), (second(), first())] {
            let (left, right) = (label(&a), label(&b));
            let got = merge_outcomes(a, b);
            assert_eq!(
                label(&got),
                *expected,
                "merging {left} with {right} should give {expected}"
            );
        }
    }
}

/// A rewrite and an escalation are not alternatives, so folding them keeps both.
#[test]
fn a_fold_never_drops_a_patch_or_a_reason() {
    let first = HookOutcome::Modify {
        args: serde_json::json!({ "command": "a", "cwd": "/tmp" }),
        ask: Some("one".into()),
    };
    let second = HookOutcome::Modify {
        args: serde_json::json!({ "command": "b" }),
        ask: Some("two".into()),
    };
    match merge_outcomes(first, second) {
        HookOutcome::Modify { args, ask } => {
            assert_eq!(args["command"], "b", "the later patch wins a collided key");
            assert_eq!(args["cwd"], "/tmp", "a key the later patch omitted survives");
            assert_eq!(ask.as_deref(), Some("one\n\ntwo"), "both reasons are kept");
        }
        other => panic!("expected a folded Modify, got {other:?}"),
    }
    // An escalation beside a rewrite rides along instead of replacing it.
    match merge_outcomes(
        HookOutcome::Modify { args: serde_json::json!({ "command": "a" }), ask: None },
        HookOutcome::Ask("confirm".into()),
    ) {
        HookOutcome::Modify { args, ask } => {
            assert_eq!(args["command"], "a");
            assert_eq!(ask.as_deref(), Some("confirm"));
        }
        other => panic!("expected the patch to survive the escalation, got {other:?}"),
    }
    // Two guards with two worries: the prompt has to carry both, or the one that is dropped
    // is the one the reader needed to see.
    match merge_outcomes(
        HookOutcome::Ask("unsigned binary".into()),
        HookOutcome::Ask("writes outside the repo".into()),
    ) {
        HookOutcome::Ask(reason) => {
            assert_eq!(reason, "unsigned binary\n\nwrites outside the repo")
        }
        other => panic!("expected both reasons, got {other:?}"),
    }
}

/// A patch replaces the keys it names and nothing else.
#[test]
fn merging_arguments_is_shallow() {
    let merged = merge_args(
        serde_json::json!({ "command": "a", "timeout": 60, "env": { "A": "1", "B": "2" } }),
        serde_json::json!({ "command": "b", "env": { "A": "9" } }),
    );
    assert_eq!(merged["command"], "b");
    assert_eq!(merged["timeout"], 60, "an untouched key survives");
    assert_eq!(merged["env"], serde_json::json!({ "A": "9" }), "a nested object is replaced");
    // Nothing to merge into: the patch stands alone rather than being silently discarded.
    assert_eq!(merge_args(serde_json::json!("scalar"), serde_json::json!({ "a": 1 }))["a"], 1);
}

/// The hook vocabulary is a published contract, pinned against a literal.
///
/// These strings are what users have written in their own `hooks.json`. Nothing in the type
/// system objects to renaming a variant, and the failure it causes is silent — the hook simply
/// stops running, on a machine nobody here can see. A round-trip test would sail through it,
/// because it exercises both halves of the same renamed code.
#[test]
fn the_hook_vocabulary_is_pinned() {
    let names: Vec<&str> = HookPoint::ALL.iter().map(HookPoint::canonical_name).collect();
    assert_eq!(
        names,
        [
            "SessionStart",
            "SessionEnd",
            "BeforeAgentStart",
            "UserPromptSubmit",
            "AgentStart",
            "AgentEnd",
            "BeforeToolUse",
            "AfterToolUse",
            "MessageStart",
            "MessageEnd",
            "BeforeCompact",
            "AfterCompact",
            "FileChanged",
            "Notification",
            "Error",
            "ReviewGate",
            "LoopControl",
            "PermissionRequested",
            "PermissionResolved",
        ],
        "renaming a hook point unhooks every user who named it; add, don't rename"
    );
}

/// `canonical_name` must agree with how the variant actually serializes, or a `hooks.json`
/// written from the documented name would not deserialize.
#[test]
fn the_canonical_name_is_the_serialized_name() {
    for point in HookPoint::ALL {
        let json = serde_json::to_string(&point).expect("a hook point serializes");
        assert_eq!(json, format!("\"{}\"", point.canonical_name()));
    }
}

#[test]
fn a_hook_point_parses_however_the_user_spelled_it() {
    for point in HookPoint::ALL {
        assert_eq!(HookPoint::parse(point.canonical_name()), Some(point));
    }
    for spelling in ["before_tool_use", "beforeToolUse", "before-tool-use", "BEFORETOOLUSE"] {
        assert_eq!(HookPoint::parse(spelling), Some(HookPoint::BeforeToolUse), "{spelling}");
    }
    assert_eq!(HookPoint::parse("SubagentStop"), None);
}

#[test]
fn an_unknown_point_suggests_the_nearest_known_one() {
    assert_eq!(HookPoint::closest("BeforeToolUsage"), Some("BeforeToolUse"));
    assert_eq!(HookPoint::closest("SessionStarted"), Some("SessionStart"));
    // Nothing shares a prefix worth suggesting, so say nothing rather than guess.
    assert_eq!(HookPoint::closest("Frobnicate"), None);
}

/// A mock hook that returns a fixed outcome.
struct MockHook {
    name: &'static str,
    interest: Vec<HookPoint>,
    outcome: HookOutcome,
}

impl MockHook {
    fn new(interest: Vec<HookPoint>, outcome: HookOutcome) -> Self {
        Self { name: "mock", interest, outcome }
    }

    fn named(name: &'static str, interest: Vec<HookPoint>, outcome: HookOutcome) -> Self {
        Self { name, interest, outcome }
    }
}

#[async_trait::async_trait]
impl Hook for MockHook {
    fn name(&self) -> &'static str {
        self.name
    }

    fn interest(&self) -> &[HookPoint] {
        &self.interest
    }

    async fn on(&self, _point: HookPoint, _ev: &HookEvent, _cx: &HookCtx) -> HookOutcome {
        self.outcome.clone()
    }
}

/// The registry can say what is in it, in the order it will run.
///
/// The break this catches: `hook_names` answered `["hook", "hook", …]` and said so in a comment,
/// so nothing the loop did could be attributed to whatever did it — not in a log, not in the
/// line `luxe loop start` prints about what it registered, which was a hand-written list that
/// described the hooks of whichever day somebody typed it.
#[test]
fn the_registry_names_what_it_will_run_in_order() {
    let mut registry = HookRegistry::new();
    for name in ["self-continuation", "review-integrator", "runaway-guards"] {
        registry.register(Arc::new(MockHook::named(
            name,
            vec![HookPoint::AgentEnd],
            HookOutcome::Continue,
        )));
    }

    assert_eq!(
        registry.hook_names(),
        vec!["self-continuation", "review-integrator", "runaway-guards"]
    );
}

#[tokio::test]
async fn test_merge_stoploop_wins_over_continue_loop() {
    let mut registry = HookRegistry::new();
    let hook_continue_loop = MockHook::new(
        vec![HookPoint::AgentEnd],
        HookOutcome::ContinueLoop { message: "next".to_string() },
    );
    let hook_stop = MockHook::new(
        vec![HookPoint::AgentEnd],
        HookOutcome::StopLoop { reason: "stop".to_string() },
    );
    registry.register(Arc::new(hook_continue_loop));
    registry.register(Arc::new(hook_stop));

    let cx = HookCtx {
        session_id: "test".to_string(),
        event_tx: tokio::sync::mpsc::channel(1).0,
        hook_registry: HookRegistry::new(),
    };

    let outcome =
        registry.invoke(HookPoint::AgentEnd, &HookEvent::Other(serde_json::json!({})), &cx).await;

    match outcome {
        HookOutcome::StopLoop { reason } => assert_eq!(reason, "stop"),
        other => panic!("Expected StopLoop, got {:?}", other),
    }
}

#[tokio::test]
async fn test_merge_stoploop_wins_over_continue() {
    let mut registry = HookRegistry::new();
    let hook_continue = MockHook::new(vec![HookPoint::AgentEnd], HookOutcome::Continue);
    let hook_stop = MockHook::new(
        vec![HookPoint::AgentEnd],
        HookOutcome::StopLoop { reason: "stop".to_string() },
    );
    registry.register(Arc::new(hook_continue));
    registry.register(Arc::new(hook_stop));

    let cx = HookCtx {
        session_id: "test".to_string(),
        event_tx: tokio::sync::mpsc::channel(1).0,
        hook_registry: HookRegistry::new(),
    };

    let outcome =
        registry.invoke(HookPoint::AgentEnd, &HookEvent::Other(serde_json::json!({})), &cx).await;

    match outcome {
        HookOutcome::StopLoop { reason } => assert_eq!(reason, "stop"),
        other => panic!("Expected StopLoop, got {:?}", other),
    }
}

#[tokio::test]
async fn test_merge_continue_loop_wins_over_continue() {
    let mut registry = HookRegistry::new();
    let hook_continue = MockHook::new(vec![HookPoint::AgentEnd], HookOutcome::Continue);
    let hook_continue_loop = MockHook::new(
        vec![HookPoint::AgentEnd],
        HookOutcome::ContinueLoop { message: "next".to_string() },
    );
    registry.register(Arc::new(hook_continue));
    registry.register(Arc::new(hook_continue_loop));

    let cx = HookCtx {
        session_id: "test".to_string(),
        event_tx: tokio::sync::mpsc::channel(1).0,
        hook_registry: HookRegistry::new(),
    };

    let outcome =
        registry.invoke(HookPoint::AgentEnd, &HookEvent::Other(serde_json::json!({})), &cx).await;

    match outcome {
        HookOutcome::ContinueLoop { message } => assert_eq!(message, "next"),
        other => panic!("Expected ContinueLoop, got {:?}", other),
    }
}

#[tokio::test]
async fn test_merge_all_hooks_priority() {
    // Every rung of the ladder at once: StopLoop > ContinueLoop > Continue.
    let mut registry = HookRegistry::new();
    registry.register(Arc::new(MockHook::new(vec![HookPoint::AgentEnd], HookOutcome::Continue)));
    registry.register(Arc::new(MockHook::new(
        vec![HookPoint::AgentEnd],
        HookOutcome::ContinueLoop { message: "continue".to_string() },
    )));
    registry.register(Arc::new(MockHook::new(
        vec![HookPoint::AgentEnd],
        HookOutcome::StopLoop { reason: "stop".to_string() },
    )));

    let cx = HookCtx {
        session_id: "test".to_string(),
        event_tx: tokio::sync::mpsc::channel(1).0,
        hook_registry: HookRegistry::new(),
    };

    let outcome =
        registry.invoke(HookPoint::AgentEnd, &HookEvent::Other(serde_json::json!({})), &cx).await;

    match outcome {
        HookOutcome::StopLoop { reason } => assert_eq!(reason, "stop"),
        other => panic!("Expected StopLoop, got {:?}", other),
    }
}

/// Two hooks steering the same loop must both be heard.
///
/// Self-continuation steers on every iteration of a running loop and is registered first, so
/// "keep the first" meant context-aware termination's messages — the compaction nudge, and the
/// notice saying why a run had just ended — were discarded on every single iteration.
#[tokio::test]
async fn two_steering_hooks_are_both_delivered_in_order() {
    let mut registry = HookRegistry::new();
    registry.register(Arc::new(MockHook::new(
        vec![HookPoint::AgentEnd],
        HookOutcome::ContinueLoop { message: "keep building".to_string() },
    )));
    registry.register(Arc::new(MockHook::new(
        vec![HookPoint::AgentEnd],
        HookOutcome::ContinueLoop { message: "context is nearly full".to_string() },
    )));

    let cx = HookCtx {
        session_id: "test".to_string(),
        event_tx: tokio::sync::mpsc::channel(1).0,
        hook_registry: HookRegistry::new(),
    };
    let outcome =
        registry.invoke(HookPoint::AgentEnd, &HookEvent::Other(serde_json::json!({})), &cx).await;

    match outcome {
        HookOutcome::ContinueLoop { message } => {
            assert!(message.contains("keep building"), "{message}");
            assert!(message.contains("context is nearly full"), "{message}");
            assert!(
                message.find("keep building") < message.find("context is nearly full"),
                "registration order is preserved: {message}"
            );
        }
        other => panic!("expected ContinueLoop, got {other:?}"),
    }
}

/// Two hooks feeding the model at the same point must both be heard, for the same reason.
///
/// `AfterToolUse` carries a user's own command hooks, a linter's output and the plan reminder,
/// and only one outcome reaches dispatch — so "keep the first" silently threw away whichever
/// happened to be registered second. They are unrelated by construction; there is nothing to
/// choose between them.
#[tokio::test]
async fn two_hooks_feeding_the_model_are_both_delivered_in_order() {
    let mut registry = HookRegistry::new();
    for note in ["lint: line 3 is unused", "Plan check: 10 tool calls since"] {
        registry.register(Arc::new(MockHook::new(
            vec![HookPoint::AfterToolUse],
            HookOutcome::InjectContext(note.to_string()),
        )));
    }

    let cx = HookCtx {
        session_id: "test".to_string(),
        event_tx: tokio::sync::mpsc::channel(1).0,
        hook_registry: HookRegistry::new(),
    };
    let outcome = registry
        .invoke(HookPoint::AfterToolUse, &HookEvent::Other(serde_json::json!({})), &cx)
        .await;

    match outcome {
        HookOutcome::InjectContext(context) => {
            assert!(context.contains("line 3 is unused"), "{context}");
            assert!(context.contains("Plan check"), "{context}");
            assert!(
                context.find("lint") < context.find("Plan check"),
                "registration order is preserved: {context}"
            );
        }
        other => panic!("expected InjectContext, got {other:?}"),
    }
}

/// ...and a stop still outranks both of them.
#[tokio::test]
async fn a_stop_still_beats_two_steers() {
    let mut registry = HookRegistry::new();
    for message in ["one", "two"] {
        registry.register(Arc::new(MockHook::new(
            vec![HookPoint::AgentEnd],
            HookOutcome::ContinueLoop { message: message.to_string() },
        )));
    }
    registry.register(Arc::new(MockHook::new(
        vec![HookPoint::AgentEnd],
        HookOutcome::StopLoop { reason: "done".to_string() },
    )));

    let cx = HookCtx {
        session_id: "test".to_string(),
        event_tx: tokio::sync::mpsc::channel(1).0,
        hook_registry: HookRegistry::new(),
    };
    let outcome =
        registry.invoke(HookPoint::AgentEnd, &HookEvent::Other(serde_json::json!({})), &cx).await;
    assert!(matches!(outcome, HookOutcome::StopLoop { .. }));
}
