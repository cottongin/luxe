//! Python — pytest, and unittest with the runners built on it.

use super::{count_before, TestFailure, TestSummary};

pub(super) fn parse_pytest(text: &str) -> Option<TestSummary> {
    let mut summary = TestSummary { framework: "pytest".to_string(), ..Default::default() };
    let mut found = false;
    for line in text.lines() {
        // pytest centers its closing summary in a banner padded to the terminal width —
        // `======== 1 failed, 1 passed in 0.05s ========` — and only `-q` prints the bare line. So
        // the banner comes off first, or every default-verbosity run falls through to a raw dump.
        let t = line.trim().trim_matches('=').trim();
        // Summary line: `1 failed, 1 passed in 0.01s` / `2 passed in 0.1s`.
        if (t.contains(" passed") || t.contains(" failed"))
            && t.contains(" in ")
            && t.ends_with('s')
        {
            found = true;
            summary.passed = count_before(t, "passed").unwrap_or(summary.passed);
            summary.failed = count_before(t, "failed").unwrap_or(summary.failed);
            summary.ignored = count_before(t, "skipped").unwrap_or(summary.ignored);
        } else if let Some(rest) = t.strip_prefix("FAILED ") {
            let (name, message) = match rest.split_once(" - ") {
                Some((n, m)) => (n.trim(), Some(m.trim().to_string())),
                None => (rest.trim(), None),
            };
            summary.failures.push(TestFailure { name: name.to_string(), location: None, message });
        }
    }
    if found {
        Some(summary)
    } else {
        None
    }
}

/// Longest a failure's message line may be before it is cut. A Django assertion can print every
/// query it captured on one line, thousands of characters each, and the first few hundred say what
/// went wrong.
const UNITTEST_MESSAGE_CELLS: usize = 300;

/// Lines of a failure's message kept after the exception's own line.
const UNITTEST_MESSAGE_LINES: usize = 4;

/// One `FAIL:`/`ERROR:` block: where it failed and what was asserted.
fn unittest_failure(name: &str, body: &[&str]) -> TestFailure {
    let location = body.iter().find_map(|line| {
        let rest = line.trim().strip_prefix("File \"")?;
        let (path, tail) = rest.split_once("\", line ")?;
        let number: String = tail.chars().take_while(char::is_ascii_digit).collect();
        Some(format!("{path}:{number}"))
    });
    let exception = body.iter().position(|line| {
        let line = line.trim_start();
        !line.is_empty()
            && !line.starts_with("File ")
            && !line.starts_with("Traceback")
            && !line.starts_with("---")
            && !line.starts_with(' ')
            && line.split_once(':').is_some_and(|(kind, _)| {
                kind.chars().all(|c| c.is_alphanumeric() || c == '.' || c == '_')
            })
    });
    let message = exception.map(|at| {
        body[at..]
            .iter()
            .map(|line| line.trim_end())
            .filter(|line| !line.is_empty() && !line.starts_with("---"))
            .take(UNITTEST_MESSAGE_LINES + 1)
            .map(|line| match line.chars().count() > UNITTEST_MESSAGE_CELLS {
                true => {
                    let kept: String = line.chars().take(UNITTEST_MESSAGE_CELLS).collect();
                    format!(
                        "{kept}… ({} more characters)",
                        line.chars().count() - UNITTEST_MESSAGE_CELLS
                    )
                }
                false => line.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    });
    TestFailure { name: name.trim().to_string(), location, message }
}

/// `unittest`: `Ran 45 tests in 0.311s`, then `OK` or `FAILED (failures=2, errors=1, skipped=1)`,
/// with a `FAIL:`/`ERROR:` block per failure above it.
///
/// Each failure keeps the test's own line (unittest strips its frames, so a traceback's first is
/// always the test) and what was asserted, and nothing else: the frames below it are plumbing, and
/// a message line is cut at [`UNITTEST_MESSAGE_CELLS`].
pub(super) fn parse_unittest(text: &str) -> Option<TestSummary> {
    let lines: Vec<&str> = text.lines().collect();
    let ran = lines.iter().find_map(|line| {
        let rest = line.trim().strip_prefix("Ran ")?;
        let (count, tail) = rest.split_once(' ')?;
        (tail.starts_with("test") && tail.contains(" in ")).then(|| count.parse::<usize>().ok())?
    })?;
    let verdict =
        lines.iter().rev().map(|line| line.trim()).find(|line| {
            *line == "OK" || line.starts_with("OK (") || line.starts_with("FAILED (")
        })?;
    let count = |key: &str| {
        verdict
            .split(['(', ')', ','])
            .filter_map(|part| part.trim().split_once('='))
            .filter(|(name, _)| *name == key)
            .filter_map(|(_, value)| value.parse::<usize>().ok())
            .sum::<usize>()
    };
    let failed = count("failures") + count("errors") + count("unexpected successes");
    let ignored = count("skipped") + count("expected failures");
    let mut summary = TestSummary {
        framework: "unittest".to_string(),
        passed: ran.saturating_sub(failed + ignored),
        failed,
        ignored,
        failures: Vec::new(),
    };
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim();
        let Some(name) = line.strip_prefix("FAIL: ").or_else(|| line.strip_prefix("ERROR: "))
        else {
            index += 1;
            continue;
        };
        // The block runs to the next banner; its traceback starts after the dashed rule
        let mut end = index + 1;
        while end < lines.len()
            && !lines[end].starts_with("======")
            && !lines[end].trim().starts_with("Ran ")
        {
            end += 1;
        }
        let body = &lines[index + 1..end];
        summary.failures.push(unittest_failure(name, body));
        index = end;
    }
    Some(summary)
}

#[cfg(test)]
mod tests;
