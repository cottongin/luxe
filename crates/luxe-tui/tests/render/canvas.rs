//! Canvas — theme colors, prose width and alignment, and the borderless conversation.

use super::*;

/// Test that theme styles actually influence rendered colors (P18 gate).
#[test]
fn test_theme_styles_affect_rendered_colors() {
    let tool = card_with_arguments("bash", "ls", serde_json::json!({"command": "ls"}));
    let convo = vec![
        ConvoBlock::User { text: "user".to_string(), attached: Vec::new(), chips: Vec::new() },
        ConvoBlock::Assistant("assistant".to_string()),
        ConvoBlock::Tool(Box::new(tool)),
        ConvoBlock::System("system".to_string()),
    ];

    let backend_a = test_backend(80, 24);
    let mut app_a = create_app_with_theme("dark");
    for block in &convo {
        app_a.push_convo(block.clone());
    }
    let buffer_a = render_app(&mut app_a, &backend_a);

    let backend_b = test_backend(80, 24);
    let mut app_b = create_app_with_light_theme();
    for block in convo {
        app_b.push_convo(block);
    }
    let buffer_b = render_app(&mut app_b, &backend_b);

    let buffers_differ = buffer_a
        .content
        .iter()
        .zip(buffer_b.content.iter())
        .any(|(a, b)| a.fg != b.fg || a.bg != b.bg);
    assert!(buffers_differ, "Dark and light themes should produce different rendered colors");
    assert_eq!(buffer_b[(0, 0)].bg, ratatui::style::Color::Rgb(0xef, 0xf1, 0xf5));
}

#[test]
fn light_theme_assistant_markdown_uses_the_configured_text_foreground() {
    let (width, height) = (80, 16);
    let backend = test_backend(width, height);
    let mut app = create_app_with_light_theme();
    app.push_convo(ConvoBlock::Assistant("LIGHT_THEME_PROSE".into()));

    let buffer = render_app(&mut app, &backend);
    let (x, y) = marker_position(&buffer, "LIGHT_THEME_PROSE", width, height);
    assert_eq!(buffer[(x, y)].fg, ratatui::style::Color::Rgb(0x4c, 0x4f, 0x69));
}

#[test]
fn configured_prose_width_centers_assistant_markdown_without_narrowing_code() {
    let (width, height) = (200, 18);
    let backend = test_backend(width, height);
    let mut app = create_app();
    app.config.conversation.prose_max_width = Some(72);
    app.config.conversation.alignment = Some(ConversationAlignment::Center);
    app.push_convo(ConvoBlock::Assistant(
        "CONFIGURED_PROSE\n\n```rust\nCONFIGURED_CODE\n```".to_string(),
    ));

    let buffer = render_app(&mut app, &backend);
    assert_eq!(marker_position(&buffer, "CONFIGURED_PROSE", width, height).0, 62);
    assert_eq!(marker_position(&buffer, "CONFIGURED_CODE", width, height).0, 28);
}

#[test]
fn markdown_prose_and_code_use_distinct_conversation_canvases_when_centered() {
    // Centering splits the slack of the *padded* content region, so a capped block sits one cell
    // left of the band's own center; an uncapped one still hugs the gutter.
    for (width, prose_x, wide_x) in [(200, 38, 28), (140, 8, 1), (78, 1, 1)] {
        let backend = test_backend(width, 18);
        let mut app = create_app();
        app.config.conversation.alignment = Some(ConversationAlignment::Center);
        app.push_convo(ConvoBlock::Assistant(
            "PROSE_CANVAS\n\n```rust\nWIDE_CANVAS\n```".to_string(),
        ));

        let buffer = render_app(&mut app, &backend);
        let prose = marker_position(&buffer, "PROSE_CANVAS", width, 18);
        let wide = marker_position(&buffer, "WIDE_CANVAS", width, 18);
        assert_eq!(prose.0, prose_x, "prose x at width {width}");
        assert_eq!(wide.0, wide_x, "wide x at width {width}");
        assert_eq!(buffer[(width - 1, prose.1)].symbol(), " ");
        assert_eq!(buffer[(width - 1, wide.1)].symbol(), " ");
    }
}

#[test]
fn default_full_alignment_fills_the_width_from_the_left_gutter() {
    // The default policy fills the pane: prose and code share the left gutter and neither is
    // capped, so a wide terminal isn't left mostly empty.
    let width = 200;
    let backend = test_backend(width, 18);
    let mut app = create_app();
    app.push_convo(ConvoBlock::Assistant("PROSE_CANVAS\n\n```rust\nWIDE_CANVAS\n```".to_string()));

    let buffer = render_app(&mut app, &backend);
    let prose = marker_position(&buffer, "PROSE_CANVAS", width, 18);
    let wide = marker_position(&buffer, "WIDE_CANVAS", width, 18);
    assert_eq!(prose.0, wide.0, "prose and code share the left edge");
    assert!(prose.0 <= 2, "content hugs the left gutter, got x={}", prose.0);

    // And `left` remains available for anyone who wants the readable caps back.
    let mut capped = create_app_with_config(Config {
        conversation: luxe_core::config::ConversationConfig {
            alignment: Some(ConversationAlignment::Left),
            ..Default::default()
        },
        ..Default::default()
    });
    capped.push_convo(ConvoBlock::Assistant("PROSE_CANVAS".to_string()));
    let buffer = render_app(&mut capped, &backend);
    assert!(marker_position(&buffer, "PROSE_CANVAS", width, 18).0 <= 2);
}

