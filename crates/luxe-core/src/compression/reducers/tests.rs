use super::*;

#[test]
fn detects_test_commands() {
    assert!(is_test_command(&CommandInvocation::parse("cargo test")));
    assert!(is_test_command(&CommandInvocation::parse("cargo nextest run")));
    assert!(is_test_command(&CommandInvocation::parse("go test ./...")));
    assert!(is_test_command(&CommandInvocation::parse("pytest -q")));
    assert!(is_test_command(&CommandInvocation::parse("python3 -m pytest")));
    assert!(is_test_command(&CommandInvocation::parse("npm test")));
    // Both runners `npx` fronts, and neither of them is every `npx` invocation.
    assert!(is_test_command(&CommandInvocation::parse("npx jest")));
    assert!(is_test_command(&CommandInvocation::parse("npx vitest run")));
    assert!(!is_test_command(&CommandInvocation::parse("npx tsc")));
    // How models run Django's suite, and a Django project's
    assert!(is_test_command(&CommandInvocation::parse(
        "cd tests && ../.venv/bin/python runtests.py --settings=test_sqlite x 2>&1 | tail -5"
    )));
    assert!(is_test_command(&CommandInvocation::parse("./tests/runtests.py admin_views")));
    assert!(is_test_command(&CommandInvocation::parse("python3.8 manage.py test app")));
    assert!(is_test_command(&CommandInvocation::parse("python -m django test")));
    assert!(!is_test_command(&CommandInvocation::parse("python manage.py migrate")));
    assert!(!is_test_command(&CommandInvocation::parse("python setup.py build")));
    assert!(!is_test_command(&CommandInvocation::parse("cargo build")));
    assert!(!is_test_command(&CommandInvocation::parse("ls -la")));
}

/// The reducer is how `bash "cargo test"` gets the same summary the `test` tool renders, so
/// its routing and its output are both load-bearing.
#[test]
fn the_reducer_claims_test_commands_and_renders_their_summary() {
    let reducer = TestReducer;
    assert_eq!(reducer.id(), "test");
    assert!(reducer.can_handle(&CommandInvocation::parse("cargo test")));
    assert!(!reducer.can_handle(&CommandInvocation::parse("cargo build")));

    let output = CommandOutput {
        stdout: "test tests::a ... FAILED\ntest result: FAILED. 0 passed; 1 failed; 0 \
                     ignored;\n"
            .into(),
        stderr: String::new(),
        exit_code: Some(101),
    };
    let reduced = reducer.reduce(&CommandInvocation::parse("cargo test"), &output);
    assert!(reduced.contains("1 failed"), "the summary survives the reducer: {reduced}");
    assert!(reduced.contains("tests::a"), "{reduced}");
}

/// nextest prints each failure twice — once inline, once in the closing list.
#[test]
fn a_failure_named_twice_is_listed_once() {
    let out = "        FAIL [   0.002s] (1/1) ct tests::boom\n\
            Summary [   0.002s] 1 test run: 0 passed, 1 failed, 0 skipped\n\
            \x20       FAIL [   0.002s] (1/1) ct tests::boom\n";
    let s = parse_test_output(out, "").expect("nextest output parses");
    assert_eq!(s.failures.len(), 1, "{:?}", s.failures);
}

#[test]
fn unknown_output_does_not_parse() {
    assert!(parse_test_output("just some logs\nno test markers\n", "").is_none());
}
