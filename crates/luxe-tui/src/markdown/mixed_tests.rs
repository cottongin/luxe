//! A mixed-block document, pinned line for line in both voices.
//!
//! The parser's state machines (fences, lists, quotes, tables, inline runs) interleave in one walk,
//! so a restructure that moves a flush or a gap shows up here as a changed row before it shows up
//! anywhere a reader would notice.

use super::line::MarkdownBlockKind;
use super::voice::{build_code_style, build_heading_style, build_link_style};
use super::*;
use crate::theme::styles::ThemeStyles;
use pulldown_cmark::HeadingLevel;
use ratatui::style::Modifier;
use ratatui::text::Span;

const DOCUMENT: &str = "# Plan

Some **bold**, *italic*, `code` and a [link](https://example.com) here,
with a soft break.  \nAnd a hard one.

1. First
2. Second
   - nested *point*
   - [ ] task

- > a quote opening an item

> A quote with `code`.

> `code` first in a quote

> [!WARNING]
> Mind the gap.

```rust
fn main() {}
```

    indented code

<div>raw html</div>

| Name | Value |
|:-----|------:|
| `key` | 42 |
| [site](https://example.com) | x |

---

Trailing paragraph.

> - a list in a quote

after the list in a quote

> ```
> fenced in a quote
> ```

after the fence in a quote
";

fn styles() -> ThemeStyles {
    let resolved = luxe_core::theme::ThemeRegistry::new().resolve("luxe").expect("a bundled theme");
    ThemeStyles::from_resolved(&resolved, "luxe")
}

/// Each laid-out line as `kind width | text`, with a fence's rule and a table spelled out.
fn rows(lines: &[MarkdownLine]) -> Vec<String> {
    // A hard break is a newline inside a span; spelled out, so one row stays one line here
    let text = |spans: &[Span]| {
        spans.iter().map(|span| span.content.replace('\n', "⏎")).collect::<String>()
    };
    lines
        .iter()
        .map(|line| {
            let kind = match line.kind {
                MarkdownBlockKind::Prose => "P",
                MarkdownBlockKind::Wide => "W",
            };
            if let Some(rule) = &line.fence_rule {
                return format!(
                    "{kind} rule {} | {} … {}",
                    rule.ends_at,
                    text(&rule.lead),
                    text(&rule.tail)
                );
            }
            if let Some(table) = &line.table {
                let drawn = table.lines_at(usize::MAX);
                let body = drawn.iter().map(|row| text(&row.spans)).collect::<Vec<_>>().join(" / ");
                return format!("{kind} table | {body}");
            }
            format!("{kind} {} | {}", line.line.width(), text(&line.line.spans))
        })
        .collect()
}

fn digest(lines: &[MarkdownLine]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in format!("{lines:?}").bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// The mixed document's rows, widths and every span's style, in both voices, as recorded.
///
/// The break this catches: a block gap, a list prefix, a quote marker, a fence rule or a table
/// cell moving, or a span changing style, while the parser is restructured.
#[test]
fn a_mixed_document_lays_out_as_recorded_in_both_voices() {
    crate::glyphs::set_tier(crate::glyphs::GlyphTier::Nerd);
    let theme = styles();
    let answer = layout_markdown_in(DOCUMENT, &theme, MarkdownVoice::Answer);
    let reasoning = layout_markdown_in(DOCUMENT, &theme, MarkdownVoice::Reasoning);
    let recorded = include_str!("mixed_tests.txt");
    let actual = format!(
        "{}\n--- reasoning\n{}\nanswer {:016x} reasoning {:016x}\n",
        rows(&answer).join("\n"),
        rows(&reasoning).join("\n"),
        digest(&answer),
        digest(&reasoning),
    );
    if let Ok(path) = std::env::var("LUXE_MARKDOWN_PINS") {
        std::fs::write(path, &actual).expect("write the record");
    }
    assert_eq!(actual, recorded);
}

/// The two voices differ in style, not in shape: same rows, and the heading, inline code and link
/// each take their own voice's ink.
#[test]
fn the_voices_share_a_shape_and_differ_in_ink() {
    let theme = styles();
    let answer = layout_markdown_in(DOCUMENT, &theme, MarkdownVoice::Answer);
    let reasoning = layout_markdown_in(DOCUMENT, &theme, MarkdownVoice::Reasoning);
    assert_eq!(answer.len(), reasoning.len());
    let span = |lines: &[MarkdownLine], needle: &str| {
        lines
            .iter()
            .flat_map(|line| line.line.spans.iter())
            .find(|span| span.content.contains(needle))
            .map(|span| span.style)
            .unwrap_or_else(|| panic!("no span carrying {needle:?}"))
    };
    let heading = |voice| build_heading_style(&theme, HeadingLevel::H1, voice);
    assert_eq!(span(&answer, "Plan"), heading(MarkdownVoice::Answer));
    assert_eq!(span(&reasoning, "Plan"), heading(MarkdownVoice::Reasoning));
    assert_ne!(heading(MarkdownVoice::Answer), heading(MarkdownVoice::Reasoning));
    assert_eq!(span(&answer, "code"), build_code_style(&theme, MarkdownVoice::Answer));
    assert_eq!(span(&reasoning, "code"), build_code_style(&theme, MarkdownVoice::Reasoning));
    assert_eq!(span(&answer, "link"), build_link_style(&theme, MarkdownVoice::Answer));
    assert_eq!(span(&reasoning, "link"), build_link_style(&theme, MarkdownVoice::Reasoning));
    assert!(span(&reasoning, "Trailing").add_modifier.contains(Modifier::ITALIC));
    assert!(!span(&answer, "Trailing").add_modifier.contains(Modifier::ITALIC));
}
