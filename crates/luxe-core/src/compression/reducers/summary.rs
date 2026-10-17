//! Summary — a parsed test run, its model-facing rendering, and reading that rendering back.

use super::count_before;

/// A parsed test-run summary.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TestSummary {
    /// Detected framework id (`cargo`, `nextest`, `pytest`, `go`, `jest`).
    pub framework: String,
    pub passed: usize,
    pub failed: usize,
    pub ignored: usize,
    /// Failing tests, with best-effort location and message.
    pub failures: Vec<TestFailure>,
}

/// A single failing test.
#[derive(Debug, PartialEq, Eq)]
pub struct TestFailure {
    pub name: String,
    pub location: Option<String>,
    pub message: Option<String>,
}

impl TestSummary {
    /// Whether the run passed (no failures).
    pub fn ok(&self) -> bool {
        self.failed == 0
    }

    /// A compact, model-facing rendering.
    pub fn render(&self) -> String {
        let verdict = if self.ok() { "ok" } else { "FAILED" };
        let mut out = format!(
            "{}: {verdict} — {} passed, {} failed, {} skipped",
            self.framework, self.passed, self.failed, self.ignored
        );
        if !self.failures.is_empty() {
            out.push_str("\nFailures:");
            for failure in &self.failures {
                out.push_str(&format!("\n  ✗ {}", failure.name));
                if let Some(loc) = &failure.location {
                    out.push_str(&format!("  ({loc})"));
                }
                // A panic body can be several lines (`assert_eq!`'s left/right), and each one
                // has to carry the indent or the block stops reading as the failure's detail.
                if let Some(msg) = &failure.message {
                    for line in msg.trim().lines() {
                        out.push_str(&format!("\n      {line}"));
                    }
                }
            }
        }
        out
    }

    /// Reads back what [`Self::render`] wrote, wherever it sits in the `test` tool's reply, for a
    /// front-end drawing a test card. Messages come back whole, but only what the reply carried.
    /// The skipped count reads as `render` writes it, or as `ignored`, as older replies had it.
    pub fn parse_rendered(text: &str) -> Option<Self> {
        let mut lines = text.lines().skip_while(|line| !line.contains(" passed, "));
        let head = lines.next()?;
        let (framework, rest) = head.split_once(": ")?;
        if framework.contains(' ') {
            return None;
        }
        let counts = rest.split_once(" — ")?.1;
        let mut summary = TestSummary {
            framework: framework.to_string(),
            passed: count_before(counts, "passed")?,
            failed: count_before(counts, "failed")?,
            ignored: ["skipped", "ignored"]
                .iter()
                .find_map(|w| count_before(counts, w))
                .unwrap_or(0),
            failures: Vec::new(),
        };
        if lines.next().map(str::trim) != Some("Failures:") {
            return Some(summary);
        }
        for line in lines {
            if let Some(entry) = line.strip_prefix("  ✗ ") {
                let (name, location) = match entry.rsplit_once("  (") {
                    Some((name, location)) if location.ends_with(')') => {
                        (name, Some(location.trim_end_matches(')').to_string()))
                    }
                    _ => (entry, None),
                };
                summary.failures.push(TestFailure {
                    name: name.to_string(),
                    location,
                    message: None,
                });
            } else if let Some(detail) = line.strip_prefix("      ") {
                let Some(failure) = summary.failures.last_mut() else { break };
                let message = failure.message.get_or_insert_with(String::new);
                if !message.is_empty() {
                    message.push('\n');
                }
                message.push_str(detail);
            } else {
                break;
            }
        }
        Some(summary)
    }
}

#[cfg(test)]
mod tests;