/// Text used to run flush into the right edge of a band: the canvas wrapped to the band's full
/// width, so a hard-wrapped row's last character landed on the band's last cell.
#[test]
fn banded_blocks_keep_a_cell_of_padding_at_both_edges() {
    let (width, height) = (60u16, 24u16);
    let backend = test_backend(width, height);
    let mut app = create_app();
    // Unbroken tokens, so every row but the last is filled to the wrap limit exactly — word wrap
    // would leave a ragged edge and prove nothing.
    app.push_convo(ConvoBlock::User {
        text: "u".repeat(180),
        attached: Vec::new(),
        chips: Vec::new(),
    });
    let mut activity = ToolActivity::running("session-a", "call-a", "bash", "b".repeat(140), 1);
    activity.state = ToolActivityState::Succeeded;
    activity.elapsed_ms = Some(1_240);
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));

    let buffer = render_app(&mut app, &backend);
    let bands = [
        app.theme_styles.user_msg_bg.bg.expect("a user band color"),
        app.theme_styles.tool_call_success_bg.bg.expect("a tool band color"),
    ];
    let mut filled_rows = 0;
    for y in 0..height {
        let Some(right_edge) = (0..width).rev().find(|&x| bands.contains(&buffer[(x, y)].bg))
        else {
            continue;
        };
        let Some(left_edge) = (0..width).find(|&x| bands.contains(&buffer[(x, y)].bg)) else {
            continue;
        };
        let Some(last_text) =
            (left_edge..=right_edge).rev().find(|&x| buffer[(x, y)].symbol() != " ")
        else {
            continue; // the band's own blank pad row, above and below the text
        };
        assert!(
            last_text < right_edge,
            "row {y} runs flush into the band's right edge at x={right_edge}"
        );
        assert!(
            buffer[(left_edge, y)].symbol() == " ",
            "row {y} runs flush into the band's left edge at x={left_edge}"
        );
        if last_text + 1 == right_edge {
            filled_rows += 1;
        }
    }
    assert!(filled_rows >= 2, "the fixture must actually fill rows to the wrap limit");
}

pub(super) fn marker_position(
    buffer: &Buffer,
    marker: &str,
    width: u16,
    height: u16,
) -> (u16, u16) {
    for y in 0..height {
        if let Some(x) = row_text(buffer, y, width).find(marker) {
            return (x as u16, y);
        }
    }
    panic!("missing marker {marker:?}");
}

/// Helper: dump a buffer row into a String of cell.symbol()s.
pub(super) fn row_text(buffer: &Buffer, y: u16, width: u16) -> String {
    (0..width).map(|x| buffer[(x, y)].symbol()).collect()
}

/// A long conversation stays virtualized without reintroducing a frame.
#[test]
fn test_conversation_is_borderless_and_separate_from_composer() {
    let (w, h) = (40u16, 8u16);
    let backend = test_backend(w, h);
    let mut app = create_app();
    app.status.model = "gpt-4o".into();
    for i in 0..30 {
        app.push_convo(ConvoBlock::Assistant(format!("line number {i}")));
    }
    let buffer = render_app(&mut app, &backend);

    // Virtualized window follows the bottom, so the newest line is visible (exact top row depends
    // on inter-message spacing).
    let full = (0..h).map(|y| row_text(&buffer, y, w)).collect::<Vec<_>>().join("\n");
    assert!(full.contains("line number 29"), "latest conversation window missing: {full:?}");
    let top = row_text(&buffer, 0, w);
    assert_eq!(buffer[(0, 0)].symbol(), " ", "conversation should have no left border");
    assert!(!top.contains("Conversation"));

    assert!(row_text(&buffer, h - 4, w).trim_start().starts_with('╭'), "composer outline missing");
    let activity = row_text(&buffer, h - 5, w);
    assert!(activity.trim().ends_with("build"), "the mode button leads the row: {activity:?}");
    assert!(row_text(&buffer, h - 6, w).trim().is_empty(), "conversation gap missing");
    assert!(
        row_text(&buffer, h - 2, w).trim_start().starts_with('╰'),
        "composer should touch statusline"
    );
    // The flat bar carries the model on its bottom row (mode label is gone).
    assert!(row_text(&buffer, h - 1, w).contains("gpt-4o"));
}
