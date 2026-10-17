use super::*;

fn table(header: &[&str], body: &[&[&str]]) -> TableBlock {
    let cells = |row: &[&str]| row.iter().map(|cell| cell.to_string()).collect::<Vec<_>>();
    TableBlock::new(
        cells(header),
        body.iter().map(|row| cells(row)).collect(),
        &[],
        Style::default(),
        Style::default(),
    )
    .expect("columns")
}

fn text(lines: &[Line<'static>]) -> Vec<String> {
    lines.iter().map(Line::to_string).collect()
}

fn widths(lines: &[Line<'static>]) -> Vec<usize> {
    lines.iter().map(Line::width).collect()
}

const SAMPLE: [[&str; 3]; 3] = [
    ["Item", "Status", "Notes"],
    ["Notifications", "done", "permission, question and review prompts notify"],
    ["Peek width", "planned", "cap the diff at code width so the transcript keeps room"],
];

fn source() -> Vec<String> {
    SAMPLE.iter().map(|row| row.join(" ")).collect()
}

/// Every letter and digit in `rows`, sorted: the content, whatever the layout did to it.
fn letters(rows: &[String]) -> Vec<char> {
    let mut letters: Vec<char> =
        rows.iter().flat_map(|row| row.chars()).filter(|c| c.is_alphanumeric()).collect();
    letters.sort_unstable();
    letters
}

fn sample() -> TableBlock {
    table(
        &["Item", "Status", "Notes"],
        &[
            &["Notifications", "done", "permission, question and review prompts notify"],
            &["Peek width", "planned", "cap the diff at code width so the transcript keeps room"],
        ],
    )
}

/// With room, the grid is what it always was: one line a row, no rules between rows.
#[test]
fn a_table_with_room_keeps_its_natural_grid() {
    let lines = sample().lines_at(200);
    let rows = text(&lines);
    assert_eq!(rows.len(), 6, "{rows:#?}");
    assert!(rows[1].contains("Item") && rows[3].contains("Notifications"));
    let first = lines[0].width();
    assert!(widths(&lines).iter().all(|&width| width == first), "{rows:#?}");
}

/// The bug this exists for: narrower than its natural width, every line of the table still fits
/// and every border still lines up, the cells wrapping inside their own columns.
#[test]
fn a_narrow_table_wraps_inside_its_columns_and_stays_a_grid() {
    for width in [40usize, 56, 72, 90] {
        let lines = sample().lines_at(width);
        let rows = text(&lines);
        let first = lines[0].width();
        assert!(first <= width, "{width}: {rows:#?}");
        assert!(widths(&lines).iter().all(|&w| w == first), "{width}: aligned {rows:#?}");
        assert!(rows.iter().all(|row| row.starts_with(['┌', '│', '├', '└'])), "{rows:#?}");
        assert_eq!(letters(&rows), letters(&source()), "{width}: nothing is cut {rows:#?}");
    }
}

/// Once a row takes more than one line, rules between rows say where one ends.
#[test]
fn wrapped_rows_are_ruled_apart() {
    let rows = text(&sample().lines_at(50));
    let separators = rows.iter().filter(|row| row.starts_with('├')).count();
    assert_eq!(separators, 2, "under the header, and between the two rows: {rows:#?}");
}

/// A column too narrow for any grid gives the table up for records, still nothing cut.
#[test]
fn too_narrow_for_a_grid_reads_as_records() {
    let rows = text(&sample().lines_at(20));
    assert!(rows.iter().all(|row| UnicodeWidthStr::width(row.as_str()) <= 20), "{rows:#?}");
    assert!(rows[0].starts_with("Item: Notifications"), "{rows:#?}");
    assert!(rows.iter().any(|row| row.starts_with("Status: done")));
    assert!(rows.iter().any(|row| row.starts_with('─')), "rows ruled apart: {rows:#?}");
    // Each record names its header again, so that is the content to account for
    let records: Vec<String> = SAMPLE[1..]
        .iter()
        .flat_map(|row| SAMPLE[0].iter().zip(row).map(|(label, value)| format!("{label}{value}")))
        .collect();
    assert_eq!(letters(&rows), letters(&records), "{rows:#?}");
}

#[test]
fn a_word_longer_than_its_column_is_split_not_dropped() {
    assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
    assert_eq!(wrap("a bb ccc", 4), ["a bb", "ccc"]);
    assert_eq!(wrap("", 4), [""]);
}

#[test]
fn alignment_pads_each_line() {
    assert_eq!(pad("ab", 5, Alignment::Right), "   ab");
    assert_eq!(pad("ab", 5, Alignment::Center), " ab  ");
    assert_eq!(pad("ab", 5, Alignment::Left), "ab   ");
    assert_eq!(Alignment::from(pulldown_cmark::Alignment::Right), Alignment::Right);
    assert_eq!(Alignment::from(pulldown_cmark::Alignment::Center), Alignment::Center);
    assert_eq!(Alignment::from(pulldown_cmark::Alignment::None), Alignment::Left);
}

/// The room left after every column's word goes to whoever is shortest of what they want, and
/// nobody gets more than they asked for.
#[test]
fn spare_room_goes_where_it_is_wanted() {
    assert_eq!(share(&[3, 3], &[10, 3], 4), [7, 3]);
    assert_eq!(share(&[3, 3], &[5, 5], 100), [5, 5], "never past what is wanted");
    assert_eq!(share(&[4, 4, 4], &[20, 10, 4], 9), [13, 4, 4], "the neediest first");
    assert_eq!(share(&[4, 4, 4], &[20, 10, 4], 18), [18, 8, 4], "until the shortfalls even out");
    assert_eq!(share(&[4, 4], &[9, 9], 3), [6, 5], "a tie goes to the first");
}

fn widths_of(table: &TableBlock, width: usize) -> Option<Vec<usize>> {
    table.column_widths(width)
}

/// Short of every column's longest word, the columns shrink together, each in proportion to its
/// word; below six cells a column, the grid is given up for records.
#[test]
fn too_narrow_for_every_word_the_columns_shrink_together() {
    let long = table(
        &["Identifier", "Kind", "Description"],
        &[&["abcdefghijklmnopqrst", "status", "verylongwordhere"]],
    );
    // Words 16 (capped), 6 and 16: 38 cells, against a budget of 30
    assert_eq!(widths_of(&long, 30 + 10), Some(vec![12, 6, 12]));
    assert_eq!(widths_of(&long, 18 + 10), Some(vec![6, 6, 6]), "the narrowest grid there is");
    // The widest gives way first, so two columns keep their words whole and one splits
    let uneven = table(&["a", "b", "c"], &[&["abcdefghijklmnopqrst", "abcdefgh", "abcdefgh"]]);
    assert_eq!(widths_of(&uneven, 24 + 10), Some(vec![8, 8, 8]));
    assert_eq!(widths_of(&long, 17 + 10), None, "one cell short: records");
    let rows = text(&long.lines_at(28));
    let first = long.lines_at(28)[0].width();
    assert_eq!(first, 28, "the grid fills the width exactly: {rows:#?}");
}

/// With every word in, the room left over grows the columns that want it.
#[test]
fn with_every_word_in_the_rest_goes_to_who_wants_it() {
    let wide = table(&["Name", "Notes"], &[&["alpha", &"word ".repeat(20)]]);
    // Words 5 and 4; wants 5 and 48; budget 40 leaves 31 to share, all of it wanted by Notes
    assert_eq!(widths_of(&wide, 40 + 7), Some(vec![5, 35]));
}

/// A record whose header is wider than half the row puts its value under it, indented.
#[test]
fn a_long_header_puts_its_record_value_underneath() {
    let narrow =
        table(&["A rather long header", "Status", "B"], &[&["value here and more", "", "x"]]);
    assert!(widths_of(&narrow, 12).is_none(), "too narrow for a grid");
    let rows = text(&narrow.lines_at(12));
    assert_eq!(
        rows,
        ["A rather", "long header:", "  value here", "  and more", "Status:", "B: x"],
        "an empty value adds no blank line"
    );
    // So narrow there is no room for an indent, the value takes the whole row
    let rows = text(&table(&["Header", "B"], &[&["ab", "c"]]).lines_at(2));
    assert!(rows.contains(&"ab".to_string()), "{rows:#?}");
}

#[test]
fn trimming_never_takes_a_column_below_one_cell() {
    assert_eq!(fit(vec![1, 1], 1), [1, 1]);
    assert_eq!(fit(vec![9, 4, 9], 18), [7, 4, 7]);
}

/// The rules between rows sit between rows: not doubled under the header, not missing below.
#[test]
fn a_rule_separates_each_pair_of_wrapped_rows() {
    let rows = text(&sample().lines_at(50));
    let peek = rows.iter().position(|row| row.contains("Peek width")).expect("the row");
    assert!(rows[peek - 1].starts_with('├'), "{rows:#?}");
    let header_rule = rows.iter().position(|row| row.starts_with('├')).expect("a rule");
    assert!(rows[header_rule + 1].starts_with('│'), "one rule under the header: {rows:#?}");
}
