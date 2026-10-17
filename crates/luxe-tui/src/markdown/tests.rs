use super::fence::fence_language;
use super::*;
use crate::theme::styles::ThemeStyles;
use luxe_core::theme::{Theme as CoreTheme, ThemeRegistry};
use ratatui::style::Modifier;
use ratatui::text::Line;
use std::collections::HashMap;

/// A fence's rule announces the language, normalized, and says nothing where there is nothing
/// to say. `text` is not a language, it is a way of writing "this is a code block".
#[test]
fn a_fence_names_its_language_or_names_nothing() {
    assert_eq!(fence_language(Some("rust")).as_deref(), Some("rust"));
    assert_eq!(fence_language(Some("  Rust  ")).as_deref(), Some("rust"), "hand-written");
    assert_eq!(fence_language(Some("rust,ignore")).as_deref(), Some("rust"), "rustdoc");
    assert_eq!(fence_language(Some("rust no_run")).as_deref(), Some("rust"), "mdBook");
    assert_eq!(fence_language(Some("rust,ignore")).as_deref(), Some("rust"));
    for empty in ["", "   ", "text", "TEXT", "plain", "plaintext", "txt"] {
        assert_eq!(fence_language(Some(empty)), None, "{empty:?} names no language");
    }
    assert_eq!(fence_language(None), None);
}

/// Helper: convert a CoreTheme to ThemeStyles for testing.
fn theme_to_styles(theme: &CoreTheme) -> ThemeStyles {
    let _registry = ThemeRegistry::new();
    let resolved: HashMap<String, luxe_core::theme::ColorValue> = theme.resolve_aliases();
    ThemeStyles::from_resolved(&resolved, &theme.name)
}

#[test]
fn test_bold_and_code() {
    let theme = luxe_core::theme::builtin_themes()[0].clone();
    let theme = theme_to_styles(&theme);
    let input = "**bold** and `code`";
    let lines = render_markdown(input, &theme);
    assert!(!lines.is_empty(), "should produce at least one line");
    // Should have at least one span with BOLD modifier
    let has_bold = lines.iter().any(|line| {
        line.spans.iter().any(|span| span.style.add_modifier(Modifier::BOLD) == span.style)
    });
    assert!(has_bold, "expected BOLD modifier on some span");
    // Inline code should be rendered (text "code" should appear)
    let has_code = lines.iter().any(|line| line.spans.iter().any(|span| span.content == "code"));
    assert!(has_code, "expected inline code 'code' to be rendered");
}

#[test]
fn test_heading() {
    let theme = luxe_core::theme::builtin_themes()[0].clone();
    let theme = theme_to_styles(&theme);
    let input = "# Heading\nSome text";
    let lines = render_markdown(input, &theme);
    assert!(lines.len() >= 2, "heading + text should produce at least 2 lines");
    // Heading text should have BOLD modifier (from md_heading token)
    let heading_line = &lines[0];
    let has_bold =
        heading_line.spans.iter().any(|span| span.style.add_modifier(Modifier::BOLD) == span.style);
    assert!(has_bold, "heading text should have BOLD modifier from md_heading token");
}

#[test]
fn test_heading_no_leak() {
    let theme = luxe_core::theme::builtin_themes()[0].clone();
    let theme = theme_to_styles(&theme);
    let input = "# Heading\n\nSome text";
    let lines = render_markdown(input, &theme);
    assert!(lines.len() >= 2, "heading + blank line + text should produce at least 2 lines");
    // Body text must NOT carry the heading BOLD style
    let body = lines.last().unwrap();
    let body_bold = body.spans.iter().any(|s| s.style.add_modifier(Modifier::BOLD) == s.style);
    assert!(!body_bold, "heading style leaked into body text");
}

#[test]
fn test_unordered_list() {
    let theme = luxe_core::theme::builtin_themes()[0].clone();
    let theme = theme_to_styles(&theme);
    let input = "- item 1\n- item 2\n- item 3";
    let lines = render_markdown(input, &theme);
    assert_eq!(lines.len(), 3, "should produce 3 list items");
    // Each line should start with a bullet prefix
    for (i, line) in lines.iter().enumerate() {
        let content = line.spans.iter().map(|s| s.content.as_ref()).collect::<String>();
        assert!(content.starts_with("• "), "line {} should start with bullet: {:?}", i, content);
    }
}

