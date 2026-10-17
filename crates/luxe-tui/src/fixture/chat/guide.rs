//! Guide fixtures — the states the user guide pictures: a finished turn's cards, a permission
//! prompt, the Command Palette and the plan handoff.
//!
//! One small change runs through all four, the `todo overdue` command the site's demos build, so
//! the pictures read as one session rather than four unrelated ones.

use super::*;

/// What the reader asked for, in every scenario here.
const ASK: &str = "add a `todo overdue` command that lists open tasks past their due date, oldest \
                   first, with a test";

/// A finished turn: the lookups folded, an edit with its diff, the tests, and the reply.
pub(super) fn seed_turn(app: &mut App) {
    push_turn(app);
    app.push_convo(ConvoBlock::Assistant(
        "Added `todo overdue`. It lists open tasks past their due date, oldest first, through \
         a new `overdue` in `src/filter.js`, and two tests cover it. All five pass."
            .into(),
    ));
    seed_status(app);
}

/// The same turn, stopped on a commit that waits for approval.
pub(super) fn seed_permission(app: &mut App) {
    push_turn(app);
    app.push_convo(ConvoBlock::Assistant("The suite passes. Committing the change.".into()));
    app.overlays.permission = Some(
        crate::permission_prompt::PermissionPromptState::new(
            luxe_core::permission::PermissionRequestId::new(),
            "git".into(),
            "git commit \u{2014} Add a todo overdue command".into(),
            vec!["git commit*".into()],
        )
        .opened(app.frame_now()),
    );
    seed_status(app);
}

/// The Command Palette open over the conversation, with the commands used last at the top.
pub(super) fn seed_palette(app: &mut App) {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    push_turn(app);
    for id in [crate::commands::id::MODEL, crate::commands::id::DIFF, crate::commands::id::COST] {
        app.recent_commands.record(id);
    }
    seed_status(app);
    app.update(Msg::Key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)));
}

/// Plan mode's plan written, and the question of what to do with it.
pub(super) fn seed_plan_ready(app: &mut App) {
    app.status.mode = luxe_core::mode::PLAN.into();
    app.push_convo(ConvoBlock::User {
        text: "Plan the `todo overdue` command, then the reminders that use it.".into(),
        attached: Vec::new(),
        chips: Vec::new(),
    });
    app.push_convo(ConvoBlock::Tool(Box::new(item(settled(
        "call-write-plan",
        "write",
        serde_json::json!({"path": "PLAN.md", "content": ""}),
        "Created 'PLAN.md' (1.4 KB)",
    )))));
    app.push_convo(ConvoBlock::Assistant(
        "The plan is in `PLAN.md`, in three phases: the `overdue` filter and its command, a \
         daily reminder that reads it, and the docs. Each phase ends with `npm test` passing."
            .into(),
    ));
    app.update(Msg::Engine(Event::PlanWritten { path: "PLAN.md".into() }));
    app.update(Msg::Engine(Event::TurnEnded { reason: luxe_core::StopReason::EndTurn }));
    seed_status(app);
    app.status.model = "claude-opus-5".into();
    app.status.provider = "anthropic".into();
    app.status.thinking = luxe_core::provider::ThinkingLevel::High;
}

/// The ask, three lookups folded into a line, the edit and the test run.
fn push_turn(app: &mut App) {
    app.push_convo(ConvoBlock::User { text: ASK.into(), attached: Vec::new(), chips: Vec::new() });
    app.push_convo(ConvoBlock::Assistant(
        "Reading the command table and the filters first.".into(),
    ));
    for (index, (call_id, path)) in [
        ("call-read-cli", "src/cli.js"),
        ("call-read-filter", "src/filter.js"),
        ("call-read-tests", "test/filter.test.js"),
    ]
    .into_iter()
    .enumerate()
    {
        let mut read = settled(call_id, "read", serde_json::json!({ "path": path }), "…");
        let index = u32::try_from(index).unwrap_or(0) + 1;
        read.run = Some(luxe_core::ToolRun { index, total: 3 });
        app.push_convo(ConvoBlock::Tool(Box::new(item(read))));
    }
    let mut edit = settled("call-edit", "edit", serde_json::json!({ "path": "src/filter.js" }), "");
    edit.output_preview = Some(
        "Applied 1 edit(s) to 'src/filter.js'\n\n\
         \u{20}   ...\n\
         \u{20}19   return tasks.filter((task) => tags.every((tag) => task.tags.includes(tag)));\n\
         \u{20}20 }\n\
         +21 \n\
         +22 /** Open tasks past their due date as of `today`, oldest first. */\n\
         +23 export function overdue(tasks, today) {\n\
         +24   return open(tasks)\n\
         +25     .filter((task) => task.due && task.due < today)\n\
         +26     .sort((a, b) => a.due.localeCompare(b.due));\n\
         +27 }"
            .into(),
    );
    app.push_convo(ConvoBlock::Tool(Box::new(item(edit))));
    let mut test = settled(
        "call-test",
        "test",
        serde_json::json!({ "filter": "test/filter.test.js" }),
        "node: ok — 5 passed, 0 failed, 0 skipped",
    );
    test.elapsed_ms = Some(640);
    app.push_convo(ConvoBlock::Tool(Box::new(item(test))));
}

/// A call that finished, labeled the way the live projection labels it.
fn settled(call_id: &str, tool: &str, arguments: serde_json::Value, output: &str) -> ToolActivity {
    let label = luxe_core::describe_call(tool, &arguments, "/work/todo");
    let mut activity = ToolActivity::running("fixture-session", call_id, tool, label, 1);
    activity.state = ToolActivityState::Succeeded;
    activity.summary = Some("Completed".into());
    activity.elapsed_ms = Some(4);
    activity.input_preview = Some(arguments.to_string());
    activity.output_preview = Some(output.into());
    activity
}

fn item(activity: ToolActivity) -> ToolItem {
    ToolItem { activity, expanded: false, details_toggled: false, waited_on_user_ms: 0 }
}

fn seed_status(app: &mut App) {
    app.status.model = "claude-sonnet-5".into();
    app.status.provider = "anthropic".into();
    app.status.thinking = luxe_core::provider::ThinkingLevel::Medium;
    app.status.context_window = 1_000_000;
    app.status.context_pct = 3.1;
}
