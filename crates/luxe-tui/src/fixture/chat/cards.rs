//! Chat card fixtures — tool cards in each of their states, their bodies, and a failed call.

use super::*;

/// Chat with deterministic structured tool outcomes.
pub(super) fn seed_tool_cards(app: &mut App) {
    let running = ToolActivity::running(
        "fixture-session",
        "call-read",
        "read",
        "crates/luxe-tui/src/view.rs",
        1,
    );
    let mut failed =
        ToolActivity::running("fixture-session", "call-test", "bash", "cargo test -p luxe-tui", 2);
    failed.revision = 1;
    failed.state = ToolActivityState::Failed;
    failed.elapsed_ms = Some(1_240);
    failed.summary = Some("validation failed".into());
    failed.input_preview = Some("--workspace --all-targets".into());
    failed.output_preview = Some("test target failed after compiling luxe-tui".into());
    app.push_convo(ConvoBlock::Assistant("Checking the focused TUI slice.".into()));
    for (index, (call_id, path)) in [
        ("call-read-cargo", "Cargo.toml"),
        ("call-read-app", "crates/luxe-tui/src/app.rs"),
        ("call-read-view", "crates/luxe-tui/src/view.rs"),
    ]
    .into_iter()
    .enumerate()
    {
        let mut completed = ToolActivity::running("fixture-session", call_id, "read", path, 1);
        completed.state = ToolActivityState::Succeeded;
        completed.summary = Some("Completed".into());
        // One message asked for all three, which is what folds them into a line.
        let index = u32::try_from(index).unwrap_or(0) + 1;
        completed.run = Some(luxe_core::ToolRun { index, total: 3 });
        app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
            activity: completed,
            expanded: false,
            details_toggled: false,
            waited_on_user_ms: 0,
        })));
    }
    // An edit card renders its precise diff inline (colored +/-), independent of expand.
    let mut edited = ToolActivity::running(
        "fixture-session",
        "call-edit",
        "edit",
        "crates/luxe-tui/src/view/dock.rs",
        1,
    );
    edited.state = ToolActivityState::Succeeded;
    edited.summary = Some("Completed".into());
    edited.elapsed_ms = Some(6);
    edited.output_preview = Some(
        "Applied 1 edit(s) to 'crates/luxe-tui/src/view/dock.rs'\n\n\
         \u{20}    ...\n\
         \u{20}734 fn section_heading_line(section: DockSection, app: &App) -> Line<'static> {\n\
         \u{20}735     let styles = &app.theme_styles;\n\
         -736     let selected = app.dock.focused && app.dock.selected_section == Some(section);\n\
         +736     let collapsed = section.is_collapsible() && app.dock.collapsed.contains(&section);\n\
         \u{20}737     let marker = if collapsed { \"▸\" } else { \"▾\" };\n\
         \u{20}    ..."
            .into(),
    );
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity: edited,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));
    // A skill card carries the theme's secondary/purple accent as an activation cue.
    let mut skill =
        ToolActivity::running("fixture-session", "call-skill", "skill", "frontend-slides", 1);
    skill.state = ToolActivityState::Succeeded;
    skill.summary = Some("Completed".into());
    skill.elapsed_ms = Some(3);
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity: skill,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity: failed,
        expanded: true,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));
    // The call in flight goes last, because it is the only one there can be: dispatch runs
    // them one at a time, so every card above a spinner has already landed.
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity: running,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));
    // The three reads fold, settled: a finished run is a count and a duration and no meter,
    // because there is no progress left to report. `chat-tool-run` catches one mid-flight.
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
}

