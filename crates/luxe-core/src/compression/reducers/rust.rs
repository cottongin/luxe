//! Rust — libtest and nextest runs, with each failure's panic message.

use super::{count_before, push_unique, TestSummary};

pub(super) fn parse_cargo(text: &str) -> Option<TestSummary> {
    let mut summary = TestSummary { framework: "cargo".to_string(), ..Default::default() };
    let mut found = false;
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("test result:") {
            found = true;
            summary.passed += count_before(rest, "passed").unwrap_or(0);
            summary.failed += count_before(rest, "failed").unwrap_or(0);
            summary.ignored += count_before(rest, "ignored").unwrap_or(0);
        } else if let Some(name) =
            t.strip_prefix("test ").and_then(|s| s.strip_suffix(" ... FAILED"))
        {
            push_unique(&mut summary.failures, name.trim());
        }
    }
    if found {
        enrich_with_panics(text, &mut summary);
        Some(summary)
    } else {
        None
    }
}

pub(super) fn parse_nextest(text: &str) -> Option<TestSummary> {
    let mut summary = TestSummary { framework: "nextest".to_string(), ..Default::default() };
    let mut found = false;
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("Summary [").and_then(|s| s.split(']').nth(1)) {
            found = true;
            summary.passed = count_before(rest, "passed").unwrap_or(0);
            summary.failed = count_before(rest, "failed").unwrap_or(0);
            summary.ignored = count_before(rest, "skipped").unwrap_or(0);
        } else if let Some(rest) = t.strip_prefix("FAIL [") {
            // `FAIL [   0.002s] (2/2) crate tests::name` → keep the crate + test name tail.
            if let Some(name) = rest.split(')').nth(1) {
                push_unique(&mut summary.failures, name.trim());
            }
        }
    }
    if found {
        enrich_with_panics(text, &mut summary);
        Some(summary)
    } else {
        None
    }
}

/// Attach panic `file:line` + message to matching Rust failures (`thread 'name' panicked at loc:`).
fn enrich_with_panics(text: &str, summary: &mut TestSummary) {
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix("thread '") else {
            continue;
        };
        let Some((name, after)) = rest.split_once('\'') else {
            continue;
        };
        let Some(loc) = after.split("panicked at ").nth(1).map(|l| l.trim_end_matches(':')) else {
            continue;
        };
        let message = panic_body(&lines, i);
        if let Some(failure) = summary.failures.iter_mut().find(|f| names_same_test(&f.name, name))
        {
            failure.location.get_or_insert_with(|| loc.to_string());
            if failure.message.is_none() {
                failure.message = message;
            }
        }
    }
}

/// Whether a parsed failure name and a panic's thread name refer to the same test.
///
/// nextest prefixes every failure with the binary that ran it (`crate::bin/name tests::foo`) and
/// the panic line does not carry it, so equality alone matched nothing on a nextest run and every
/// failure came back with a bare name and no assertion text. Comparing the last whitespace-
/// separated segment keeps the binary id out of it without the false positives a substring test
/// would invite (`bar_foo` must not match `foo`).
fn names_same_test(failure_name: &str, thread_name: &str) -> bool {
    failure_name == thread_name
        || failure_name.rsplit(char::is_whitespace).next() == Some(thread_name)
}

/// Panic body lines the summary keeps, past which a reader is better served by the artifact file.
const MAX_PANIC_LINES: usize = 6;

/// The panic body under a `thread '…' panicked at …:` line.
///
/// More than one line, because the line that matters for `assert_eq!` is not the first one — the
/// `left:`/`right:` values under it are the whole reason to read the failure. Stops at the runner's
/// own trailers so a backtrace hint never lands in the summary.
fn panic_body(lines: &[&str], panic_line: usize) -> Option<String> {
    let mut body: Vec<&str> = Vec::new();
    for raw in lines.iter().skip(panic_line + 1).take(MAX_PANIC_LINES) {
        let line = raw.trim();
        if line.is_empty()
            || line.starts_with("note:")
            || line.starts_with("thread '")
            || line.starts_with("stack backtrace")
        {
            break;
        }
        body.push(line);
    }
    (!body.is_empty()).then(|| body.join("\n"))
}

#[cfg(test)]
mod tests;
