use super::*;

/// The marked text on each side, which is what a reader would actually see emphasized.
fn marked(removed: &str, added: &str) -> (Vec<String>, Vec<String>) {
    let change = intraline_change(removed, added);
    let slice = |line: &str, ranges: &[Range<usize>]| {
        ranges.iter().map(|range| line[range.clone()].to_string()).collect()
    };
    (slice(removed, &change.removed), slice(added, &change.added))
}

/// The case a single contiguous span gets wrong: two separate edits with unchanged words between
/// them. Trimming a common prefix and suffix marks `string like '2'`, three quarters of which did
/// not change.
///
/// The sides need not have the same span count. Here the digit survives the edit — only the quotes
/// around it died — so the addition marks the word that changed and leaves `2` alone, while the
/// removal shows the quoted literal going away.
#[test]
fn separate_edits_on_one_line_are_marked_separately() {
    let (removed, added) = marked("/// takes a string like '2'", "/// takes a number like 2");

    assert_eq!(removed, ["string", "'2'"]);
    assert_eq!(added, ["number"]);
}

/// Whole words, not the characters inside them: a lone `i`/`o` is invisible and tells the reader
/// nothing about which identifier moved.
#[test]
fn a_changed_word_is_marked_whole() {
    let (removed, added) = marked("let value: String = x;", "let value: Strong = x;");

    assert_eq!(removed, ["String"]);
    assert_eq!(added, ["Strong"]);
}

/// The everyday case, and the one the old single-span version also handled.
#[test]
fn a_replaced_type_is_the_only_span() {
    let (removed, added) =
        marked("    pub category: String,", "    pub category: NotificationCategory,");

    assert_eq!(removed, ["String"]);
    assert_eq!(added, ["NotificationCategory"]);
}

/// An edit that removes a chunk and renames a call in the same line: three spans on the left,
/// one on the right, and the unchanged `= category.` between them left alone.
#[test]
fn several_edits_in_one_line_each_get_their_own_span() {
    let (removed, added) = marked(
        "    let label: String = category.to_string();",
        "    let label = category.display_name();",
    );

    assert_eq!(removed, [": String", "to_string"]);
    assert_eq!(added, ["display_name"]);
}

#[test]
fn identical_lines_have_nothing_to_mark() {
    assert!(intraline_change("same", "same").is_empty());
}

/// Two lines with nothing in common are a rewrite; their own colors already say so, and marking
/// all of both would only paint the row.
#[test]
fn a_rewrite_is_not_marked() {
    assert!(intraline_change("abc", "xyz").is_empty());
    assert!(intraline_change("let ready = compute(a, b);", "return;").is_empty());
    // Right at the edge: `x` survives on the left, but almost nothing does on the right.
    assert!(intraline_change("x", "some_long_expression(x)").is_empty());
}

/// Re-indentation must not light up a line's left margin.
#[test]
fn whitespace_only_changes_are_not_marked() {
    assert!(intraline_change("  let x = 1;", "        let x = 1;").is_empty());
    assert!(intraline_change("let x = 1;", "let  x  =  1;").is_empty());
}

/// Whitespace next to a real edit belongs to neither side of it.
#[test]
fn a_span_never_starts_or_ends_on_whitespace() {
    let (removed, added) = marked("    if ready {", "    if let Some(x) = ready {");
    for span in removed.iter().chain(&added) {
        assert_eq!(span.trim(), span, "span {span:?} carries whitespace at an edge");
    }
}

/// A character or two of unchanged text between two edits merges, so the result reads as spans
/// rather than stripes.
#[test]
fn runs_a_character_or_two_apart_merge() {
    // Both quotes changed and the digit did not; three spans would stripe the literal.
    let (removed, _) = marked("value = '2'", "value = 2");
    assert_eq!(removed, ["'2'"]);

    // A lone surviving space is a gap too: dropping `: String` is one edit, not two.
    let (removed, _) = marked("let label: String = x;", "let label = x;");
    assert_eq!(removed, [": String"]);

    // A whole unchanged word between two edits is a real gap and stays one.
    let (removed, added) = marked("alpha keep omega", "gamma keep delta");
    assert_eq!(removed, ["alpha", "omega"]);
    assert_eq!(added, ["gamma", "delta"]);
}

