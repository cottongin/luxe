//! Test gap — tests for the file-changed hook.

use super::*;

/// The advisory names the symbols, bounds the list, and leaves the decision to the agent.
#[test]
fn a_test_gap_reads_as_a_fact_with_a_choice() {
    let note = format_test_gaps("src/auth.rs", &["src/auth.rs::logout".to_string()])
        .expect("a gap is reported");

    assert!(note.starts_with("src/auth.rs: no covering test for logout"));
    assert!(note.contains("Add one, or say why"), "the agent decides: {note}");
    // The reviewer will be told the same thing, which is why saying so here is honest.
    assert!(note.contains("a reviewer will be told the same thing"));
}

#[test]
fn a_long_gap_list_is_counted_rather_than_pasted() {
    let symbols: Vec<String> = (0..9).map(|index| format!("src/lib.rs::symbol_{index}")).collect();

    let note = format_test_gaps("src/lib.rs", &symbols).expect("a gap is reported");

    assert!(note.contains("(+4 more)"), "the tail is counted: {note}");
    assert!(!note.contains("symbol_8"), "beyond the cap nothing is named: {note}");
}

#[test]
fn a_fully_covered_edit_says_nothing() {
    assert!(format_test_gaps("src/auth.rs", &[]).is_none());
}
