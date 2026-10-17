use super::*;
use crate::compression::parse_test_output;

#[test]
fn parses_cargo_libtest() {
    let out = "running 3 tests\n\
            test tests::skipped ... ignored\n\
            test tests::ok_one ... ok\n\
            test tests::fails_here ... FAILED\n\n\
            thread 'tests::fails_here' (1) panicked at src/lib.rs:6:30:\n\
            assertion `left == right` failed\n\n\
            test result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";
    let s = parse_test_output(out, "").unwrap();
    assert_eq!((s.passed, s.failed, s.ignored), (1, 1, 1));
    assert_eq!(s.failures.len(), 1);
    assert_eq!(s.failures[0].name, "tests::fails_here");
    assert_eq!(s.failures[0].location.as_deref(), Some("src/lib.rs:6:30"));
    assert!(!s.ok());
}

#[test]
fn parses_nextest() {
    let out = "        PASS [   0.002s] (1/2) ct tests::ok_one\n\
                    FAIL [   0.002s] (2/2) ct tests::fails_here\n\
            Summary [   0.002s] 2 tests run: 1 passed, 1 failed, 1 skipped\n";
    let s = parse_test_output(out, "").unwrap();
    assert_eq!(s.framework, "nextest");
    assert_eq!((s.passed, s.failed, s.ignored), (1, 1, 1));
    assert_eq!(s.failures[0].name, "ct tests::fails_here");
}

/// Copied out of a real `cargo nextest run` artifact, because the defect lives in the gap
/// between the two name spellings nextest emits: the `FAIL [` line carries a binary id that
/// the panic's thread name does not. Matching them by equality found nothing, so every
/// nextest failure came back as a bare name and the assertion text stayed in the log file.
#[test]
fn nextest_failures_carry_their_location_and_assertion_text() {
    let out = "        FAIL [   0.004s] (3/4) lazymail::bin/lazymail \
             ui::statusbar::tests::pane_loading_uses_the_same_statusbar_spinner\n\
             \x20 stderr \u{2500}\u{2500}\u{2500}\n\n\
             \x20   thread 'ui::statusbar::tests::pane_loading_uses_the_same_statusbar_spinner' \
             (4032629) panicked at src/ui/statusbar.rs:345:9:\n\
             \x20   assertion failed: line_text(&build_status_line(&app, 80)).contains(\"\u{2502} \
             Fetching message\u{2026}\")\n\
             \x20   note: run with `RUST_BACKTRACE=1` environment variable to display a \
             backtrace\n\
             \x20    Summary [   0.006s] 4 tests run: 3 passed, 1 failed, 901 skipped\n";

    let summary = parse_test_output(out, "").expect("nextest output parses");
    assert_eq!(summary.framework, "nextest");
    assert_eq!((summary.passed, summary.failed), (3, 1));

    let failure = &summary.failures[0];
    assert_eq!(failure.location.as_deref(), Some("src/ui/statusbar.rs:345:9"));
    let message = failure.message.as_deref().expect("the assertion text is the whole point");
    assert!(message.starts_with("assertion failed:"), "{message}");
    assert!(!message.contains("RUST_BACKTRACE"), "the trailer is not the failure: {message}");
    assert!(summary.render().contains("src/ui/statusbar.rs:345:9"), "{}", summary.render());
}

/// The first line of an `assert_eq!` panic says nothing a reader did not already know; the
/// values under it are the reason to look.
#[test]
fn an_assert_eq_keeps_its_left_and_right_values() {
    let out = "test tests::compares ... FAILED\n\
            thread 'tests::compares' panicked at src/lib.rs:9:5:\n\
            assertion `left == right` failed\n\
            \x20 left: 1\n\
            right: 2\n\
            note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n\
            test result: FAILED. 0 passed; 1 failed; 0 ignored;\n";

    let summary = parse_test_output(out, "").expect("cargo output parses");
    let message = summary.failures[0].message.as_deref().expect("a panic body");
    assert!(message.contains("left: 1") && message.contains("right: 2"), "{message}");

    // Every line of the body carries the indent, or the block stops reading as one failure.
    let rendered = summary.render();
    assert!(rendered.contains("\n      left: 1"), "{rendered}");
}

/// The body stops at the runner's own trailers, each of which ends it on its own.
#[test]
fn a_panic_body_stops_at_every_trailer_that_ends_it() {
    let lines = ["thread 'x' panicked at a.rs:1:1:", "the message"];
    for trailer in ["", "note: run with `RUST_BACKTRACE=1`", "thread 'y' panicked at b.rs:2:2:"] {
        let mut with_trailer = lines.to_vec();
        with_trailer.push(trailer);
        with_trailer.push("not part of the failure");
        let body = panic_body(&with_trailer, 0).expect("a body");
        assert_eq!(body, "the message", "stopped at {trailer:?}");
    }
    // `stack backtrace:` ends it too, and is the one trailer with no `note:` in front of it.
    let backtrace = ["thread 'x' panicked at a.rs:1:1:", "stack backtrace:", "   0: core"];
    assert_eq!(panic_body(&backtrace, 0), None, "a backtrace is not a message");
}

/// A name is matched by its tail, not by containment — `bar_foo` is not `foo`.
#[test]
fn a_panic_never_enriches_a_different_test_that_merely_ends_alike() {
    assert!(names_same_test("ct tests::fails", "tests::fails"));
    assert!(names_same_test("tests::fails", "tests::fails"));
    assert!(!names_same_test("ct tests::bar_fails", "tests::fails"));
    assert!(!names_same_test("ct tests::fails_later", "tests::fails"));
}