/// A pathological line must not run the quadratic step; it degrades to the differing middle.
///
/// The middle has to be past the cap *after* the shared head and tail are trimmed, and has to have
/// shared tokens between its edits. Then the LCS would mark each edit on its own, and one span over
/// the whole middle can only be the cap's doing: with the guard gone this finds a hundred ranges.
/// The outer context is long enough that the fallback still passes the survival filter.
#[test]
fn an_enormous_line_falls_back_to_one_span() {
    let outer = "context_word ".repeat(40);
    let middle = |tag: &str| (0..100).map(|n| format!("{tag}{n} shared_{n} ")).collect::<String>();
    let removed = format!("{outer}{}end_of_line", middle("old"));
    let added = format!("{outer}{}end_of_line", middle("new"));

    let change = intraline_change(&removed, &added);

    assert_eq!(change.removed.len(), 1, "one contiguous span past the cap");
    assert_eq!(change.added.len(), 1);
    // from the first edit to the last, the shared tokens between them included
    let (removed_span, added_span) =
        (&removed[change.removed[0].clone()], &added[change.added[0].clone()]);
    assert!(removed_span.starts_with("old0 ") && removed_span.ends_with("old99"), "{removed_span}");
    assert!(added_span.starts_with("new0 ") && added_span.ends_with("new99"), "{added_span}");
}

/// Under the cap the same shape is marked edit by edit, which is what makes the test above mean
/// something: the one span is the fallback, not how every line of this form reads.
#[test]
fn a_line_under_the_cap_marks_each_edit_on_its_own() {
    let middle = |tag: &str| (0..10).map(|n| format!("{tag}{n} shared_{n} ")).collect::<String>();
    let removed = format!("context_word {}end_of_line", middle("old"));
    let added = format!("context_word {}end_of_line", middle("new"));

    let change = intraline_change(&removed, &added);

    assert_eq!(change.removed.len(), 10, "{:?}", change.removed);
}

/// Ranges are handed straight to a slicing renderer, so they must always be sliceable.
#[test]
fn ranges_are_ordered_disjoint_and_land_on_character_boundaries() {
    let cases = [
        ("let name = \"café\";", "let name = \"naïve\";"),
        ("値 = 1", "値 = 2"),
        ("emoji 🎉 tail", "emoji 🚀 tail"),
        ("a\tb\tc", "a\tb\td"),
        ("", "added"),
        ("removed", ""),
        ("mixed 界 width", "mixed 界 depth"),
    ];
    for (removed, added) in cases {
        let change = intraline_change(removed, added);
        for (line, ranges) in [(removed, &change.removed), (added, &change.added)] {
            let mut previous = 0usize;
            for range in ranges {
                assert!(range.start >= previous, "{line:?}: {ranges:?} overlap or unsorted");
                assert!(range.end <= line.len(), "{line:?}: {ranges:?} out of bounds");
                assert!(range.start < range.end, "{line:?}: {ranges:?} has an empty span");
                assert!(line.is_char_boundary(range.start), "{line:?}: {range:?} splits a char");
                assert!(line.is_char_boundary(range.end), "{line:?}: {range:?} splits a char");
                previous = range.end;
            }
        }
    }
}

/// Multi-byte content is marked by word like anything else, and the ranges address bytes.
#[test]
fn multibyte_words_are_marked_whole() {
    let (removed, added) = marked("greet(\"héllo\");", "greet(\"wörld\");");
    assert_eq!(removed, ["héllo"]);
    assert_eq!(added, ["wörld"]);
}

/// An outright insertion or deletion of a whole line has no counterpart worth comparing.
#[test]
fn an_empty_side_is_not_marked() {
    assert!(intraline_change("", "added").is_empty());
    assert!(intraline_change("removed", "").is_empty());
}
