use super::*;
use crate::compression::parse_test_output;
use crate::compression::reducers::javascript::tests::NODE_SPEC;

/// What `render` writes, `parse_rendered` reads back — from inside the `test` tool's whole
/// reply, with the command above it and notes below.
#[test]
fn a_rendered_summary_reads_back() {
    let summary = parse_test_output(NODE_SPEC, "").expect("node");
    let reply = format!("$ npm test\n{}\n\nFull output: x.log\n\nExit code: 1", summary.render());
    assert_eq!(TestSummary::parse_rendered(&reply), Some(summary));
    let green = TestSummary { framework: "cargo".into(), passed: 12, ..Default::default() };
    assert_eq!(TestSummary::parse_rendered(&green.render()), Some(green));
    assert_eq!(TestSummary::parse_rendered("$ npm test\nall good"), None);
}

/// The skipped count survives the trip: a literal summary as `render` writes it, and a rendered
/// one with failures and skips both read back with it.
///
/// The break this catches: `render` wrote `N skipped` and the parser looked for `ignored`, so every
/// test card drew zero skipped.
#[test]
fn a_rendered_summary_keeps_its_skipped_count() {
    let literal = TestSummary::parse_rendered("cargo: ok — 12 passed, 0 failed, 3 skipped");
    assert_eq!(literal.map(|summary| summary.ignored), Some(3));
    let mut summary = parse_test_output(NODE_SPEC, "").expect("node");
    summary.ignored = 4;
    assert!(summary.failed > 0, "a summary with failures too");
    assert_eq!(TestSummary::parse_rendered(&summary.render()), Some(summary));
    let older = TestSummary::parse_rendered("cargo: ok — 1 passed, 0 failed, 2 ignored");
    assert_eq!(older.map(|summary| summary.ignored), Some(2), "older output still reads");
}

/// The verdict word every rendering leads with.
#[test]
fn a_run_is_ok_only_with_no_failures() {
    let mut summary = TestSummary { passed: 3, ..Default::default() };
    assert!(summary.ok());
    assert!(summary.render().contains("ok —"));
    summary.failed = 1;
    assert!(!summary.ok());
    assert!(summary.render().contains("FAILED —"));
}

#[test]
fn renders_compact_summary() {
    let out = "test tests::a ... FAILED\ntest result: FAILED. 0 passed; 1 failed; 0 ignored;\n";
    let rendered = parse_test_output(out, "").unwrap().render();
    assert!(rendered.contains("cargo: FAILED — 0 passed, 1 failed"));
    assert!(rendered.contains("✗ tests::a"));
}
