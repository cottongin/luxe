use crate::compression::parse_test_output;

#[test]
fn parses_go() {
    let out = "--- FAIL: TestBad (0.00s)\n    m_test.go:4: got 2 want 3\nFAIL\nFAIL\tm\t0.001s\n";
    let s = parse_test_output(out, "").unwrap();
    assert_eq!(s.framework, "go");
    assert_eq!(s.failed, 1);
    assert_eq!(s.failures[0].name, "TestBad");
    assert_eq!(s.failures[0].location.as_deref(), Some("m_test.go:4"));
}

/// A real `go test -v ./...`, copied verbatim.
///
/// Verbose is the only mode that says anything about a test that passed — and it prints each
/// message *above* the verdict it belongs to, so the naive reading hands one failure's message
/// to the next one.
#[test]
fn parses_verbose_go_where_the_message_comes_before_its_verdict() {
    let out = "\
=== RUN   TestGood
--- PASS: TestGood (0.00s)
=== RUN   TestBad
    m_test.go:4: got 2 want 3
--- FAIL: TestBad (0.00s)
=== RUN   TestSkip
    m_test.go:5: later
--- SKIP: TestSkip (0.00s)
FAIL
FAIL\tm\t0.001s
FAIL
";
    let s = parse_test_output(out, "").expect("go output parses");
    assert_eq!((s.passed, s.failed, s.ignored), (1, 1, 1));
    assert_eq!(s.failures.len(), 1, "a skip is not a failure");
    assert_eq!(s.failures[0].name, "TestBad");
    assert_eq!(s.failures[0].location.as_deref(), Some("m_test.go:4"));
    assert_eq!(s.failures[0].message.as_deref(), Some("got 2 want 3"));
}

/// A test that reports twice keeps the *first* message, in both orderings.
///
/// `t.Errorf` does not stop the test, so a failure often prints several lines; the one that says
/// what broke is the first, and the last is usually a consequence of it.
#[test]
fn a_go_failure_that_says_several_things_is_summarized_by_its_first() {
    let verbose = "\
=== RUN   TestBad
    m_test.go:3: got 2 want 3
    m_test.go:3: and again
--- FAIL: TestBad (0.00s)
FAIL
";
    let buffered = "\
--- FAIL: TestBad (0.00s)
    m_test.go:3: got 2 want 3
    m_test.go:3: and again
FAIL
";
    for out in [verbose, buffered] {
        let s = parse_test_output(out, "").expect("go output parses");
        assert_eq!(s.failures[0].message.as_deref(), Some("got 2 want 3"), "{out}");
    }
}

/// go's passes are counted one `--- PASS:` at a time, and the package footer it ends with comes
/// in three spellings that all mean the same thing.
#[test]
fn go_counts_each_pass_and_accepts_every_footer_it_prints() {
    let out = "--- PASS: TestOne (0.00s)\n--- PASS: TestTwo (0.00s)\nPASS\nok  \tm\t0.5s\n";
    let s = parse_test_output(out, "").expect("go output parses");
    assert_eq!((s.passed, s.failed), (2, 0));

    for footer in ["ok \tm\t0.5s", "ok\tm\t0.5s", "PASS"] {
        assert!(parse_test_output(footer, "").is_some(), "{footer} ends a go run");
    }
    // A failure's location is the first `file.go:line:` under it, and only a go file's.
    let mislabeled = "--- FAIL: TestBad (0.00s)\n    note: something else\n    \
                       m_test.go:4: got 2 want 3\nFAIL\n";
    let s = parse_test_output(mislabeled, "").expect("go output parses");
    assert_eq!(s.failures[0].location.as_deref(), Some("m_test.go:4"));

    // A `word: text` line that names no go file is prose, and prose is not a location.
    let prose = "--- FAIL: TestBad (0.00s)\n    reason: the flag was unset\nFAIL\n";
    let s = parse_test_output(prose, "").expect("go output parses");
    assert!(s.failures[0].location.is_none(), "{:?}", s.failures[0]);
}
