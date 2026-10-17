use super::*;

/// Two versions of a file preview as the change between them, numbered against the first.
#[test]
fn two_versions_preview_as_one_change() {
    let before = "fn a() {}\nfn sub(a:i32)->i32{a}\nfn c() {}\n";
    let after = "fn a() {}\nfn sub(a: i32) -> i32 {\n    a\n}\nfn c() {}\n";
    let shown = texts_diff(before, after);
    assert!(shown.contains("-2 fn sub(a:i32)->i32{a}"), "{shown}");
    assert!(shown.contains("+2 fn sub(a: i32) -> i32 {"), "{shown}");
    assert!(shown.contains("+4 }"), "{shown}");
    assert!(
        !shown.contains("-1") && !shown.contains("+1 "),
        "unchanged lines are context: {shown}"
    );
    // multibyte text on both sides of the change keeps the slices on char boundaries
    let shown = texts_diff("é→a←é", "é→b←é");
    assert!(shown.contains("-1 é→a←é") && shown.contains("+1 é→b←é"), "{shown}");
    assert_eq!(texts_diff("same\n", "same\n"), ranges_diff("same\n", &[(5..5, "")]));
}

#[test]
fn a_change_shows_line_numbers_and_context_without_hunk_headers() {
    let original = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight";
    let diff = edits_diff(original, &[("four", "FOUR")]);

    assert!(!diff.contains("@@"), "no unified hunk headers: {diff}");
    assert!(diff.contains("-4 four"), "the removed line is numbered: {diff}");
    assert!(diff.contains("+4 FOUR"), "the added line is numbered: {diff}");
    assert!(diff.contains(" 3 three"), "context above: {diff}");
    assert!(diff.contains(" 5 five"), "context below: {diff}");
    assert!(diff.contains("..."), "the elided tail is marked: {diff}");
    assert!(!diff.contains("eight"), "far-away lines aren't shown: {diff}");
}

#[test]
fn a_change_at_the_top_has_no_leading_elision() {
    let original = "alpha\nbeta\ngamma";
    let diff = edits_diff(original, &[("alpha", "ALPHA")]);
    assert!(diff.starts_with("-1 alpha"), "starts right at the change: {diff:?}");
    assert!(!diff.contains("..."), "everything else is context: {diff}");
}

#[test]
fn a_mid_line_edit_shows_the_whole_before_and_after_line() {
    let original = "fn main() {\n    println!(\"old\");\n}";
    let diff = edits_diff(original, &[("\"old\"", "\"new\"")]);
    assert!(diff.contains("-2     println!(\"old\");"), "{diff}");
    assert!(diff.contains("+2     println!(\"new\");"), "{diff}");
}

#[test]
fn multi_line_and_multi_edit_changes_keep_their_positions() {
    let original = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten";
    let diff = edits_diff(original, &[("two\nthree", "TWO"), ("nine", "NINE")]);
    // The gutter is two wide here (line 10 exists), so numbers are right-aligned into it.
    assert!(diff.contains("- 2 two"), "{diff}");
    assert!(diff.contains("- 3 three"), "{diff}");
    assert!(diff.contains("+ 2 TWO"), "{diff}");
    assert!(diff.contains("- 9 nine"), "{diff}");
    assert!(diff.contains("+ 9 NINE"), "{diff}");
    // These two changes are close enough that their context blocks meet, so nothing is elided.
    assert!(!diff.contains("..."), "adjacent context blocks merge: {diff}");
}

#[test]
fn a_far_apart_pair_of_edits_elides_the_span_between_them() {
    let original = (1..=40).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
    let diff = edits_diff(&original, &[("line 5", "FIVE"), ("line 35", "THIRTY-FIVE")]);
    assert!(diff.matches("...").count() >= 3, "head, middle, and tail are elided: {diff}");
}

