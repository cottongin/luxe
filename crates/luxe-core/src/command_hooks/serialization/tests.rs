use super::*;
use crate::command_hooks::HookFailureAction;
use crate::hook::HookPoint;

struct UnserializableEvent;

impl Serialize for UnserializableEvent {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("event fixture cannot be encoded"))
    }
}

fn failure_spec(action: HookFailureAction) -> CommandHookSpec {
    CommandHookSpec {
        point: HookPoint::BeforeToolUse,
        matcher: None,
        command: vec![],
        timeout_ms: 5000,
        on_timeout: action,
    }
}

/// A serialization failure must use deny with the encoding error, not attempt an empty-input run.
#[tokio::test]
async fn a_serialization_failure_uses_the_deny_fail_safe() {
    let outcome =
        run_event_hook(&failure_spec(HookFailureAction::Deny), &UnserializableEvent).await;
    match outcome {
        HookOutcome::Deny { reason } => {
            assert!(reason.contains("event fixture cannot be encoded"), "{reason}")
        }
        other => panic!("serialization failure did not deny: {other:?}"),
    }
}

/// A serialization failure must retain the ask policy and its encoding error.
#[tokio::test]
async fn a_serialization_failure_uses_the_ask_fail_safe() {
    let outcome = run_event_hook(&failure_spec(HookFailureAction::Ask), &UnserializableEvent).await;
    match outcome {
        HookOutcome::Ask(reason) => {
            assert!(reason.contains("event fixture cannot be encoded"), "{reason}")
        }
        other => panic!("serialization failure did not ask: {other:?}"),
    }
}

#[test]
#[ignore = "subprocess fixture explicitly run by the serialization failure test"]
fn serialization_marker_child() {
    let marker =
        std::env::var_os("LUXE_SERIALIZATION_TEST_MARKER").expect("parent supplies the marker");
    std::fs::write(marker, "started").unwrap();
}

fn marker_spec() -> (tempfile::TempDir, CommandHookSpec, std::path::PathBuf) {
    let project = tempfile::tempdir().unwrap();
    let marker = project.path().join("serialization-hook-ran");
    std::env::set_var("LUXE_SERIALIZATION_TEST_MARKER", &marker);
    let mut spec = failure_spec(HookFailureAction::Allow);
    spec.point = HookPoint::AgentEnd;
    spec.command = vec![
        crate::paths::display(&std::env::current_exe().unwrap()),
        "--exact".into(),
        "command_hooks::serialization::tests::serialization_marker_child".into(),
        "--ignored".into(),
    ];
    (project, spec, marker)
}

/// Allow skips a failed serialization, but a valid event must still start the same command.
#[tokio::test]
async fn a_serialization_failure_does_not_spawn_even_when_allowed() {
    let (_project, spec, marker) = marker_spec();
    let outcome = run_event_hook(&spec, &UnserializableEvent).await;
    assert!(matches!(outcome, HookOutcome::Continue), "{outcome:?}");
    assert!(!marker.exists(), "a command ran with an unserializable event");
    let valid = run_command_hook(&spec, &HookEvent::Other(serde_json::Value::Null)).await;
    std::env::remove_var("LUXE_SERIALIZATION_TEST_MARKER");
    assert!(matches!(valid, HookOutcome::Continue), "{valid:?}");
    assert!(marker.exists(), "a valid event must still start the configured command");
}
