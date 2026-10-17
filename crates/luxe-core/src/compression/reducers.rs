//! Command-specific output reducers — currently the structured test reducer.
//!
//! The test reducer parses a test runner's noisy output into a compact, high-signal summary
//! (framework, pass/fail/skipped counts, failing test names + file:line + message) so the model
//! sees *what failed and where* instead of scrolling a wall of log. It's the most-structured
//! reducer on the compression pipeline; on an unrecognized format it falls back to the generic
//! pipeline so no output is lost. Shared by the `bash` registry and the standalone `test` tool.

use super::stages::strip_ansi;
use super::{CommandInvocation, CommandOutput, CompressionConfig, OutputReducer};

mod go;
mod javascript;
mod python;
mod rust;
mod summary;

use go::parse_go;
use javascript::{parse_jest, parse_node, parse_vitest};
use python::{parse_pytest, parse_unittest};
use rust::{parse_cargo, parse_nextest};
pub use summary::{TestFailure, TestSummary};

/// The structured test reducer.
pub struct TestReducer;

impl OutputReducer for TestReducer {
    fn id(&self) -> &str {
        "test"
    }

    fn can_handle(&self, command: &CommandInvocation) -> bool {
        is_test_command(command)
    }

    fn reduce(&self, _command: &CommandInvocation, output: &CommandOutput) -> String {
        match parse_test_output(&output.stdout, &output.stderr) {
            Some(summary) => summary.render(),
            // Unknown format: don't lose the output — fall back to the generic pipeline.
            None => super::generic_reduce(output, &CompressionConfig::default()),
        }
    }
}

/// Whether a command looks like a test invocation (used to route the reducer + the `test` tool).
pub fn is_test_command(command: &CommandInvocation) -> bool {
    let exe = command.executable.as_str();
    let has = |needle: &str| command.args.iter().any(|a| a == needle);
    match exe {
        "cargo" => has("test") || has("nextest"),
        "go" => has("test"),
        "pytest" | "jest" | "vitest" | "tox" | "nox" => true,
        // Django's own suite, and a project's `manage.py test`
        "runtests.py" => true,
        python if python.starts_with("python") => {
            has("pytest")
                || has("unittest")
                || command.args.first().is_some_and(|script| script.ends_with("runtests.py"))
                || (command.args.first().is_some_and(|script| script.ends_with("manage.py"))
                    && command.args.get(1).is_some_and(|sub| sub == "test"))
                || (has("django") && has("test"))
        }
        "django-admin" => command.args.first().is_some_and(|sub| sub == "test"),
        "npm" | "pnpm" | "yarn" | "bun" => has("test"),
        "npx" => command.args.iter().any(|a| a == "jest" || a == "vitest"),
        _ => false,
    }
}

/// Parse test output, trying each framework in a robust order.
pub fn parse_test_output(stdout: &str, stderr: &str) -> Option<TestSummary> {
    let text = strip_ansi(&format!("{stdout}\n{stderr}"));
    // Order matters: nextest embeds a libtest `test result:` line, so detect it first.
    parse_nextest(&text)
        .or_else(|| parse_cargo(&text))
        .or_else(|| parse_pytest(&text))
        // `unittest` and the runners built on it (Django's `runtests.py`, `manage.py test`)
        .or_else(|| parse_unittest(&text))
        .or_else(|| parse_vitest(&text))
        .or_else(|| parse_jest(&text))
        // Ahead of go, whose `ok <package>` lines also match every TAP `ok 1 - name`
        .or_else(|| parse_node(&text))
        .or_else(|| parse_go(&text))
}

/// `<num> <keyword>` occurring in `line` (e.g. `12 passed`).
fn count_before(line: &str, keyword: &str) -> Option<usize> {
    let idx = line.find(keyword)?;
    let digits: String =
        line[..idx].trim_end().chars().rev().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    digits.chars().rev().collect::<String>().parse().ok()
}

fn push_unique(failures: &mut Vec<TestFailure>, name: &str) {
    if !failures.iter().any(|f| f.name == name) {
        failures.push(TestFailure { name: name.to_string(), location: None, message: None });
    }
}

#[cfg(test)]
mod tests;
