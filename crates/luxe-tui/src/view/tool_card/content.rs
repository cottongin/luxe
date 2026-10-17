//! Card content — a file a `write` created, a command `bash` ran.
//!
//! Not a diff: every line of a `write` is new, so painting the whole body green says nothing a
//! reader can act on. Highlighted as the language it is, with a muted line-number gutter, it reads
//! as the file it just became. A command has no line numbers — it is not a file.

use super::{CardContext, RowLines};
use ratatui::text::Span;

/// Whether a body is numbered like a file or bare like a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Gutter {
    /// A muted 1-based ordinal per row — a file's line number, a search's place in its list.
    LineNumbers,
    /// No gutter.
    None,
}

/// Lays out at most `rows` lines of `text`, numbered from one.
///
/// Bounded because the card draws ten lines of a body and the window throws the rest away, so
/// highlighting all of it is work spent on rows nobody sees, and not a rounding error: a 600-line
/// `write` costs tens of milliseconds a frame, over the frame budget on its own, for as long as
/// the model is *still writing the file*.
///
/// `rows` is the window's own depth, or `None` for an opened card that draws everything.
///
/// The gutter is as wide as the last number that window can reach — not as wide as the payload.
/// Sized from the payload, every tenfold line the model wrote widened the column and shunted the
/// whole body one cell right, over and over, for as long as the write took. Bounded, it is the
/// same two cells from the first line to the six hundredth, and the body sits still while it fills.
pub(super) fn render(
    text: &str,
    language: Option<&str>,
    gutter: Gutter,
    rows: Option<usize>,
    ctx: &CardContext<'_>,
) -> Vec<RowLines> {
    let lines: Vec<String> =
        text.lines().take(rows.unwrap_or(usize::MAX)).map(super::rows::sanitize).collect();
    let number_width = match gutter {
        Gutter::LineNumbers => rows.unwrap_or(lines.len()).to_string().len(),
        Gutter::None => 0,
    };
    // The number plus the space after it.
    let gutter_width = if number_width == 0 { 0 } else { number_width + 1 };
    let code_width = super::code_width(usize::from(ctx.width), super::row_lead_width(gutter_width));

    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let gutter_span = match gutter {
                Gutter::LineNumbers => {
                    Span::styled(format!("{:>number_width$} ", index + 1), ctx.styles.muted)
                }
                Gutter::None => Span::raw(String::new()),
            };
            let code = super::highlighted_spans(line, language, ctx);
            super::wrapped_rows(gutter_span, code, code_width, gutter_width, ctx)
        })
        .collect()
}

/// What a `write` did, for the card's header: `Created · 1.2 KB`.
///
/// Read off the tool's own first line rather than recomputed, so the card and the transcript agree.
/// Whatever the tool said, if it isn't this shape the header simply stays quiet — a summary is a
/// courtesy, not something worth guessing at.
pub(super) fn write_outcome(output: &str) -> Option<String> {
    let first = output.lines().next()?;
    let verb = ["Created", "Overwrote"].into_iter().find(|verb| first.starts_with(verb))?;
    let bytes = first
        .rsplit_once(" (")
        .and_then(|(_, tail)| tail.strip_suffix(" bytes)"))
        .and_then(|count| count.parse::<u64>().ok())?;
    Some(format!("{verb} · {}", human_bytes(bytes)))
}

/// Which todo a `todo` call was about, for the card's header.
///
/// An `update` or a `complete` carries nothing but an id, which the header deliberately does not
/// show — and the checklist is the one view the user has of the plan, so a card that never names
/// the item it moved is the card failing at its only job. The name comes back with the result, past
/// the colon and whole.
///
/// Only the name. Where the item is *going* is already on the header, out of the arguments, before
/// the result exists; repeating it would spend the summary slot saying `in_progress` twice.
///
/// `add` is left alone: its own arguments already named what it added.
pub(super) fn todo_outcome(output: &str) -> Option<String> {
    let first = output.lines().next()?;
    if !first.starts_with("Updated todo #") && !first.starts_with("Completed todo #") {
        return None;
    }
    let (_, named) = first.split_once(": ")?;
    let named = named.trim();
    (!named.is_empty()).then(|| named.to_string())
}

