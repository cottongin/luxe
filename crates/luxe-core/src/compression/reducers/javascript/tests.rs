use super::*;
use crate::compression::parse_test_output;

/// What Node 25's spec reporter prints for a suite with one failure, captured from a real run.
pub(crate) const NODE_SPEC: &str = r#"✔ dueBy keeps everything up to the cutoff (0.601294ms)
✔ open sorts newest first and drops what is done (3.922396ms)
✔ tagged wants all of them, not any (0.078751ms)
✖ dueBy treats an unpadded cutoff as the same day (#12) (0.625144ms)
ℹ tests 4
ℹ suites 0
ℹ pass 3
ℹ fail 1
ℹ cancelled 0
ℹ skipped 0
ℹ todo 0
ℹ duration_ms 34.644023

✖ failing tests:

test at test/filter.test.js:31:1
✖ dueBy treats an unpadded cutoff as the same day (#12) (0.625144ms)
  AssertionError [ERR_ASSERTION]: Expected values to be strictly deep-equal:
  + actual - expected
  
    [
      'Ship the docs',
  +   'Fix the parser',
  +   'Answer Priya'
    ]
  
      at TestContext.<anonymous> (file:///work/todo/test/filter.test.js:32:10)
      at Test.runInAsyncScope (node:async_hooks:226:14)
      at Test.run (node:internal/test_runner/test:1201:25)
      at Test.processPendingSubtests (node:internal/test_runner/test:831:18)
      at Test.postRun (node:internal/test_runner/test:1330:19)
      at Test.run (node:internal/test_runner/test:1258:12)
      at async Test.processPendingSubtests (node:internal/test_runner/test:831:7) {
    generatedMessage: true,
    code: 'ERR_ASSERTION',
    actual: [ 'Ship the docs', 'Fix the parser', 'Answer Priya' ],
    expected: [ 'Ship the docs' ],
    operator: 'deepStrictEqual',
    diff: 'simple'
  }
"#;

/// The same run through `--test-reporter=tap`.
const NODE_TAP: &str = r#"TAP version 13
# Subtest: dueBy keeps everything up to the cutoff
ok 1 - dueBy keeps everything up to the cutoff
  ---
  duration_ms: 0.620974
  type: 'test'
  ...
# Subtest: open sorts newest first and drops what is done
ok 2 - open sorts newest first and drops what is done
  ---
  duration_ms: 3.898016
  type: 'test'
  ...
# Subtest: tagged wants all of them, not any
ok 3 - tagged wants all of them, not any
  ---
  duration_ms: 0.072731
  type: 'test'
  ...
# Subtest: dueBy treats an unpadded cutoff as the same day (\#12)
not ok 4 - dueBy treats an unpadded cutoff as the same day (\#12)
  ---
  duration_ms: 0.548544
  type: 'test'
  location: '/work/todo/test/filter.test.js:31:1'
  failureType: 'testCodeFailure'
  error: |-
    Expected values to be strictly deep-equal:
    + actual - expected
    
      [
        'Ship the docs',
    +   'Fix the parser',
    +   'Answer Priya'
      ]
    
  code: 'ERR_ASSERTION'
  name: 'AssertionError'
  expected:
    0: 'Ship the docs'
  actual:
    0: 'Ship the docs'
    1: 'Fix the parser'
    2: 'Answer Priya'
  operator: 'deepStrictEqual'
  stack: |-
    TestContext.<anonymous> (file:///work/todo/test/filter.test.js:32:10)
    Test.runInAsyncScope (node:async_hooks:226:14)
    Test.run (node:internal/test_runner/test:1201:25)
    Test.processPendingSubtests (node:internal/test_runner/test:831:18)
    Test.postRun (node:internal/test_runner/test:1330:19)
    Test.run (node:internal/test_runner/test:1258:12)
    async Test.processPendingSubtests (node:internal/test_runner/test:831:7)
  ...
1..4
# tests 4
# suites 0
# pass 3
# fail 1
# cancelled 0
# skipped 0
# todo 0
# duration_ms 32.488839
"#;

/// One failing assertion comes back as its name, where it is, and the diff that says what to
/// change — not the forty lines of runner chatter around it.
#[test]
fn node_spec_output_reduces_to_its_failure() {
    let summary = parse_test_output(NODE_SPEC, "").expect("node's spec reporter");
    assert_eq!(summary.framework, "node");
    assert_eq!((summary.passed, summary.failed, summary.ignored), (3, 1, 0));
    assert_eq!(summary.failures.len(), 1);
    let failure = &summary.failures[0];
    assert_eq!(failure.name, "dueBy treats an unpadded cutoff as the same day (#12)");
    assert_eq!(failure.location.as_deref(), Some("test/filter.test.js:31"));
    let message = failure.message.as_deref().expect("the assertion");
    assert!(message.starts_with("AssertionError"), "{message}");
    assert!(message.contains("'Fix the parser'"), "the diff is kept: {message}");
    assert!(!message.contains("async_hooks"), "the stack is not: {message}");
    assert!(summary.render().lines().count() < 16, "{}", summary.render());
}

/// TAP carries the same facts in YAML: the name unescaped, the location, the error block.
#[test]
fn node_tap_output_reduces_to_its_failure() {
    let summary = parse_test_output(NODE_TAP, "").expect("node's TAP reporter");
    assert_eq!(summary.framework, "node", "{summary:?}");
    assert_eq!((summary.passed, summary.failed), (3, 1));
    let failure = &summary.failures[0];
    assert_eq!(failure.name, "dueBy treats an unpadded cutoff as the same day (#12)");
    assert_eq!(failure.location.as_deref(), Some("/work/todo/test/filter.test.js:31"));
    let message = failure.message.as_deref().expect("the error block");
    assert!(message.starts_with("Expected values to be strictly deep-equal"), "{message}");
    assert!(message.contains("'Answer Priya'"), "{message}");
    assert!(!message.contains("code:"), "the block ends with its indent: {message}");
}

/// A file that crashed on import, as Node 25's spec reporter prints it: the crash first, the
/// results after, and the file's own failure carrying nothing but `'test failed'`.
const NODE_SPEC_CRASH: &str = "file:///work/todo/test/filter.test.js:4
import { dueBy, isoDay, open, tagged } from \"../src/filter.js\";
                ^^^^^^
SyntaxError: The requested module '../src/filter.js' does not provide an export named 'isoDay'
    at #asyncInstantiate (node:internal/modules/esm/module_job:326:21)
    at async ModuleJob.run (node:internal/modules/esm/module_job:429:5)

Node.js v25.9.0
\u{2716} test/filter.test.js (17.829508ms)
\u{2139} tests 1
\u{2139} suites 0
\u{2139} pass 0
\u{2139} fail 1
\u{2139} cancelled 0
\u{2139} skipped 0
\u{2139} todo 0
\u{2139} duration_ms 23.942601

\u{2716} failing tests:

test at test/filter.test.js:1:1
\u{2716} test/filter.test.js (17.829508ms)
  'test failed'
";

/// The same crash under TAP, as comments, beside a top-level throw in a second file that
/// prints a blank line before its error.
const NODE_TAP_CRASH: &str = "TAP version 13
# file:///work/todo/test/filter.test.js:4
# import { dueBy, isoDay, open, tagged } from \"../src/filter.js\";
#                 ^^^^^^
# SyntaxError: The requested module '../src/filter.js' does not provide an export named 'isoDay'
#     at \\#asyncInstantiate (node:internal/modules/esm/module_job:326:21)
# Node.js v25.9.0
# Subtest: test/filter.test.js
not ok 1 - test/filter.test.js
  ---
  duration_ms: 16.531108
  type: 'test'
  location: '/work/todo/test/filter.test.js:1:1'
  failureType: 'testCodeFailure'
  exitCode: 1
  error: 'test failed'
  code: 'ERR_TEST_FAILURE'
  ...
# file:///work/todo/test/throw.test.js:2
# const x = notDefined + 1;
#           ^
#
# ReferenceError: notDefined is not defined
#     at file:///work/todo/test/throw.test.js:2:11
# Subtest: test/throw.test.js
not ok 2 - test/throw.test.js
  ---
  location: '/work/todo/test/throw.test.js:1:1'
  error: 'test failed'
  ...
1..2
# tests 2
# pass 0
# fail 2
";

/// A file that never loaded fails for the reason it never loaded, at the line that stopped it.
#[test]
fn a_test_file_that_crashed_says_why() {
    let summary = parse_test_output(NODE_SPEC_CRASH, "").expect("node's spec reporter");
    let failure = &summary.failures[0];
    assert_eq!(failure.location.as_deref(), Some("test/filter.test.js:4"));
    let message = failure.message.as_deref().expect("the crash");
    assert!(
        message.starts_with(
            "SyntaxError: The requested module '../src/filter.js' does not provide an export \
                 named 'isoDay'\nimport { dueBy, isoDay"
        ),
        "{message}"
    );
    assert!(message.ends_with("^^^^^^"), "the caret points at it: {message}");
    assert!(!message.contains("module_job"), "no stack: {message}");

    let summary = parse_test_output(NODE_TAP_CRASH, "").expect("node's TAP reporter");
    let [import, throw] = &summary.failures[..] else { panic!("{summary:?}") };
    assert_eq!(import.location.as_deref(), Some("/work/todo/test/filter.test.js:4"));
    assert!(import.message.as_deref().unwrap().starts_with("SyntaxError: "), "{import:?}");
    assert_eq!(throw.location.as_deref(), Some("/work/todo/test/throw.test.js:2"));
    assert_eq!(
        throw.message.as_deref(),
        Some("ReferenceError: notDefined is not defined\nconst x = notDefined + 1;\n          ^"),
        "each crash to its own file, past the blank line"
    );
}

/// An assertion that happens to start a line like an error is not a crash, and a placeholder
/// with no crash to explain it stays as Node wrote it.
#[test]
fn only_a_crash_explains_a_file_failure() {
    let unexplained = NODE_SPEC_CRASH.split_once("Node.js v25.9.0\n").unwrap().1;
    let summary = parse_test_output(unexplained, "").expect("node");
    assert_eq!(summary.failures[0].message.as_deref(), Some("'test failed'"));
    assert_eq!(summary.failures[0].location.as_deref(), Some("test/filter.test.js:1"));
    let no_line = "file:///work/a.js:here\nSyntaxError: nope\n";
    assert!(node_file_crashes(no_line).is_empty(), "a crash header ends in its line");
    assert!(is_error_headline("Error [ERR_MODULE_NOT_FOUND]: Cannot find package 'x'"));
    assert!(is_error_headline("TypeError: x is not a function"));
    assert!(!is_error_headline("    at Error: nope"));
    assert!(!is_error_headline("AssertionError [ERR_ASSERTION] something"));
    assert!(!is_error_headline("Some Error: words"));
}

/// A green run is a count and nothing else, and a run with no Node markers is not Node's.
#[test]
fn node_counts_without_failures_and_nothing_else_is_node() {
    let green = "ℹ tests 3\nℹ suites 0\nℹ pass 3\nℹ fail 0\nℹ skipped 1\nℹ todo 1\n";
    let summary = parse_node(green).expect("counts alone");
    assert_eq!((summary.passed, summary.failed, summary.ignored), (3, 0, 2));
    assert!(summary.failures.is_empty() && summary.ok());
    assert!(parse_node("ℹ pass 3\n").is_none(), "no test count, not a run summary");
    assert!(parse_node("all good\n").is_none());
}

#[test]
fn parses_jest() {
    let out = "✕ renders the header (4 ms)\n\
            Tests:       1 failed, 2 passed, 1 skipped, 4 total\n";
    let s = parse_test_output(out, "").expect("jest output parses");
    assert_eq!(s.framework, "jest");
    assert_eq!((s.passed, s.failed, s.ignored), (2, 1, 1));
    assert_eq!(s.failures[0].name, "renders the header");
}

/// The tail of a real `npx jest` run, copied verbatim.
///
/// Default output names its failures with `●`, not `✕` — the tick list is verbose mode. So the
/// counts parsed and the names, which are the reason to call this tool at all, came back empty.
#[test]
fn parses_the_failure_names_jest_really_prints() {
    let out = "\
FAIL ./x.test.js
  ● bad

  ● Console

FAIL ./y.test.js
  ● math › multiplies

Test Suites: 2 failed, 2 total
Tests:       2 failed, 1 skipped, 2 passed, 5 total
";
    let s = parse_test_output(out, "").expect("jest output parses");
    assert_eq!(s.framework, "jest");
    assert_eq!((s.passed, s.failed, s.ignored), (2, 2, 1));
    assert_eq!(
        s.failures.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        ["bad", "math › multiplies"],
        "a console section is not a failing test"
    );
}

/// The tail of a real `npx vitest run`, copied verbatim.
#[test]
fn parses_vitest_which_reports_the_same_facts_in_another_shape() {
    let out = "\
 ❯ x.test.js (3 tests | 1 failed | 1 skipped) 3ms
   × bad 2ms

 FAIL  x.test.js > bad
AssertionError: expected 1 to be 2

 Test Files  1 failed (1)
      Tests  1 failed | 1 passed | 1 skipped (3)
   Duration  65ms
";
    let s = parse_test_output(out, "").expect("vitest output parses");
    assert_eq!(s.framework, "vitest", "named after the runner that produced it");
    assert_eq!((s.passed, s.failed, s.ignored), (1, 1, 1));
    assert!(
        s.failures.iter().any(|f| f.name == "x.test.js > bad"),
        "a name that reads back as a filter: {:?}",
        s.failures
    );
}
