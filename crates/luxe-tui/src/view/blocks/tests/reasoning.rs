//! Reasoning — folded and expanded reasoning blocks and their peeks.

use super::*;

pub(super) fn reasoning(text: &str, expanded: bool) -> ConvoBlock {
    ConvoBlock::Thinking { text: text.into(), expanded }
}

/// Reasoning is markdown, and the whole point of rendering it is that its syntax stops showing:
/// a `**title**` becomes a bold title, a backticked symbol becomes a code-colored one, and the
/// muted italic voice survives both.
#[test]
fn expanded_reasoning_renders_its_markdown_in_the_reasoning_voice() {
    let styles = theme();
    let block = reasoning("**Verifying configuration**\n\nStart with `lms ls`.", true);
    let rendered = lines(&block, &context(&styles, 0));
    let text = rendered.iter().map(|line| line.line.to_string()).collect::<Vec<_>>();
    assert!(
        text.iter().all(|line| !line.contains('*') && !line.contains('`')),
        "markdown syntax must not survive rendering: {text:?}"
    );

    let title = rendered[0]
        .line
        .spans
        .iter()
        .find(|span| span.content.contains("Verifying configuration"))
        .expect("the title renders");
    assert!(title.style.add_modifier.contains(Modifier::BOLD), "a title is bold");
    assert!(title.style.add_modifier.contains(Modifier::ITALIC), "and still italic");
    assert_eq!(
        title.style.fg, styles.thinking_text.fg,
        "a title spends weight, not a color: the block stays reasoning"
    );

    let body = rendered.last().expect("a body line");
    let prose = body
        .line
        .spans
        .iter()
        .find(|span| span.content.contains("Start with"))
        .expect("the prose renders");
    assert_eq!(prose.style.fg, styles.thinking_text.fg);
    assert!(prose.style.add_modifier.contains(Modifier::ITALIC));
    assert!(!prose.style.add_modifier.contains(Modifier::BOLD));

    let code = body
        .line
        .spans
        .iter()
        .find(|span| span.content.contains("lms ls"))
        .expect("the code span renders");
    assert_eq!(code.style.fg, styles.md_code.fg, "a backticked symbol keeps the code hue");
    assert!(code.style.add_modifier.contains(Modifier::ITALIC));
}

/// A model's own line breaks are structure, not accident: joining them into one paragraph the
/// way authored markdown would turns a list of separate thoughts into a wall of text.
#[test]
fn reasoning_keeps_the_line_breaks_the_model_wrote() {
    let styles = theme();
    let block = reasoning("first thought\nsecond thought", true);
    // `Line`'s Display drops newlines, so the break has to be read off the spans themselves.
    let text = lines(&block, &context(&styles, 0))
        .iter()
        .flat_map(|layout| layout.line.spans.iter())
        .map(|span| span.content.to_string())
        .collect::<String>();
    assert_eq!(text, "first thought\nsecond thought");

    // An answer, by contrast, reflows: a soft break there is authored prose, not structure.
    let answer = ConvoBlock::Assistant("first thought\nsecond thought".into());
    let reflowed = lines(&answer, &context(&styles, 0))
        .iter()
        .flat_map(|layout| layout.line.spans.iter())
        .map(|span| span.content.to_string())
        .collect::<String>();
    assert_eq!(reflowed, "first thought second thought");
}

