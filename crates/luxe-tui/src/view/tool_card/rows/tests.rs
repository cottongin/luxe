use super::*;

/// Builds a preview exactly the way `luxe_tools::diff_preview` does: one shared, right-aligned
/// number field across every row. Hand-counting the spaces is how the parser got this wrong in
/// the first place.
fn preview(rows: &[(char, Option<usize>, &str)]) -> String {
    let width = rows
        .iter()
        .filter_map(|(_, number, _)| *number)
        .map(|number| number.to_string().len())
        .max()
        .unwrap_or(1);
    rows.iter()
        .map(|(sign, number, code)| match number {
            Some(number) => format!("{sign}{number:>width$} {code}"),
            None if *sign == ' ' => format!("{sign}{:>width$} {code}", ""),
            // A note occupies the whole line, gutter and all.
            None => (*code).to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn source(row: &Row) -> &SourceRow {
    match row {
        Row::Source(source) => source,
        other => panic!("expected a source row, got {other:?}"),
    }
}

/// The three shapes a preview line can take, told apart by their first characters.
#[test]
fn a_preview_parses_into_source_rows_elisions_and_notes() {
    let rows = parse(&preview(&[
        ('!', None, "Applied 1 edit(s) to 'src/lib.rs'"),
        (' ', None, "..."),
        (' ', Some(11), "fn main() {"),
        ('-', Some(12), "    println!(\"old\");"),
        ('+', Some(12), "    println!(\"new\");"),
        (' ', Some(13), "}"),
    ]));

    assert_eq!(rows[0], Row::Note("Applied 1 edit(s) to 'src/lib.rs'".into()));
    assert_eq!(rows[1], Row::Elision);
    assert_eq!(
        source(&rows[2]),
        &SourceRow { kind: RowKind::Context, number: Some(11), code: "fn main() {".into() }
    );
    assert_eq!(source(&rows[3]).kind, RowKind::Removed);
    assert_eq!(source(&rows[4]).kind, RowKind::Added);
    assert_eq!(source(&rows[5]).number, Some(13));
    assert_eq!(number_width(&rows), 2);
}

/// Indentation is half of what a reader looks at in a diff, and it belongs to the code.
///
/// The gutter is one shared width for the whole preview, so a row cannot be split by consuming
/// spaces and digits until the code starts — that reads a Rust body's four-space indent as more
/// gutter and quietly deletes it.
#[test]
fn indentation_survives_the_gutter() {
    for (numbers, indent) in [(vec![4usize, 4], 4), (vec![120, 120], 8)] {
        let code = format!("{}nested();", " ".repeat(indent));
        let rows = parse(&preview(&[
            ('-', Some(numbers[0]), &code),
            ('+', Some(numbers[1]), "  shallow();"),
        ]));
        assert_eq!(source(&rows[0]).code, code, "indent lost with numbers {numbers:?}");
        assert_eq!(source(&rows[1]).code, "  shallow();");
    }
}

/// A tab is spent before anything measures the row, and from the code's own column zero.
///
/// `unicode-width` scores a tab nothing and the terminal advances it to a stop, so a card that
/// let one through laid its columns out against a width the screen did not agree with. Anchoring
/// the stops to the code rather than to the whole row is what keeps one file's indentation the
/// same width either side of line 100, where the gutter grows a digit.
#[test]
fn a_tab_is_spent_before_a_row_is_measured() {
    let rows = parse(&preview(&[
        (' ', Some(9), "\tone();"),
        ('-', Some(10), "\t\ttwo();"),
        ('+', Some(10), "ab\tc();"),
    ]));

    assert_eq!(source(&rows[0]).code, "    one();");
    assert_eq!(source(&rows[1]).code, "        two();");
    assert_eq!(source(&rows[2]).code, "ab  c();", "two cells in, the stop is two cells away");

    // A five-digit gutter is five cells wider and must not move the indentation with it.
    let deep = parse(&preview(&[(' ', Some(10_000), "\tone();")]));
    assert_eq!(source(&deep[0]).code, "    one();");
}

/// A note is measured and wrapped like anything else, so it cannot carry a tab either.
#[test]
fn a_note_spends_its_tabs_too() {
    let rows = parse("Applied 1 edit(s)\tto 'src/lib.rs'");
    // Seventeen cells in, the next stop is three away — not a fixed run of four.
    assert_eq!(rows[0], Row::Note("Applied 1 edit(s)   to 'src/lib.rs'".into()));
}

/// `...` is the preview's own elision only on the row the preview put it on.
///
/// A *numbered* row whose code happens to be `...` is a line of the file — Python writes one,
/// and so does a stubbed body — and folding it into the elision would delete a line of the diff
/// while claiming the file was skipped there. The mirror is as wrong: an unnumbered row is not
/// a marker unless `...` is the whole of what it says.
#[test]
fn only_an_unnumbered_row_of_dots_is_the_elision() {
    let rows = parse(&preview(&[
        (' ', None, "..."),
        ('-', Some(7), "..."),
        ('+', Some(7), "Ellipsis"),
        (' ', None, "unnumbered, but not a marker"),
    ]));

    assert_eq!(rows[0], Row::Elision, "the preview's own marker");
    assert_eq!(
        source(&rows[1]),
        &SourceRow { kind: RowKind::Removed, number: Some(7), code: "...".into() },
        "a numbered `...` is a line of the file, not a stretch of it that was skipped"
    );
    assert_eq!(
        source(&rows[3]),
        &SourceRow {
            kind: RowKind::Context,
            number: None,
            code: "unnumbered, but not a marker".into()
        }
    );
}

/// A row with no line number is not allowed to say where the gutter ends.
///
/// Every numbered row reports the same separator, because the writer right-aligns one field
/// across the preview. A row with an empty field reports whatever its *code* looks like, and
/// code beginning `42 ` reads exactly like a number and a separator — always deeper than the
/// truth. Cutting every row at the deepest answer credited that row with line 42 and then took
/// `42 ` off the front of the next row's code while dropping its number entirely.
#[test]
fn an_unnumbered_row_does_not_widen_the_gutter() {
    let rows = parse(&preview(&[
        (' ', Some(1000), "code"),
        (' ', None, "42 answer"),
        (' ', Some(1), "42 answer"),
    ]));

    assert_eq!(source(&rows[0]).number, Some(1000));
    assert_eq!(
        source(&rows[1]),
        &SourceRow { kind: RowKind::Context, number: None, code: "42 answer".into() },
        "the row's own code was read as its line number"
    );
    assert_eq!(
        source(&rows[2]),
        &SourceRow { kind: RowKind::Context, number: Some(1), code: "42 answer".into() },
        "and the row after it lost its number and the head of its code"
    );
}

/// A row the shared gutter does not fit falls back to its own separator.
///
/// The shared width is a byte offset into every row at once, so a row that does not match it is
/// the one case where the whole-preview reading has to be abandoned per row. Both ways it can
/// fail to match are here: a row too short to reach the offset at all — which `split_at` would
/// panic on rather than merely misread — and a row whose *code* reads like more gutter, which is
/// what the trailing space in [`is_gutter`] exists to rule out. A file of numbers is not exotic.
#[test]
fn a_row_the_shared_gutter_does_not_fit_keeps_its_own() {
    // The second row's number field is five cells wider, so the shared width overruns the first
    // row's last byte.
    let short = parse(" 5 x\n 1000 yyyy");
    assert_eq!(
        source(&short[0]),
        &SourceRow { kind: RowKind::Context, number: Some(5), code: "x".into() },
        "the short row kept its own gutter"
    );
    assert_eq!(source(&short[1]).number, Some(1000));
    assert_eq!(source(&short[1]).code, "yyyy");

    // Long enough to reach the shared offset, and every byte up to it is a digit or a space —
    // so only the missing separator says this is code rather than gutter.
    let numeric = parse(" 12 3456789\n 12345678 z");
    assert_eq!(
        source(&numeric[0]),
        &SourceRow { kind: RowKind::Context, number: Some(12), code: "3456789".into() },
        "the code was eaten as gutter"
    );
    assert_eq!(source(&numeric[1]).number, Some(12_345_678));
}

/// A line of code that itself begins with digits is code, not more gutter.
#[test]
fn code_starting_with_digits_is_not_read_as_a_number() {
    let rows = parse(&preview(&[(' ', Some(7), "42 is the answer")]));
    assert_eq!(source(&rows[0]).number, Some(7));
    assert_eq!(source(&rows[0]).code, "42 is the answer");
}

/// A diagnostics block's banners are notes, not a one-character addition and removal.
#[test]
fn injected_banners_are_notes() {
    let rows = parse("--- Diagnostics ---\n+++ b/src/lib.rs");
    assert!(rows.iter().all(|row| matches!(row, Row::Note(_))), "{rows:?}");
    assert!(replacements(&rows).is_empty());
}

/// A replacement pairs positionally; a surplus on either side is left unpaired.
#[test]
fn runs_pair_removals_with_their_replacements() {
    let rows = parse(&preview(&[
        ('-', Some(1), "let a = 1;"),
        ('-', Some(2), "let b = 2;"),
        ('+', Some(1), "let a = 9;"),
        (' ', Some(3), "let c = 3;"),
        ('+', Some(4), "let d = 4;"),
    ]));

    let found = replacements(&rows);
    assert_eq!(found[0], Replacement { removed: 0..2, added: 2..3 });
    // The lone addition after a context row is an insertion: no removals to pair with.
    assert_eq!(found[1], Replacement { removed: 4..4, added: 4..5 });

    let emphasis = intraline_emphasis(&rows);
    assert!(emphasis.contains_key(&0) && emphasis.contains_key(&2), "the pair is emphasized");
    assert!(!emphasis.contains_key(&1), "the unpaired removal has nothing to compare against");
    assert!(!emphasis.contains_key(&3), "context is never emphasized");
}

/// Two lines that share nothing are a rewrite, not an edit — there is no "changed span" in them
/// worth pointing at, and their own add/remove color already says everything.
#[test]
fn a_rewritten_line_pair_is_left_unemphasized() {
    let rows = parse(&preview(&[('-', Some(1), "alpha"), ('+', Some(1), "omega")]));

    assert_eq!(replacements(&rows).len(), 1, "the pair is still a replacement");
    assert!(intraline_emphasis(&rows).is_empty(), "but nothing in it is marked");
}

/// Emphasis covers only what differs, span by span, measured against the code rather than the row.
#[test]
fn emphasis_isolates_the_changed_span_within_the_code() {
    let rows = parse(&preview(&[
        (' ', Some(3), "    let value = 1;"),
        ('-', Some(4), "    let label: String = category.to_string();"),
        ('+', Some(4), "    let label = category.display_name();"),
        (' ', Some(5), "    emit(label);"),
    ]));
    let emphasis = intraline_emphasis(&rows);

    let marked = |row: usize| -> Vec<&str> {
        emphasis[&row].iter().map(|span| &source(&rows[row]).code[span.clone()]).collect()
    };
    // Word-level, so the unchanged `= category.` *between* the two edits stays unmarked — a
    // single contiguous span swept it up along with the shared `();` tail.
    assert_eq!(marked(1), [": String", "to_string"]);
    assert_eq!(marked(2), ["display_name"]);
}

#[test]
fn a_language_comes_from_the_extension_or_the_bare_file_name() {
    assert_eq!(language_for("crates/luxe-tui/src/view.rs").as_deref(), Some("rs"));
    assert_eq!(language_for("scripts/install.SH").as_deref(), Some("sh"));
    assert_eq!(language_for("Makefile").as_deref(), Some("makefile"));
    assert_eq!(language_for("  Dockerfile  ").as_deref(), Some("dockerfile"));
    assert_eq!(language_for(""), None);
}

/// An empty write line is still a line; a blank transcript line is not a row.
#[test]
fn blank_input_yields_no_rows_and_a_blank_row_keeps_its_number() {
    assert!(parse("").is_empty(), "nothing in, nothing out");
    let rows = parse(&preview(&[('+', Some(1), "one"), ('+', Some(2), ""), ('+', Some(3), "3")]));
    assert_eq!(rows.len(), 3);
    assert_eq!(
        source(&rows[1]),
        &SourceRow { kind: RowKind::Added, number: Some(2), code: String::new() }
    );
}
