use super::*;
use luxe_core::context_report::{ContextDetail, ContextFinding, ContextPart, McpFootprint};
use luxe_core::theme::ThemeRegistry;

fn styles() -> ThemeStyles {
    let resolved = ThemeRegistry::new().resolve("dark").unwrap();
    ThemeStyles::from_resolved(&resolved, "dark")
}

fn report() -> ContextComposition {
    ContextComposition {
        model: "claude-opus-4-6".into(),
        context_window: 200_000,
        used_tokens: 48_000,
        calibration: Calibration::Measured { chars_per_token: 3.6 },
        slices: vec![
            ContextSlice {
                part: ContextPart::Conversation,
                tokens: 32_000,
                detail: vec![ContextDetail {
                    label: "`read` crates/luxe-tui/src/view.rs".into(),
                    tokens: 12_000,
                }],
            },
            ContextSlice { part: ContextPart::ToolSchemas, tokens: 12_000, detail: Vec::new() },
            ContextSlice { part: ContextPart::Harness, tokens: 4_000, detail: Vec::new() },
        ],
        findings: vec![ContextFinding {
            level: FindingLevel::Warn,
            text: "The window is 76% full. `/compact` frees the tail.".into(),
        }],
        cache_hit_pct: Some(91.0),
        cache_write_pct: Some(3.0),
        mcp: None,
    }
}

fn rendered(report: &ContextComposition) -> String {
    lines(report, &styles())
        .iter()
        .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect::<String>() + "\n")
        .collect()
}

#[test]
fn the_headline_names_the_model_the_total_and_the_share() {
    let text = rendered(&report());
    assert!(text.contains("claude-opus-4-6"), "{text}");
    assert!(text.contains("48.0k of 200.0k"), "{text}");
    assert!(text.contains("24% used"), "{text}");
}

/// Every part is a row, ranked, with what is left set apart from what is occupied.
#[test]
fn every_part_gets_a_row_and_free_space_is_named() {
    let text = rendered(&report());
    for label in ["conversation", "tool schemas", "harness", "free"] {
        assert!(text.contains(label), "missing {label}: {text}");
    }
    let conversation = text.find("conversation").unwrap();
    let harness = text.find("harness").unwrap();
    assert!(conversation < harness, "rows are ranked largest first: {text}");
    assert!(text.contains("152.0k"), "free space is the remainder: {text}");
}

/// A cost figure that cannot be checked is not much of a readout: the surface says how the
/// split was derived, every time.
#[test]
fn the_method_is_always_stated() {
    assert!(rendered(&report()).contains("3.6 per token"));
    let mut estimated = report();
    estimated.calibration = Calibration::Estimated;
    let text = rendered(&estimated);
    assert!(text.contains("Estimated"), "{text}");
    assert!(text.contains("nothing has been billed"), "{text}");
}

/// A detail row is for the outlier. A small one is noise and stays out.
#[test]
fn only_outlying_details_are_named() {
    let text = rendered(&report());
    assert!(text.contains("view.rs"), "12k of a 32k conversation earns its line: {text}");

    let mut small = report();
    small.slices[0].detail[0].tokens = 150;
    assert!(!rendered(&small).contains("view.rs"), "a trace of it does not");
}

/// A detail is judged against its own part, so the same runaway result is named on a 200k model
/// and on a 1.1M one. Judged against the window, it would quietly stop being named on exactly
/// the models where a session runs longest.
#[test]
fn naming_a_detail_does_not_depend_on_the_window_size() {
    let mut huge = report();
    huge.context_window = 1_100_000;
    assert!(rendered(&huge).contains("view.rs"), "a big window hides nothing");

    assert!(worth_naming(32_000, 12_000), "over a third of its part");
    assert!(!worth_naming(32_000, 1_000), "3% of its part is not an outlier");
    // The floor is what stops a barely-started session from naming its only call as if it were
    // a problem; share alone would name this one at 38%.
    assert!(!worth_naming(400, 150), "below the floor, whatever its share");
}

/// Both ends of a detail label identify it, so a long one loses its middle rather than the
/// file name that says which call to narrow.
#[test]
fn a_long_detail_label_keeps_the_tool_and_the_file() {
    let elided = elide_middle("`read` crates/luxe-tui/src/view/guided_review/diff_render.rs", 34);
    assert_eq!(elided.width(), 34, "{elided}");
    assert!(elided.starts_with("`read`"), "{elided}");
    assert!(elided.ends_with("diff_render.rs"), "{elided}");
    assert_eq!(elide_middle("`test`", 34), "`test`", "a short label is left alone");
}

/// The figures are the message, so the rows are columns and nothing is drawn.
///
/// A bar here would be a third rendering of a number the row already prints twice. The rail
/// holds itself to the same rule: it prints occupancy and lets the hue carry the warning.
#[test]
fn rows_are_columns_of_figures_with_no_chart_drawn() {
    let styles = styles();
    let report = report();
    for slice in &report.slices {
        let text: String = slice_row(&report, slice, &styles)
            .spans
            .iter()
            .flat_map(|span| span.content.chars())
            .collect();
        assert!(
            !text.chars().any(|character| ('\u{2580}'..='\u{259f}').contains(&character)),
            "{text:?} draws block elements"
        );
    }
    let text = rendered(&report);
    assert!(text.contains("conversation    32.0k   67%"), "{text}");
    assert!(text.contains("tool schemas    12.0k   25%"), "aligned columns: {text}");
}

/// MCP is a line, not a category — and only when a server is actually enabled.
#[test]
fn mcp_is_reported_as_one_line_when_there_is_one_to_report() {
    assert!(!rendered(&report()).contains("MCP"));
    let mut with_mcp = report();
    with_mcp.mcp = Some(McpFootprint { servers: 3, gateway_tokens: 700 });
    let text = rendered(&with_mcp);
    assert!(text.contains("3 MCP servers behind one 700 gateway tool"), "{text}");
}

/// A local model with no published window reports what is in it and claims no percentage.
#[test]
fn an_unknown_window_claims_no_percentage() {
    let mut local = report();
    local.context_window = 0;
    let text = rendered(&local);
    assert!(text.contains("no published size"), "{text}");
    assert!(!text.contains("% used"), "{text}");
    assert!(
        !text.lines().any(|line| line.trim_start().starts_with("free")),
        "nothing to subtract from, so no remainder row: {text}"
    );
}

/// Findings hang under their marker so a wrapped one still reads as one item.
#[test]
fn a_long_finding_wraps_under_its_marker() {
    let styles = styles();
    let long = "A ".repeat(80);
    let wrapped = finding_lines(&long, styles.warning, &styles);
    assert!(wrapped.len() > 1);
    let text = |line: &Line<'static>| -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    };
    assert!(text(&wrapped[0]).starts_with("  · "));
    assert!(text(&wrapped[1]).starts_with("    A"), "{:?}", text(&wrapped[1]));
}

#[test]
fn an_empty_window_says_so_rather_than_drawing_nothing() {
    let mut empty = report();
    empty.slices.clear();
    assert!(rendered(&empty).contains("Nothing in the window yet"));
}