/// The remediation report is for the review: the transcript shows the prose around it, and a
/// message that was nothing but the report shows nothing.
#[test]
fn the_remediation_report_stays_out_of_the_transcript() {
    let styles = theme();
    let report = "```luxe-remediation\n{\"findings\":[]}\n```";
    let text = |block: &ConvoBlock| {
        lines(block, &context(&styles, 0))
            .iter()
            .map(|layout| layout.line.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let answer = ConvoBlock::Assistant(format!("Fixed both.\n{report}"));
    assert_eq!(text(&answer), "Fixed both.");
    assert!(lines(&ConvoBlock::Assistant(report.into()), &context(&styles, 0)).is_empty());
}

/// The collapsed peek is built from the *rendered* line, so it shows the section title rather
/// than the asterisks that made it — and counts what it is standing in for.
#[test]
fn a_collapsed_reasoning_block_peeks_at_its_rendered_title() {
    let styles = theme();
    let block = reasoning("**Verifying configuration**\n\nStart with `lms ls`.", false);
    let rendered = lines(&block, &context(&styles, 0));
    assert_eq!(rendered.len(), 1, "a collapsed block is one line");
    assert_eq!(rendered[0].line.to_string(), "Verifying configuration · +1 more line");

    // A single short thought hides nothing, so it says nothing; empty reasoning still speaks.
    let short = lines(&reasoning("just this", false), &context(&styles, 0));
    assert_eq!(short[0].line.to_string(), "just this");
    let empty = lines(&reasoning("   ", false), &context(&styles, 0));
    assert_eq!(empty[0].line.to_string(), "(no content)");

    // A hard break hides a line as surely as a paragraph does, and is counted the same way.
    let broken = lines(&reasoning("one\ntwo\nthree", false), &context(&styles, 0));
    assert_eq!(broken[0].line.to_string(), "one · +2 more lines");

    // An over-long first line is cut to the prose measure, ellipsis included.
    let long = "w".repeat(200);
    let capped = lines(&reasoning(&long, false), &context(&styles, 0));
    assert_eq!(capped[0].line.to_string(), format!("{}…", "w".repeat(79)));

    // A line that fills the measure exactly is not cut: an ellipsis there would be spending a
    // character to report an elision that never happened.
    let exact = lines(&reasoning(&"w".repeat(80), false), &context(&styles, 0));
    assert_eq!(exact[0].line.to_string(), "w".repeat(80));
    let over = lines(&reasoning(&"w".repeat(81), false), &context(&styles, 0));
    assert_eq!(over[0].line.to_string(), format!("{}…", "w".repeat(79)));
}

/// The peek's whole job is to be one line. A count that pushed it past the measure would wrap
/// into a second row, which is the fold failing at the only thing it promised.
#[test]
fn a_peek_and_its_count_fit_the_prose_measure() {
    let styles = theme();
    let sentence = "a thought long enough to run past any measure worth wrapping at, twice \
                        over, with no break in it anywhere";
    for width in [16u16, 24, 30, 40, 72, 80, 120] {
        let mut ctx = context(&styles, 0);
        ctx.prose_width = width;
        let peek = lines(&reasoning(&format!("{sentence}\nand another"), false), &ctx);
        assert_eq!(peek.len(), 1, "a peek is one line at {width}");
        let text = line_text(&peek[0].line);
        assert!(
            crate::selection::cell_width(&text) <= usize::from(width),
            "{width}: peek is {} cells: {text:?}",
            crate::selection::cell_width(&text)
        );
        // Wide enough to seat both, the count is there; too narrow, and the thought keeps the
        // cells rather than sharing them with a number.
        assert_eq!(
            text.ends_with("· +1 more line"),
            width >= 40,
            "{width}: the wrong half won the canvas: {text:?}"
        );
    }
}

/// Reasoning shown in full is prose. A click on it must not fold it away, so no row of it is a
/// handle — the collapsed peek is the only thing here a click acts on.
#[test]
fn only_a_folded_reasoning_block_is_a_click_handle() {
    let styles = theme();
    let text = "**Tracing it**\n\nThe buffering happens in `feed`.\n\nSo I will read it.";
    let shown = lines(&reasoning(text, true), &context(&styles, 0));
    assert!(shown.len() > 1, "the fixture has rows to click");
    assert!(
        shown.iter().all(|line| !line.expands),
        "nothing in fully shown reasoning opens or closes on a click"
    );

    let folded = lines(&reasoning(text, false), &context(&styles, 0));
    assert!(folded[0].expands, "a peek is clickable, or there is no way back in with a mouse");
}

/// A fence inside reasoning stays in the reasoning voice: a syntect palette in a muted italic
/// block reads as an answer, which is exactly what it is not.
#[test]
fn a_reasoning_fence_is_not_syntax_highlighted() {
    let styles = theme();
    let block = reasoning("try:\n\n```rust\nlet total = 1 + 2;\n```", true);
    let rendered = lines(&block, &context(&styles, 0));
    let code = rendered
        .iter()
        .find(|line| line.line.to_string().contains("let total"))
        .expect("the fence renders");
    assert_eq!(code.line.spans.len(), 1, "one span, so nothing was tokenized");
    assert_eq!(code.line.spans[0].style.fg, styles.md_code.fg);
    assert!(code.line.spans[0].style.add_modifier.contains(Modifier::ITALIC));
}