/// A byte count at a scale a reader can act on.
fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    match bytes {
        0 => "empty".to_string(),
        bytes if bytes < KIB => format!("{bytes} B"),
        bytes if bytes < MIB => format!("{:.1} KB", bytes as f64 / KIB as f64),
        bytes => format!("{:.1} MB", bytes as f64 / MIB as f64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::tool_card::test_support::{context, plain, theme};

    /// A written file reads as the file it became: numbered, highlighted, not a wall of green.
    #[test]
    fn a_written_file_is_numbered_and_highlighted() {
        let styles = theme();
        let ctx = context(&styles, 80);
        let rendered =
            render("fn main() {\n    let x = 1;\n}", Some("rs"), Gutter::LineNumbers, None, &ctx);

        assert_eq!(rendered.len(), 3);
        assert_eq!(plain(&rendered[0][0]), "1 fn main() {");
        assert_eq!(plain(&rendered[2][0]), "3 }");
        assert_eq!(rendered[0][0].spans[0].style.fg, styles.muted.fg, "the gutter is muted");
        assert_ne!(
            rendered[0][0].spans[1].style.fg,
            Some(styles.success.fg.expect("a success color")),
            "content is not painted as an addition"
        );
        let colors: std::collections::HashSet<_> =
            rendered[1][0].spans.iter().skip(1).map(|span| span.style.fg).collect();
        assert!(colors.len() > 1, "the code is highlighted: {colors:?}");
    }

    /// The gutter widens for the largest number so the code stays in one column.
    #[test]
    fn the_gutter_fits_the_largest_line_number() {
        let styles = theme();
        let content = (1..=120).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
        let rendered = render(&content, None, Gutter::LineNumbers, None, &context(&styles, 80));

        assert_eq!(plain(&rendered[0][0]), "  1 line 1");
        assert_eq!(plain(&rendered[119][0]), "120 line 120");
    }

    /// A command is not a file, so it gets no line numbers.
    #[test]
    fn a_command_has_no_gutter() {
        let styles = theme();
        let rendered = render(
            "python3 - <<'PY'\nprint(1)\nPY",
            Some("sh"),
            Gutter::None,
            None,
            &context(&styles, 80),
        );
        assert_eq!(plain(&rendered[0][0]), "python3 - <<'PY'");
        assert_eq!(plain(&rendered[1][0]), "print(1)");
    }

    /// An empty line in a written file is still a line of that file.
    #[test]
    fn blank_lines_keep_their_numbers() {
        let styles = theme();
        let rendered =
            render("one\n\nthree", None, Gutter::LineNumbers, None, &context(&styles, 80));
        assert_eq!(rendered.len(), 3);
        assert_eq!(plain(&rendered[1][0]).trim(), "2");
    }

    #[test]
    fn a_write_says_whether_it_created_or_clobbered() {
        assert_eq!(
            write_outcome("Created '/tmp/a.rs' (2048 bytes)\n\n+1 fn main() {}").as_deref(),
            Some("Created · 2.0 KB")
        );
        assert_eq!(
            write_outcome("Overwrote '/tmp/a.rs' (12 bytes)").as_deref(),
            Some("Overwrote · 12 B")
        );
        assert_eq!(
            write_outcome("Created '/tmp/a.rs' (0 bytes)").as_deref(),
            Some("Created · empty")
        );
        // Anything else leaves the header alone rather than guessing.
        assert_eq!(write_outcome("Error: refused to write"), None);
        assert_eq!(write_outcome("Created something"), None);
        assert_eq!(write_outcome(""), None);
    }

    /// A `todo update` header has only an id in its arguments and does not show it, so the item's
    /// name has to come from the result — otherwise the one view the user has of the plan never
    /// says what moved.
    #[test]
    fn a_todo_update_names_the_item_it_moved() {
        assert_eq!(
            todo_outcome("Updated todo #3 → in_progress: wire the dock").as_deref(),
            Some("wire the dock"),
            "the header already says where it is going; the summary says what it is"
        );
        assert_eq!(
            todo_outcome("Updated todo #3: wire the dock").as_deref(),
            Some("wire the dock")
        );
        assert_eq!(
            todo_outcome("Completed todo #7: audit the copy").as_deref(),
            Some("audit the copy")
        );
        // An `add` already named it on its own header; saying it twice is noise.
        assert_eq!(todo_outcome("Added todo #1: wire the dock"), None);
        assert_eq!(todo_outcome("Added 3 todos:\n  #1 one"), None);
        // And anything else leaves the header alone rather than guessing.
        assert_eq!(todo_outcome("[ ] #1: one\n[x] #2: two"), None);
        assert_eq!(todo_outcome("Todo #9 not found"), None);
        assert_eq!(todo_outcome(""), None);
    }
}