#[test]
fn test_nested_list() {
    let theme = luxe_core::theme::builtin_themes()[0].clone();
    let theme = theme_to_styles(&theme);
    let input = "- item a\n  - nested b\n  - nested c";
    let lines = render_markdown(input, &theme);
    assert_eq!(lines.len(), 3, "should produce 3 list items");
    // Top-level item: no indent
    let content0 = lines[0].spans.iter().map(|s| s.content.as_ref()).collect::<String>();
    assert_eq!(content0, "• item a", "top-level should have no indent");
    // Nested items: 2-space indent + bullet
    for i in [1, 2] {
        let content = lines[i].spans.iter().map(|s| s.content.as_ref()).collect::<String>();
        assert!(
            content.starts_with("  • "),
            "line {} should have 2-space indent + bullet: {:?}",
            i,
            content
        );
    }
}

#[test]
fn test_ordered_list() {
    let theme = luxe_core::theme::builtin_themes()[0].clone();
    let theme = theme_to_styles(&theme);
    // Ordered list: 1., 2., 3.
    let input = "1. first\n2. second\n3. third";
    let lines = render_markdown(input, &theme);
    assert_eq!(lines.len(), 3, "should produce 3 list items");
    // Each line should start with a numbered prefix
    for (i, line) in lines.iter().enumerate() {
        let content = line.spans.iter().map(|s| s.content.as_ref()).collect::<String>();
        assert!(
            content.starts_with(&format!("{}. ", i + 1)),
            "line {} should start with numbered prefix: {:?}",
            i,
            content
        );
    }
}

#[test]
fn test_blockquote() {
    let theme = luxe_core::theme::builtin_themes()[0].clone();
    let theme = theme_to_styles(&theme);
    // Blockquote (pulldown-cmark treats "\n> " as SoftBreak within one Paragraph)
    let input = "> This is a quote\n> with multiple lines";
    let lines = render_markdown(input, &theme);
    assert_eq!(lines.len(), 1, "should produce 1 quoted line (SoftBreak -> space)");
    // Line should start with "> " prefix with md_quote styling (non-default fg)
    let spans = &lines[0].spans;
    assert!(spans.len() >= 2, "line should have prefix span + text span: {:?}", spans);
    assert_eq!(spans[0].content.as_ref(), "> ", "first span is the prefix");
    let quote_fg = spans[0].style.fg;
    assert!(quote_fg.is_some(), "prefix span should carry md_quote fg (not default)");
    // Body text after prefix
    let body: String = spans.iter().skip(1).map(|s| s.content.as_ref()).collect();
    assert_eq!(body, "This is a quote with multiple lines");
}

/// A quote that opens with a list, and a list item that opens with a quote, each keep the quote's
/// `> ` marker.
///
/// The break this catches: the marker is written before a line's first text, and an item's bullet
/// had already made the line non-empty by then, so the quote read as plain list text.
#[test]
fn a_quote_and_a_list_keep_the_quote_marker_when_one_opens_the_other() {
    let theme = theme_to_styles(&luxe_core::theme::builtin_themes()[0].clone());
    let text = |input: &str| -> Vec<String> {
        render_markdown(input, &theme)
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect::<String>())
            .filter(|row| !row.is_empty())
            .collect()
    };
    assert_eq!(text("> - one\n> - two"), ["> • one", "> • two"], "a list in a quote");
    assert_eq!(text("- > quoted"), ["• > quoted"], "a quote in an item");
    assert_eq!(text("> 1. first"), ["> 1. first"], "an ordered list in a quote");
}

/// An alert carries its severity in a tag rather than in its prose, so the tag has to become
/// something the reader sees. It becomes the word: the mark beside it is polish, and a reader
/// whose font draws none of them still reads "Caution".
#[test]
fn an_alert_writes_its_kind_out_above_the_quote() {
    let theme = theme_to_styles(&luxe_core::theme::builtin_themes()[0].clone());
    for (tag, word) in [
        ("NOTE", "Note"),
        ("TIP", "Tip"),
        ("IMPORTANT", "Important"),
        ("WARNING", "Warning"),
        ("CAUTION", "Caution"),
    ] {
        let rendered: Vec<String> = render_markdown(&format!("> [!{tag}]\n> mind this"), &theme)
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();

        let labeled = rendered
            .iter()
            .position(|line| line.ends_with(word))
            .unwrap_or_else(|| panic!("no line says {word:?}: {rendered:?}"));
        assert_eq!(
            rendered.get(labeled + 1).map(String::as_str),
            Some("> mind this"),
            "the quote follows its own word without a gap: {rendered:?}"
        );
        assert!(
            !rendered.iter().any(|line| line.contains("[!")),
            "the raw tag never reaches the reader: {rendered:?}"
        );
    }
}

