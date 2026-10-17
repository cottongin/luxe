//! Folds — a run of calls as one line, and what it counts and meters.

use super::*;

pub(super) fn group_header_text(items: &[ConversationItem], arriving: bool) -> String {
    tool_group_lines(items, arriving, 0, 120, &theme())[0].line.to_string()
}

pub(super) fn group_text(items: &[ConversationItem], width: u16) -> Vec<String> {
    tool_group_lines(items, false, 0, width, &theme())
        .into_iter()
        .map(|row| row.line.to_string())
        .collect()
}

/// A settled call of `tool`, labeled `label`, as a transcript item.
pub(super) fn labeled_call(tool: &str, label: &str, call_id: &str) -> ConversationItem {
    let mut activity = luxe_core::ToolActivity::running("session-a", call_id, tool, label, 1);
    activity.state = luxe_core::ToolActivityState::Succeeded;
    activity.summary = Some("Completed".into());
    ConversationItem::new(
        crate::app::ConversationItemId::Tool {
            session_id: "session-a".into(),
            call_id: call_id.into(),
        },
        ConvoBlock::Tool(Box::new(crate::app::ToolItem {
            activity,
            expanded: false,
            details_toggled: false,
            waited_on_user_ms: 0,
        })),
    )
}

/// A folded run says what it stands for, not only how much: the calls by name under the count,
/// all of them when there are five or fewer.
#[test]
fn a_folded_run_lists_what_it_folded() {
    let run = [
        labeled_call("read", "note.md", "a"),
        labeled_call("read", "scripts/demo.py:12-40", "b"),
        labeled_call("read", "tests/example.py", "c"),
    ];
    let rows = group_text(&run, 120);
    assert_eq!(
        rows[1..],
        [
            format!("{BODY_INDENT}note.md"),
            format!("{BODY_INDENT}scripts/demo.py:12-40"),
            format!("{BODY_INDENT}tests/example.py"),
        ],
        "{rows:#?}"
    );
    let rows = tool_group_lines(&run, false, 0, 120, &theme());
    assert!(rows[0].handle, "the header opens the run");
    assert!(rows[1..].iter().all(|row| !row.handle), "the list is text to select");
}

/// Past five, four by name and a count of the rest, which says how to see them.
#[test]
fn a_long_folded_run_counts_the_calls_it_does_not_list() {
    let run: Vec<ConversationItem> = (0..9)
        .map(|index| labeled_call("grep", &format!("\"pattern{index}\""), &index.to_string()))
        .collect();
    let rows = group_text(&run, 120);
    assert_eq!(rows.len(), 1 + 4 + 1, "{rows:#?}");
    assert!(rows[4].ends_with("\"pattern3\""));
    assert_eq!(rows[5], format!("{BODY_INDENT}⋯ 5 more · 9 total · click to expand"));
    let last = tool_group_lines(&run, false, 0, 120, &theme()).pop().expect("a footer");
    assert!(last.handle, "the footer offers the expand, so a click on it has to open it");

    let five = &run[..5];
    assert_eq!(group_text(five, 120).len(), 6, "five fit whole: no '1 more'");
}

/// A mixed run names each call's tool, and a row too long for the card is cut, not wrapped.
#[test]
fn a_mixed_run_names_each_tool_and_cuts_long_rows() {
    let run = [
        labeled_call("read", "src/a/very/long/path/to/some/file/that/goes/on.rs", "a"),
        labeled_call("grep", "\"needle\" in src", "b"),
        labeled_call("glob", "**/*.rs", "c"),
    ];
    let rows = group_text(&run, 30);
    assert_eq!(rows[2], format!("{BODY_INDENT}grep  \"needle\" in src"));
    assert!(rows[1].ends_with('…'), "{rows:#?}");
    assert!(rows.iter().all(|row| row.chars().count() <= 30), "{rows:#?}");
}

/// An MCP run names its server once, on the header, and each call by its tool.
#[test]
fn an_mcp_run_names_its_server_once() {
    let run = [
        labeled_call("mcp", "search tasks", "a"),
        labeled_call("mcp", "asana/get_tasks", "b"),
        labeled_call("mcp", "asana/update_task", "c"),
    ];
    let rows = group_text(&run, 120);
    assert_eq!(rows[0], format!("▸ {} 3 mcp calls · asana", tool_glyph("mcp")));
    assert_eq!(
        rows[1..],
        [
            format!("{BODY_INDENT}search tasks"),
            format!("{BODY_INDENT}get_tasks"),
            format!("{BODY_INDENT}update_task"),
        ]
    );
}

/// A collapsed run of calls says which tool it collapsed; the wash already says they all
/// succeeded.
#[test]
fn a_collapsed_group_leads_with_its_tool_icon() {
    let item = |name: &str| {
        ConversationItem::new(
            crate::app::ConversationItemId::Tool {
                session_id: "session-a".into(),
                call_id: name.into(),
            },
            tool_item(name, luxe_core::ToolActivityState::Succeeded),
        )
    };

    // The run's time is its calls' time added up: what folding them cost, in one number.
    let same = [item("read"), item("read"), item("read")];
    let line = group_header_text(&same, false);
    assert_eq!(line, format!("▸ {} 3 read calls  2s", tool_glyph("read")));

    // A mixed run has no one tool to name, so it takes the generic icon — and a word for what
    // the run actually was, since only lookups ever fold.
    let mixed = [item("read"), item("grep"), item("read")];
    let line = group_header_text(&mixed, false);
    assert_eq!(line, format!("▸ {} 3 lookups  2s", GENERIC_TOOL.glyph()));
}

