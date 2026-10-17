use crate::compression::parse_test_output;

/// Django's `runtests.py` is `unittest` underneath. The literal is a real run's tail from the
/// harness eval, where the whole of it reached the model: 48 KB, most of it one assertion
/// listing every query it captured on a single line.
#[test]
fn a_unittest_run_is_summarized_and_its_long_lines_cut() {
    let long = format!(
        "2. DELETE FROM \"delete_user\" WHERE (\"delete_user\".\"avatar_id\" IN ({}))",
        "1, ".repeat(900)
    );
    let stderr = format!(
        "Creating test database for alias 'default'...\n..s......FF.....\n\
             ======================================================================\n\
             FAIL: test_large_delete (delete.tests.DeletionTests)\n\
             ----------------------------------------------------------------------\n\
             Traceback (most recent call last):\n\
             \x20 File \"/repo/tests/delete/tests.py\", line 369, in test_large_delete\n\
             \x20   self.assertNumQueries(queries, Avatar.objects.all().delete)\n\
             \x20 File \"/repo/django/test/testcases.py\", line 78, in __exit__\n\
             \x20   self.test_case.assertEqual(\n\
             AssertionError: 22 != 25 : 22 queries executed, 25 expected\n\
             Captured queries were:\n\
             1. SELECT \"delete_avatar\".\"id\" FROM \"delete_avatar\"\n\
             {long}\n\
             \n\
             ======================================================================\n\
             ERROR: test_fast_delete (delete.tests.FastDeleteTests)\n\
             ----------------------------------------------------------------------\n\
             Traceback (most recent call last):\n\
             \x20 File \"/repo/tests/delete/tests.py\", line 512, in test_fast_delete\n\
             \x20   u.delete()\n\
             ValueError: boom\n\
             \n\
             ----------------------------------------------------------------------\n\
             Ran 45 tests in 0.311s\n\
             \n\
             FAILED (failures=1, errors=1, skipped=1)\n\
             Destroying test database for alias 'default'...\n"
    );
    let summary = parse_test_output("", &stderr).expect("unittest is recognized");
    assert_eq!(summary.framework, "unittest");
    assert_eq!((summary.passed, summary.failed, summary.ignored), (42, 2, 1));
    assert_eq!(summary.failures[0].name, "test_large_delete (delete.tests.DeletionTests)");
    assert_eq!(summary.failures[0].location.as_deref(), Some("/repo/tests/delete/tests.py:369"));
    let message = summary.failures[0].message.as_deref().unwrap_or_default();
    assert!(message.starts_with("AssertionError: 22 != 25"), "{message}");
    assert!(message.contains("more characters"), "the long line is cut: {message}");
    assert_eq!(summary.failures[1].message.as_deref(), Some("ValueError: boom"));
    assert!(summary.render().len() < 1_500, "{}", summary.render());

    let green = "....\n----------------------------------------------------------------------\nRan 4 tests in 0.01s\n\nOK\n";
    let summary = parse_test_output("", green).expect("a green run too");
    assert!(summary.ok() && summary.passed == 4);
}

#[test]
fn parses_pytest() {
    let out = "FAILED test_x.py::test_bad - assert (1 + 1) == 3\n\
            1 failed, 1 passed in 0.01s\n";
    let s = parse_test_output(out, "").unwrap();
    assert_eq!(s.framework, "pytest");
    assert_eq!((s.passed, s.failed), (1, 1));
    assert_eq!(s.failures[0].name, "test_x.py::test_bad");
    assert_eq!(s.failures[0].message.as_deref(), Some("assert (1 + 1) == 3"));
}

/// The tail of a real `python -m pytest` run, copied verbatim.
///
/// The hand-written fixture above is `-q` output, and every line of it is unpadded — so a
/// parser that only understood that shape passed its tests while returning `None` for every
/// default-verbosity run any user or agent would actually produce.
#[test]
fn parses_the_banner_pytest_really_prints() {
    let out = "\
=========================== short test summary info ============================
 FAILED test_x.py::test_bad - assert (1 + 1) == 3
==================== 1 failed, 1 passed, 1 skipped in 0.01s ====================
";
    let s = parse_test_output(out, "").expect("a banner summary is still a summary");
    assert_eq!((s.passed, s.failed, s.ignored), (1, 1, 1));
    assert_eq!(s.failures[0].name, "test_x.py::test_bad");

    let green =
        "============================== 1 passed in 0.00s ===============================\n";
    let s = parse_test_output(green, "").expect("and so is an all-green one");
    assert_eq!((s.passed, s.failed), (1, 0));
}

/// pytest's summary line is identified by shape, and each part of that shape is load-bearing:
/// an all-green run never says "failed", and a line naming a count without a duration is a
/// test's own output rather than the summary.
#[test]
fn the_pytest_summary_line_is_recognized_by_its_whole_shape() {
    let green = parse_test_output("2 passed in 0.10s\n", "").expect("an all-green run");
    assert_eq!((green.passed, green.failed), (2, 0));

    assert!(
        parse_test_output("the build passed review\n", "").is_none(),
        "a count with no duration is not a summary"
    );
    assert!(
        parse_test_output("1 passed in the first attempt\n", "").is_none(),
        "and neither is one that does not end in a duration"
    );
}
