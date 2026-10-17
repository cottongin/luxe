//! The folded tool run — a run of lookups drawn as one line, with the calls it stands for.

use super::*;

/// A run of lookups folded into one live line, mid-batch, beside the cards that never fold.
pub(super) fn seed_tool_run(app: &mut App) {
    // Seven calls in one message — a command, a checklist move, then a run of five lookups
    // — caught on the sixth. Dispatch is strictly sequential, so exactly one call is ever in
    // flight and it is always the last card: everything above it has landed, and the run's
    // own last lookup has not started. Any still showing two spinners is a state the
    // runtime cannot produce.
    let batch = |index: u32| Some(luxe_core::ToolBatch { index, total: 7 });
    let lookup = |call_id: &str, tool: &str, arguments: serde_json::Value, index: u32| {
        let label = luxe_core::describe_call(tool, &arguments, "/work/luxe");
        let mut activity = ToolActivity::running("fixture-session", call_id, tool, label, 1);
        activity.state = ToolActivityState::Succeeded;
        activity.summary = Some("Completed".into());
        activity.elapsed_ms = Some(3 + u64::from(index));
        // As the live projector builds it: the arguments travel with every card.
        activity.input_preview = Some(arguments.to_string());
        activity.output_preview = Some("…".into());
        activity.batch = batch(index + 2);
        // The message asked for five adjacent lookups, so each of them knows it is in a
        // run — which is what lets the run be one line from its first card rather than
        // three cards that later collapse. The fifth has not been dispatched yet.
        activity.run = Some(luxe_core::ToolRun { index, total: 5 });
        activity
    };
    app.push_convo(ConvoBlock::Assistant(
        "Running the focused tests, then reading the card renderer and its callers.".into(),
    ));
    // A command is never folded away: which command ran is the thing a reader is watching
    // for, and a count would hide exactly that.
    let mut command = ToolActivity::running(
        "fixture-session",
        "call-bash",
        "bash",
        "cargo nextest run -p luxe-tui",
        1,
    );
    command.state = ToolActivityState::Succeeded;
    command.summary = Some("Completed".into());
    command.elapsed_ms = Some(2_100);
    command.input_preview = Some(r#"{"command":"cargo nextest run -p luxe-tui"}"#.into());
    command.output_preview = Some("184 passed, 0 failed".into());
    command.batch = batch(1);
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity: command,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));
    // Nor is the checklist: the card names the item it moved, which only the result knows.
    let moved = serde_json::json!({
        "action": "update", "activeForm": "", "blockedBy": [], "id": 3,
        "status": "in_progress", "subject": "", "subjects": []
    });
    let mut todo = ToolActivity::running(
        "fixture-session",
        "call-todo",
        "todo",
        luxe_core::describe_call("todo", &moved, "/work/luxe"),
        1,
    );
    todo.state = ToolActivityState::Succeeded;
    todo.summary = Some("Completed".into());
    todo.elapsed_ms = Some(1);
    todo.input_preview = Some(moved.to_string());
    todo.output_preview =
        Some("Updated todo #3 → in_progress: fold a run of lookups into one line".into());
    todo.batch = batch(2);
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity: todo,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));
    for (index, (call_id, tool, arguments)) in [
        (
            "call-read-blocks",
            "read",
            serde_json::json!({"path": "crates/luxe-tui/src/view/blocks.rs"}),
        ),
        (
            "call-read-card",
            "read",
            serde_json::json!({"path": "crates/luxe-tui/src/view/tool_card/mod.rs"}),
        ),
        (
            "call-grep-header",
            "grep",
            serde_json::json!({"pattern": "tool_header_line", "path": "crates"}),
        ),
        (
            "call-read-convo",
            "read",
            serde_json::json!({"path": "crates/luxe-tui/src/app/conversation.rs"}),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let index = index as u32 + 1;
        let mut activity = lookup(call_id, tool, arguments, index);
        // The one in flight, and the last card there is: the run's fifth lookup is still
        // hypothetical, which is exactly what lets the line say "6 of 7" already.
        if index == 4 {
            activity.state = ToolActivityState::Running;
            activity.summary = None;
            activity.elapsed_ms = None;
        }
        app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
            activity,
            expanded: false,
            details_toggled: false,
            waited_on_user_ms: 0,
        })));
    }
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
    app.status.active_tool = Some("read".into());
    app.status.state = crate::app::AppState::Streaming;
}
