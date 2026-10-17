use super::*;
use crate::fixture::{seeded_app, FixtureScenario};

/// Every slash command the registry lists is one `run_slash` has an arm for.
///
/// The break this catches: a command added to the table, or an id renamed in one place, compiled
/// and did nothing when run.
#[test]
fn every_slash_command_is_one_run_slash_knows() {
    for command in crate::commands::CommandRegistry::all().iter() {
        if command.kind != crate::commands::CommandKind::Slash {
            continue;
        }
        let mut app = seeded_app("nord", FixtureScenario::ChatIdle).expect("fixture");
        assert!(run_slash(&mut app, command), "{} has no arm in run_slash", command.id);
    }
}

/// A slash command nothing handles says so and is not remembered as used.
///
/// The break this catches: `dispatch` counted every slash command as run, so an unknown id closed
/// the menu on nothing and then led the next one as recently used.
#[test]
fn a_slash_command_nothing_handles_says_so() {
    let mut app = seeded_app("nord", FixtureScenario::ChatIdle).expect("fixture");
    let registry = crate::commands::CommandRegistry::all();
    let mut unknown = registry.parse_slash("/cost").expect("a slash command");
    unknown.id = "cmd.unknown".into();

    super::super::dispatch(&mut app, unknown);

    let said = app.status.toast.as_ref().map(|(said, _)| said.as_str());
    assert_eq!(said, Some("Session cost did nothing here"));
    assert!(!app.recent_commands.ids().iter().any(|id| id == "cmd.unknown"));
}