#[test]
fn the_gutter_is_wide_enough_for_the_largest_line_number() {
    let original = (1..=120).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
    let diff = edits_diff(&original, &[("line 100", "LINE 100")]);
    assert!(diff.contains("-100 line 100"), "{diff}");
    assert!(diff.contains("  99 line 99"), "context aligns under it: {diff}");
}

/// An append shows the appended line, not the whole matched block rewritten.
///
/// The model routinely matches a block it is only adding to; printing every unchanged line as
/// a removal *and* an addition made a one-line append read like a rewrite, and the model reads
/// this result back.
#[test]
fn an_insertion_shows_only_what_was_inserted() {
    let diff = edits_diff("keep this", &[("keep this", "keep this\nplus a line")]);
    assert!(diff.contains("+2 plus a line"), "the new line is an addition: {diff}");
    assert!(diff.contains(" 1 keep this"), "the untouched line is context: {diff}");
    assert!(!diff.contains("-1 keep this"), "and never a removal: {diff}");
    assert!(!diff.contains("\n-\n"), "no empty removal row: {diff:?}");
}

/// The same, at the scale it was reported at: appending one line to a three-line file.
#[test]
fn appending_to_a_small_file_prints_one_added_row() {
    let original = "alpha\nbeta\ngamma\n";
    let diff = edits_diff(original, &[(original, "alpha\nbeta\ngamma\nzeta\n")]);
    let removals = diff.lines().filter(|line| line.starts_with('-')).count();
    let additions = diff.lines().filter(|line| line.starts_with('+')).count();
    assert_eq!(removals, 0, "nothing was removed: {diff}");
    assert_eq!(additions, 1, "exactly one line was added: {diff}");
    assert!(diff.contains("+4 zeta"), "{diff}");
}

/// A model that matches a whole block to change one line in it gets a preview of the change.
///
/// The lines a match covers but does not touch are ordinary file lines: three of them either
/// side, like any other context, and the rest elided. Printing all nine put the edit below the
/// fold of every surface reading this — a collapsed tool card was ten rows of scenery — and
/// charged the model for the scenery on the way past.
#[test]
fn a_big_match_around_a_small_change_previews_the_change_not_the_match() {
    let original = (1..=40).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
    let matched = (20..=32).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
    let replaced = matched.replace("line 30", "LINE 30");
    let diff = edits_diff(&original, &[(matched.as_str(), replaced.as_str())]);

    let rows: Vec<&str> = diff.lines().collect();
    assert_eq!(rows[0].trim(), "...", "everything above the change is elided: {diff}");
    assert_eq!(rows[1], " 27 line 27", "three lines of context, no more: {diff}");
    assert_eq!(rows[4], "-30 line 30", "{diff}");
    assert_eq!(rows[5], "+30 LINE 30", "{diff}");
    assert_eq!(rows[8], " 33 line 33", "{diff}");
    assert_eq!(rows[9].trim(), "...", "and everything below it: {diff}");
    assert_eq!(rows.len(), 10, "nothing else earns a row: {diff}");
    assert!(!diff.contains("line 20"), "the head of the match is not scenery: {diff}");
}

/// Two changes far apart keep their own context blocks, with the span between them elided.
#[test]
fn two_distant_changes_keep_their_own_context() {
    let original = (1..=60).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
    let diff = edits_diff(&original, &[("line 10", "TEN"), ("line 50", "FIFTY")]);

    assert!(diff.contains("-10 line 10") && diff.contains("+10 TEN"), "{diff}");
    assert!(diff.contains("-50 line 50") && diff.contains("+50 FIFTY"), "{diff}");
    assert_eq!(diff.matches("...").count(), 3, "head, middle and tail: {diff}");
    assert!(!diff.contains("line 20"), "the span between them is not drawn: {diff}");
}

#[test]
fn added_lines_numbers_every_line() {
    let out = added_lines("fn main() {}\n// note");
    assert!(out.contains("+1 fn main() {}"), "{out}");
    assert!(out.contains("+2 // note"), "{out}");
}
