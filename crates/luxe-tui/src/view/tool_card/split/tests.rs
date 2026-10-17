use super::*;
use crate::view::tool_card::test_support::{context, plain, theme};
use ratatui::style::{Color, Modifier};

fn sides(line: &Line<'static>) -> (String, String) {
    let text = plain(line);
    let (left, right) = text.split_once(DIVIDER).expect("a divided row");
    (left.to_string(), right.to_string())
}

/// The *display column* the rule sits at — not its byte offset, which a `↳` (three bytes, one
/// cell) would put two past where the rule actually renders.
fn divider_column(line: &Line<'static>) -> Option<usize> {
    let text = plain(line);
    text.split_once(DIVIDER).map(|(left, _)| left.width())
}

/// A replacement is one row: what it was on the left, what it became on the right.
#[test]
fn a_replacement_sits_across_the_rule() {
    let styles = theme();
    let rows = super::super::rows::parse(" 11 fn main() {\n-12     old();\n+12     new();");
    let rendered = render(&rows, Some("rs"), &context(&styles, 80));

    assert_eq!(rendered.len(), 2, "context row, then the replacement as one row");
    let (left, right) = sides(&rendered[1].lines[0]);
    assert!(left.contains("old();"), "{left}");
    assert!(right.contains("new();"), "{right}");
    assert!(left.starts_with('-') && right.starts_with('+'), "{left}|{right}");
}

/// Context appears on both sides so the eye can follow one line across the rule.
#[test]
fn context_is_shown_on_both_sides() {
    let styles = theme();
    let rows = super::super::rows::parse(" 11 fn main() {");
    let (left, right) = sides(&render(&rows, Some("rs"), &context(&styles, 80))[0].lines[0]);
    assert!(left.contains("fn main() {") && right.contains("fn main() {"));
}

/// The rule has to sit at one screen column on every row, whatever the row holds.
#[test]
fn every_row_is_the_same_width_and_the_rule_never_moves() {
    let styles = theme();
    let rows = super::super::rows::parse(
        " 11 fn main() {\n-12     old();\n-13     also_old();\n+12     new();\n 14 }\n    ...",
    );
    let rendered = render(&rows, Some("rs"), &context(&styles, 80));

    let columns: std::collections::HashSet<usize> =
        rendered.iter().flat_map(|row| &row.lines).filter_map(divider_column).collect();
    assert_eq!(columns.len(), 1, "the rule wandered: {columns:?}");
    // The surplus removal keeps its own row with an empty right column.
    let (left, right) = sides(&rendered[2].lines[0]);
    assert!(left.contains("also_old();"), "{left}");
    assert!(right.trim().is_empty(), "a deletion has nothing on the right: {right:?}");
}

/// A changed row is washed, so an absent counterpart is visibly absent.
#[test]
fn changed_rows_wear_a_wash_and_context_does_not() {
    let styles = theme();
    let rows = super::super::rows::parse(" 11 kept\n-12 old\n+12 new");
    let rendered = render(&rows, None, &context(&styles, 80));

    assert!(
        rendered[0].lines[0].spans.iter().all(|span| span.style.bg.is_none()),
        "context is calm"
    );
    let washes: Vec<_> =
        rendered[1].lines[0].spans.iter().filter_map(|span| span.style.bg).collect();
    assert!(washes.contains(&styles.diff_del_bg.bg.expect("a del wash")));
    assert!(washes.contains(&styles.diff_add_bg.bg.expect("an add wash")));
}

/// Each side marks the spans that changed on it in bold, each wearing a deeper wash of its own
/// row's hue so the row wash cannot swallow it.
#[test]
fn a_paired_row_marks_the_changed_span_on_both_sides() {
    let styles = theme();
    let rows = super::super::rows::parse("-12     let a = one();\n+12     let a = two();");
    let line = &render(&rows, Some("rs"), &context(&styles, 100))[0].lines[0];

    let marked = |modifier: Modifier, wash: Option<Color>| -> Vec<&str> {
        line.spans
            .iter()
            .filter(|span| span.style.add_modifier.contains(modifier) && span.style.bg == wash)
            .map(|span| span.content.as_ref())
            .collect()
    };
    assert_eq!(marked(Modifier::BOLD, styles.diff_del_emphasis.bg), ["one"]);
    assert!(marked(Modifier::CROSSED_OUT, styles.diff_del_emphasis.bg).is_empty(), "unstruck");
    assert_eq!(marked(Modifier::BOLD, styles.diff_add_emphasis.bg), ["two"]);
}

/// A cell with no counterpart has nothing to compare against, so none of it is marked.
#[test]
fn an_unpaired_cell_is_never_marked() {
    let styles = theme();
    let rows = super::super::rows::parse("-12     let gone = 1;");
    let line = &render(&rows, Some("rs"), &context(&styles, 100))[0].lines[0];

    assert!(line.spans.iter().all(|span| span.style.bg != styles.diff_del_emphasis.bg), "{line:?}");
}

/// A row says whether the change is in it, and a collapsed card opens on the answer.
///
/// Side by side folds a replacement's two halves into one row, so this is the only thing that
/// knows where the change is; [`super::diff_window`] trusts it to put the edit above the fold
/// rather than the scenery over it. Context is calm on both sides, and a change with nothing
/// opposite it still counts — a deletion is the half of an edit most worth landing on.
#[test]
fn a_row_says_whether_the_change_is_in_it() {
    let styles = theme();
    let rows = super::super::rows::parse(
        " 11 kept\n-12 old\n+12 new\n-13 gone\n 14 kept\n+15 added\n    ...",
    );
    let rendered = render(&rows, Some("rs"), &context(&styles, 100));

    let changed: Vec<bool> = rendered.iter().map(|row| row.changed).collect();
    assert_eq!(
        changed,
        [false, true, true, false, true, false],
        "context, a replacement, a lone deletion, context, a lone insertion, an elision"
    );
}