/// The word takes the severity's color for an answer and weight alone for reasoning, which
/// spends structure rather than color everywhere else in that voice too.
#[test]
fn an_alerts_word_is_colored_by_severity_except_in_the_reasoning_voice() {
    let theme = theme_to_styles(&luxe_core::theme::builtin_themes()[0].clone());
    let word_style = |voice| {
        layout_markdown_in("> [!CAUTION]\n> careful", &theme, voice)
            .iter()
            .flat_map(|layout| layout.line.spans.clone())
            .find(|span| span.content.ends_with("Caution"))
            .expect("the alert's word")
            .style
    };

    let answer = word_style(MarkdownVoice::Answer);
    assert_eq!(answer.fg, theme.error.fg, "caution is the theme's error color");
    assert!(answer.add_modifier.contains(Modifier::BOLD));

    let reasoning = word_style(MarkdownVoice::Reasoning);
    assert_eq!(reasoning.fg, theme.thinking_text.fg, "reasoning keeps its muted voice");
    assert!(reasoning.add_modifier.contains(Modifier::BOLD | Modifier::ITALIC));
}

#[test]
fn test_blockquote_inline_formatting() {
    let theme = luxe_core::theme::builtin_themes()[0].clone();
    let theme = theme_to_styles(&theme);
    // Inline bold inside blockquote: one prefix, BOLD modifier preserved
    let input = "> **bold** and normal";
    let lines = render_markdown(input, &theme);
    assert_eq!(lines.len(), 1, "should produce 1 quoted line");
    let spans = &lines[0].spans;
    // First span is the prefix (exactly one)
    assert_eq!(spans[0].content.as_ref(), "> ", "single prefix span");
    assert!(
        spans[0].style.fg.is_some(),
        "prefix should have a foreground color (DarkGray fallback or md_quote token)"
    );
    // Find the "bold" text and verify BOLD modifier
    let bold_span =
        spans.iter().find(|s| s.content.as_ref() == "bold").expect("'bold' span missing");
    assert!(
        bold_span.style.add_modifier.contains(Modifier::BOLD),
        "'bold' should have BOLD modifier, got {:?}",
        bold_span.style
    );
}

#[test]
fn test_loose_list() {
    let theme = luxe_core::theme::builtin_themes()[0].clone();
    let theme = theme_to_styles(&theme);
    // Loose list (blank line between items) wraps each item in a paragraph
    let input = "- item 1\n\n- item 2\n\n- item 3";
    let lines = render_markdown(input, &theme);
    // Each bullet + text should be on the same line (no orphaned bullets)
    let bullet_lines: Vec<_> = lines
        .iter()
        .filter(|line| {
            let content: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            content.starts_with("• ")
        })
        .collect();
    assert_eq!(bullet_lines.len(), 3, "should have 3 bullet lines");
    // Each bullet line should also contain its text
    for (i, line) in bullet_lines.iter().enumerate() {
        let content: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            content.contains(format!("item {}", i + 1).as_str()),
            "bullet line {} should contain text: {:?}",
            i,
            content
        );
    }
}

#[test]
fn test_code_block() {
    let theme = luxe_core::theme::builtin_themes()[0].clone();
    let theme = theme_to_styles(&theme);
    let input = "```rust\nlet x = 1;\n```\nText after";
    let lines = render_markdown(input, &theme);
    assert!(lines.len() >= 2, "code block + text should produce at least 2 lines");
    // Syntect should produce multiple colored spans for Rust code
    let has_highlighted_line = lines.iter().any(|line| {
        line.spans.len() > 1 && line.spans.windows(2).any(|w| w[0].style != w[1].style)
    });
    assert!(
        has_highlighted_line,
        "expected syntect to produce multiple colored spans for Rust code"
    );
}

#[test]
fn test_empty_input() {
    let theme = luxe_core::theme::builtin_themes()[0].clone();
    let theme = theme_to_styles(&theme);
    let input = "";
    let lines = render_markdown(input, &theme);
    assert!(lines.is_empty(), "empty input should produce no lines");
}