/// Chat with the three cards that carry a body: a written file, a side-by-side edit, a heredoc.
pub(super) fn seed_card_bodies(app: &mut App) -> Result<(), String> {
    app.push_convo(ConvoBlock::Assistant(
        "Wiring the card body renderer, then checking it against a real terminal.".into(),
    ));
    // A body sourced from the call's arguments, exactly as the live projection builds it.
    let from_arguments = |call_id: &str, tool: &str, arguments: serde_json::Value| {
        let label = luxe_core::describe_call(tool, &arguments, "/work/luxe");
        let mut activity = ToolActivity::running("fixture-session", call_id, tool, label, 1);
        activity.state = ToolActivityState::Succeeded;
        activity.summary = Some("Completed".into());
        activity.elapsed_ms = Some(4);
        activity.card_body = luxe_core::tool_card_body(tool, &arguments, "/work/luxe");
        activity
    };

    // Long enough that the preview budget bites, so the truncation footer is on screen.
    let script = "#!/usr/bin/env bash\n\
        # Renders tool-glyph candidates so they can be eyeballed in a real terminal font.\n\
        set -euo pipefail\n\
        \n\
        row() { printf ' %-14s %s\\n' \"$1\" \"$2\"; }\n\
        \n\
        glyph_for() {\n\
        \u{20}   case \"$1\" in\n\
        \u{20}   bash) printf '\\ue760' ;;\n\
        \u{20}   read) printf '\\ue245' ;;\n\
        \u{20}   *) printf '?' ;;\n\
        \u{20}   esac\n\
        }\n\
        \n\
        for name in bash read grep glob git write edit; do\n\
        \u{20}   row \"$name\" \"$(glyph_for \"$name\")\"\n\
        done\n\
        echo\n\
        row \"total\" \"$(wc -l < candidates.txt)\"\n\
        exit 0\n";
    let mut write = from_arguments(
        "call-write",
        "write",
        serde_json::json!({"path": "scripts/glyph_sampler.sh", "content": script}),
    );
    write.output_preview = Some("Created 'scripts/glyph_sampler.sh' (498 bytes)".into());
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity: write,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));

    // An edit renders its *result*: only the tool knows where the change landed.
    let mut edited = ToolActivity::running(
        "fixture-session",
        "call-edit",
        "edit",
        "crates/luxe-tui/src/view/blocks.rs",
        1,
    );
    edited.state = ToolActivityState::Succeeded;
    edited.summary = Some("Completed".into());
    edited.elapsed_ms = Some(6);
    edited.output_preview = Some(
        "Applied 1 edit(s) to 'crates/luxe-tui/src/view/blocks.rs'\n\n\
         \u{20}    ...\n\
         \u{20}128 let summary = activity.summary.as_deref();\n\
         -129 let is_diff_tool = matches!(activity.name.as_str(), \"edit\" | \"write\");\n\
         -130 if is_diff_tool && succeeded {\n\
         +129 let outcome = tool_card::header_summary(activity);\n\
         +130 if let Some(body) = tool_card::body(item, &ctx.card()) {\n\
         \u{20}131     lines.extend(body.into_iter().map(indented));\n\
         \u{20}132 }\n\
         \u{20}    ..."
            .into(),
    );
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity: edited,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));

    // A one-line command says everything on its header, so this one has no body.
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity: from_arguments(
            "call-bash-one",
            "bash",
            serde_json::json!({"command": "cargo nextest run -p luxe-tui"}),
        ),
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));
    // A search's payload is the angles it covered, not `queries=[6]`.
    // Six is past the list's own budget, so the fold and its footer are on screen too.
    let mut searched = from_arguments(
        "call-web-search",
        "web_search",
        serde_json::json!({"queries": [
            "ratatui paragraph wrapping trailing spans",
            "terminal cell width unicode east asian",
            "crossterm mouse capture bracketed paste",
            "kitty graphics protocol unicode placeholders",
            "tui scrollback performance large buffers",
            "ansi truecolor fallback 256 palette",
        ]}),
    );
    searched.elapsed_ms = Some(1_512);
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity: searched,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));
    // A heredoc does not, and must not vanish behind the header's ellipsis.
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity: from_arguments(
            "call-bash-heredoc",
            "bash",
            serde_json::json!({"command": "cd /tmp && python3 - <<'PY'\nimport json\nrows = [(\"bash\", \"\\ue760\"), (\"read\", \"\\ue245\")]\nprint(json.dumps(dict(rows), indent=2))\nPY"}),
        ),
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));
    // Described, a chained command is named by what it does, with the command folded to
    // its first line underneath
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity: from_arguments(
            "call-bash-described",
            "bash",
            serde_json::json!({
                "description": "Check which fixtures still render the old dock",
                "command": "for f in crates/luxe-tui/examples/*.rs; do\n  grep -l 'CHANGES' \"$f\" || true\ndone | sort",
            }),
        ),
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));
    // A failing suite through the shell reads as its verdict and what failed
    let mut suite = from_arguments(
        "call-bash-suite",
        "bash",
        serde_json::json!({
            "description": "Run the forms tests",
            "command": "cd tests && ../.venv/bin/python runtests.py forms_tests 2>&1",
        }),
    );
    suite.state = ToolActivityState::Failed;
    suite.output_preview = Some(
        "unittest: FAILED — 412 passed, 1 failed, 3 skipped\nFailures:\n  ✗ test_deepcopy (forms_tests.field_tests.test_base.BasicFieldsTests)"
            .into(),
    );
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity: suite,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
    Ok(())
}

/// Chat with an expanded failed tool for error-state review.
pub(super) fn seed_tool_error(app: &mut App) {
    // Two rejections worth recognizing on sight: a guard that fired (nothing was written;
    // the model has to read first) and an edit whose anchor wasn't unique. Both are
    // recoverable, and neither may render as a bare glyph plus a duration.
    let mut guarded = ToolActivity::running(
        "fixture-session",
        "call-edit-guard",
        "edit",
        "crates/luxe-core/src/event.rs",
        1,
    );
    guarded.revision = 1;
    guarded.state = ToolActivityState::Failed;
    guarded.elapsed_ms = Some(0);
    guarded.summary = Some("edited before reading the file".into());
    guarded.input_preview = Some("replace Event::ToolStarted exactly once".into());
    guarded.output_preview = Some(
        "Error: read 'crates/luxe-core/src/event.rs' before editing it \
         (read-before-edit guard). Use the `read` tool first, then retry."
            .into(),
    );
    let mut failed = ToolActivity::running(
        "fixture-session",
        "call-edit-error",
        "edit",
        "crates/luxe-core/src/event.rs",
        1,
    );
    failed.revision = 1;
    failed.state = ToolActivityState::Failed;
    failed.elapsed_ms = Some(184);
    // As the projector builds it: the tool's own first line, cut to the card's budget.
    failed.summary = Some(
        "edit[0] 'oldText' matched 2 times (must be unique). Add more context to make \
         it unique."
            .into(),
    );
    failed.input_preview = Some("replace Event::ToolStarted exactly once".into());
    failed.output_preview = Some(
        "Error: edit[0] 'oldText' matched 2 times (must be unique). Add more context \
         to make it unique."
            .into(),
    );
    app.push_convo(ConvoBlock::Assistant(
        "The focused edit needs a more specific replacement before retrying.".into(),
    ));
    for activity in [guarded, failed] {
        app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
            activity,
            expanded: true,
            details_toggled: false,
            waited_on_user_ms: 0,
        })));
    }
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
}