/// Context in a column is code, so it is highlighted rather than painted one flat color.
#[test]
fn context_is_highlighted_inside_its_column() {
    let styles = theme();
    let rows = super::super::rows::parse(" 11 fn main() { let x = 1; }");
    let line = &render(&rows, Some("rs"), &context(&styles, 100))[0].lines[0];

    let colors: std::collections::HashSet<_> = line
        .spans
        .iter()
        .take_while(|span| span.content != DIVIDER)
        .skip(1)
        .filter(|span| !span.content.trim().is_empty())
        .map(|span| span.style.fg)
        .collect();
    assert!(colors.len() > 1, "the column is one flat color: {colors:?}");
}

/// A long line wraps on both sides; the pair takes as many rows as its longer side needs.
#[test]
fn a_wrapped_pair_stays_aligned() {
    let styles = theme();
    // Only the left side is long enough to wrap, so the right side's padding rows are the ones
    // that must not sprout a continuation marker.
    let rows = super::super::rows::parse(
        "-12     let previous = compute(alpha, beta, gamma, delta, epsilon);\n\
             +12     let next = 1;",
    );
    let rendered = render(&rows, Some("rs"), &context(&styles, 60));

    assert!(rendered[0].lines.len() > 1, "the longer side wrapped");
    let columns: std::collections::HashSet<usize> =
        rendered[0].lines.iter().filter_map(divider_column).collect();
    assert_eq!(columns.len(), 1, "the rule holds through a wrap: {columns:?}");
    let (left_wrap, right_wrap) = sides(&rendered[0].lines[1]);
    assert!(left_wrap.contains("↳ "), "the wrapped side is marked, with a space: {left_wrap:?}");
    assert!(
        !right_wrap.contains('↳'),
        "the side that did not wrap wears no marker: {right_wrap:?}"
    );
    let left: String = rendered[0].lines.iter().map(|line| sides(line).0).collect();
    assert!(left.contains("epsilon);"), "the whole line survives: {left}");
}

/// A tab-indented file lays out on the same columns as a space-indented one.
///
/// `unicode-width` scores a tab nothing and the terminal advances it to a stop, so a row that
/// still held one was laid out against a width the screen did not agree with. The rule stepped
/// right once per level of indentation, and the surplus pushed the row past the card — where
/// the transcript's word wrap stranded its tail back at the left margin, an addition
/// reappearing inside the column reserved for removals.
#[test]
fn a_tab_indented_diff_lands_on_the_same_columns_as_a_spaced_one() {
    let styles = theme();
    const TABBED: &str = concat!(
        " 415 \t\t\t<GettingStartedChecklist onCloseDrawer={closeDrawer} />\n",
        "-416 \t\t\t<div class=\"shrink-0 px-2.5 pb-1 max-sm:hidden\">\n",
        "-417 \t\t\t\t<button\n",
        "+416 \t\t\t{#if tourStore.isEnabled}\n",
        "+417 \t\t\t<div class=\"shrink-0 px-2.5 pb-1 max-sm:hidden\">",
    );
    let laid_out = |preview: &str| -> Vec<String> {
        render(&super::super::rows::parse(preview), Some("svelte"), &context(&styles, 120))
            .iter()
            .flat_map(|row| &row.lines)
            .map(plain)
            .collect()
    };

    // Every tab here starts on a stop, so the spaced twin is the same file byte for byte once
    // the tabs are spent — and has to draw identically.
    let tabbed = laid_out(TABBED);
    assert_eq!(tabbed, laid_out(&TABBED.replace('\t', "    ")));

    let columns: std::collections::HashSet<usize> = tabbed
        .iter()
        .filter_map(|row| row.split_once(DIVIDER).map(|(left, _)| left.width()))
        .collect();
    assert_eq!(columns.len(), 1, "the rule stepped with the indentation: {columns:?}");

    // The sign leads its column, which is what tells the two sides apart for a reader who
    // cannot rely on the color — and it only does so while the gutter is the width of what
    // goes in it.
    let replacement =
        tabbed.iter().find(|row| row.starts_with("-416")).expect("the replacement's own row");
    let (left, right) = replacement.split_once(DIVIDER).expect("a divided row");
    assert!(right.starts_with("+416"), "the new side leads with its sign too: {right:?}");
    assert!(left.contains("shrink-0"), "{left:?}");
    for row in &tabbed {
        assert!(row.width() <= 120, "wider than the card it was laid out for: {row:?}");
        assert!(!row.contains('\t'), "a tab reached a buffer cell: {row:?}");
    }
}

/// A column narrower than its own gutter must not underflow its way to a panic.
#[test]
fn an_absurdly_narrow_card_still_renders() {
    let styles = theme();
    let rows = super::super::rows::parse(" 11 kept\n    ...\n-12 old\n+12 new");
    for width in [0, 1, 4, 12] {
        let rendered = render(&rows, None, &context(&styles, width));
        assert_eq!(rendered.len(), 3, "still one group per logical row at width {width}");
    }
}