#[test]
fn blocks_are_separated_by_blank_lines_but_list_items_stay_tight() {
    let theme = luxe_core::theme::builtin_themes()[0].clone();
    let theme = theme_to_styles(&theme);
    let input = "Intro paragraph.\n\n- Item 1\n- Item 2\n\nAfter the list.";
    let lines = render_markdown(input, &theme);
    let text = |i: usize| -> String {
        lines[i].spans.iter().map(|s| s.content.as_ref()).collect::<String>()
    };
    // Intro / blank / item1 / item2 / blank / after — blanks between blocks, items tight.
    assert_eq!(lines.len(), 6, "got: {:?}", (0..lines.len()).map(text).collect::<Vec<_>>());
    assert_eq!(text(0), "Intro paragraph.");
    assert!(lines[1].spans.is_empty(), "blank before the list");
    assert!(text(2).contains("Item 1"));
    assert!(text(3).contains("Item 2"));
    assert!(lines[4].spans.is_empty(), "blank after the list");
    assert_eq!(text(5), "After the list.");
}

#[test]
fn test_horizontal_rule() {
    let theme = luxe_core::theme::builtin_themes()[0].clone();
    let theme = theme_to_styles(&theme);
    let input = "before\n\n---\n\nafter";
    let lines = render_markdown(input, &theme);
    // Blocks are separated by blank lines: before / _ / HR / _ / after.
    assert_eq!(lines.len(), 5, "expected before, blank, HR, blank, after");
    assert_eq!(lines[0].spans[0].content.as_ref(), "before");
    assert!(lines[1].spans.is_empty(), "blank gap before the rule");
    // L2: horizontal rule
    assert!(lines[2].spans[0].content.contains('━'), "HR line should contain box-drawing chars");
    assert!(
        lines[2].spans[0].style.add_modifier.contains(Modifier::BOLD),
        "HR span should have BOLD modifier"
    );
    // HR should use muted token foreground (not hardcoded DarkGray).
    assert_eq!(
        lines[2].spans[0].style.fg, theme.muted.fg,
        "HR span should use muted token foreground"
    );
    assert!(lines[3].spans.is_empty(), "blank gap after the rule");
    assert_eq!(lines[4].spans[0].content.as_ref(), "after");
}

#[test]
fn test_link() {
    let theme = luxe_core::theme::builtin_themes()[0].clone();
    let theme = theme_to_styles(&theme);
    let input = "Check [this link](https://example.com) out";
    let lines = render_markdown(input, &theme);
    assert_eq!(lines.len(), 1, "single paragraph");
    let content: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(content.contains("this link"), "should contain link text: {}", content);
    assert_eq!(content, "Check this link (https://example.com) out", "the address follows");
    let autolink: String = render_markdown("See <https://example.com>.", &theme)[0]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(autolink, "See https://example.com.", "an address is not said twice");
    let styled: String = render_markdown("[**bold** words](https://x.dev)", &theme)[0]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(styled, "bold words (https://x.dev)", "once, after every span");
    // The text span should have UNDERLINED modifier (from link style)
    let text_span = lines[0]
        .spans
        .iter()
        .find(|s| s.content.contains("this link"))
        .expect("text span should exist");
    assert!(
        text_span.style.add_modifier.contains(Modifier::UNDERLINED),
        "text span should have UNDERLINED modifier"
    );
}

#[test]
fn test_table() {
    let theme = luxe_core::theme::builtin_themes()[0].clone();
    let theme = theme_to_styles(&theme);
    let input = "| Header 1 | Header 2 |\n|--------|--------|\n| Cell 1 | Cell 2 |";
    let lines = render_markdown(input, &theme);
    let text = |line: &Line| -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect::<String>()
    };
    // top border, header, header separator, one body row, bottom border
    assert_eq!(lines.len(), 5, "expected a bordered 5-line table");
    assert!(text(&lines[0]).starts_with('┌') && text(&lines[0]).contains('┬'));

    let header = text(&lines[1]);
    assert!(header.contains("Header 1") && header.contains("Header 2"), "header row: {header}");
    assert!(header.starts_with('│') && header.ends_with('│'));
    assert!(
        lines[1].spans.iter().any(|s| s.style.add_modifier.contains(Modifier::BOLD)),
        "header row should be bold"
    );

    assert!(text(&lines[2]).contains('┼'), "expected a header separator row");

    let body = text(&lines[3]);
    assert!(body.contains("Cell 1") && body.contains("Cell 2"), "body row: {body}");
    assert!(text(&lines[4]).starts_with('└') && text(&lines[4]).ends_with('┘'));

    // Cell text stays readable; only the box uses the muted border color.
    let cell = lines[3].spans.iter().find(|s| s.content.contains("Cell 1")).unwrap();
    assert_eq!(cell.style.fg, theme.text.fg, "cell text must use readable foreground");
}