/// A run that is still moving is a progress line, and one that has stopped is not.
///
/// The meter is a claim that something is happening. Left on a settled run it would be a
/// picture of work that already finished, which is the one thing a progress bar must never be.
#[test]
fn a_folded_run_meters_itself_only_while_it_is_moving() {
    let item = |call_id: &str, state: luxe_core::ToolActivityState, batch| {
        let mut activity =
            luxe_core::ToolActivity::running("session-a", call_id, "read", call_id, 1);
        activity.state = state;
        activity.batch = batch;
        if state == luxe_core::ToolActivityState::Succeeded {
            activity.elapsed_ms = Some(4);
            activity.summary = Some("Completed".into());
        }
        ConversationItem::new(
            crate::app::ConversationItemId::Tool {
                session_id: "session-a".into(),
                call_id: call_id.into(),
            },
            ConvoBlock::Tool(Box::new(crate::app::ToolItem {
                activity,
                expanded: false,
                details_toggled: false,
                waited_on_user_ms: 0,
            })),
        )
    };
    let batch = |index| Some(luxe_core::ToolBatch { index, total: 7 });

    let live = [
        item("a.rs", luxe_core::ToolActivityState::Succeeded, batch(1)),
        item("b.rs", luxe_core::ToolActivityState::Succeeded, batch(2)),
        item("c.rs", luxe_core::ToolActivityState::Running, batch(3)),
    ];
    let line = group_header_text(&live, true);
    assert!(line.contains("3/7"), "the run says how much of it is left: {line}");
    assert!(line.contains("read"), "named for the call in flight: {line}");
    assert!(line.contains("c.rs"), "and what it is doing right now: {line}");
    assert!(line.contains('█') || line.contains('▁'), "with a meter: {line}");

    let settled = [
        item("a.rs", luxe_core::ToolActivityState::Succeeded, batch(1)),
        item("b.rs", luxe_core::ToolActivityState::Succeeded, batch(2)),
        item("c.rs", luxe_core::ToolActivityState::Succeeded, batch(3)),
    ];
    let line = group_header_text(&settled, false);
    assert_eq!(line, format!("▸ {} 3 read calls  12ms", tool_glyph("read")));
    assert!(!line.contains('█') && !line.contains('▁'), "no meter once it stopped: {line}");

    // Between two of its calls the run has nothing in flight and is nowhere near done. Reading
    // that as settled summarized the run as "3 read calls" for a frame and washed the band
    // green — once per call, all the way down a run of seven.
    let between = &settled[..2];
    let line = group_header_text(between, true);
    assert!(line.contains("2/7"), "still a progress line: {line}");
    assert!(run_is_working(between, true), "and still wearing the working band");
    assert!(!run_is_working(&settled, false), "which it gives up when the run is done");
}

/// Both ends of the scale have to read as time, not as a raw counter.
#[test]
fn tool_durations_are_formatted_at_a_readable_scale() {
    // Sub-millisecond calls really do happen (a small read, a rejected edit); `0ms` reads as
    // "never ran", which is exactly the wrong conclusion.
    assert_eq!(format_tool_duration(0), "<1ms");
    assert_eq!(format_tool_duration(1), "1ms");
    assert_eq!(format_tool_duration(842), "842ms");
    assert_eq!(format_tool_duration(1_500), "1s", "a tenths digit moves ten times a second");
    assert_eq!(format_tool_duration(47_608), "47s");
    assert_eq!(format_tool_duration(134_000), "2m 14s");
    assert_eq!(format_tool_duration(3_780_000), "1h 03m");
}

/// A card body sits inset under its header, and a `write` says what it did to the file.
#[test]
fn a_write_card_carries_its_content_under_its_header() {
    let styles = theme();
    let mut activity =
        luxe_core::ToolActivity::running("session-a", "call-a", "write", "notes.rs", 1);
    activity.state = luxe_core::ToolActivityState::Succeeded;
    activity.summary = Some("Completed".into());
    activity.output_preview = Some("Created 'notes.rs' (13 bytes)".into());
    let arguments = serde_json::json!({"path": "notes.rs", "content": "fn main() {}"});
    activity.card_body = luxe_core::tool_card_body("write", &arguments, "/project");
    let block = ConvoBlock::Tool(Box::new(crate::app::ToolItem {
        activity,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    }));

    let rendered = lines(&block, &context(&styles, 0));
    let header = rendered[0].line.to_string();
    assert!(header.contains("write  notes.rs"), "{header}");
    assert!(header.ends_with("· Created · 13 B"), "the header says what it did: {header}");

    let body = rendered[1].line.to_string();
    // Two cells of gutter whatever the file's depth, so a body filling in sits still.
    assert_eq!(body, format!("{BODY_INDENT} 1 fn main() {{}}"));
    assert_eq!(rendered.len(), 2, "no `arguments:`/`output:` rows once a body is shown");
}