#[test]
fn test_table_columns_align_across_rows() {
    let theme = luxe_core::theme::builtin_themes()[0].clone();
    let theme = theme_to_styles(&theme);
    // Uneven natural widths must be padded to one shared grid width.
    let input = "| A | Name |\n|---|---|\n| longvalue | x |";
    let lines = render_markdown(input, &theme);
    let width = |line: &Line| -> usize {
        line.spans.iter().map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref())).sum()
    };
    let first = width(&lines[0]);
    assert!(first > 0);
    assert!(
        lines.iter().all(|line| width(line) == first),
        "every border and row must share one width for an aligned grid"
    );
}

/// pulldown-cmark numbers its alignments None, Left, Center, Right; reading them off by one
/// drew every centered column right-aligned and every right-aligned one centered.
#[test]
fn a_column_aligns_the_way_its_separator_says() {
    let theme = theme_to_styles(&luxe_core::theme::builtin_themes()[0].clone());
    let input = "| left | middle | right |\n|:-----|:------:|------:|\n| a | b | c |";
    let lines = render_markdown(input, &theme);
    let body: String = lines[3].spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(body, "│ a    │   b    │     c │");
}

#[test]
fn test_streaming_safety() {
    // Malformed/partial markdown must never panic render_markdown —
    // these are exactly the inputs a streaming model produces mid-token.
    let theme = luxe_core::theme::builtin_themes()[0].clone();
    let theme = theme_to_styles(&theme);
    let partial_inputs = [
        "# H1",
        "## H2\n",                     // unclosed heading (no text yet)
        "**bold",                      // unclosed bold
        "*italic",                     // unclosed italic
        "`code",                       // unclosed inline code
        "[link](url",                  // unclosed link
        "| a | b |\n|---|---|\n| c |", // partial table (no closing row)
        "```
            code",         // unclosed fence (no closing ```)
        "> quote",                     // unclosed blockquote
        "- item",                      // unclosed list item
        "---",                         // bare HR with no surrounding paragraphs
        "",
    ];
    for input in partial_inputs {
        let lines = render_markdown(input, &theme);
        // Just verifying it doesn't panic; output shape is allowed to vary.
        let _ = lines;
    }
}

/// A backticked identifier in a table cell stays in that cell.
///
/// The break this catches: inline code always went to the prose line, so it vanished from the
/// grid and was flushed after the table, glued to whatever paragraph came next.
#[test]
fn table_inline_code_stays_in_its_cell() {
    let theme = theme_to_styles(&luxe_core::theme::builtin_themes()[0].clone());
    let input = "| `key` | Meaning |\n|---|---|\n| `retries` | how many times, `n` max |\n\nAfter.";
    let lines = render_markdown(input, &theme);
    let text = |line: &Line| line.spans.iter().map(|s| s.content.as_ref()).collect::<String>();
    let all: Vec<String> = lines.iter().map(text).collect();
    assert!(all[1].contains("key") && all[1].contains("Meaning"), "code-only header: {all:?}");
    assert!(all[3].contains("retries"), "code-only body cell: {all:?}");
    assert!(all[3].contains("how many times, n max"), "mixed cell: {all:?}");
    let after = all.iter().find(|line| line.contains("After")).expect("the paragraph");
    assert!(!after.contains("retries") && !after.contains("key"), "nothing leaked: {after}");
}

/// A quote ends where its block ends: the paragraph after it is not quoted, however the quote's
/// last block finished.
///
/// The break this catches: a quote that ended with a line still pending flushed it and never
/// closed, so everything after a quoted list or fence was drawn with `> ` in front of it.
#[test]
fn text_after_a_quote_is_not_quoted() {
    let theme = theme_to_styles(&luxe_core::theme::builtin_themes()[0].clone());
    for source in ["> - a list in a quote\n\nafter\n", "> ```\n> fenced\n> ```\n\nafter\n"] {
        let lines = render_markdown(source, &theme);
        let last: String = lines
            .iter()
            .rev()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect::<String>())
            .find(|text| !text.trim().is_empty())
            .expect("something drawn");
        assert_eq!(last, "after", "{source:?}");
    }
}
