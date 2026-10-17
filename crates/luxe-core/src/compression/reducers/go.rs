//! Go — `go test`, terse and verbose.

use super::{TestFailure, TestSummary};

pub(super) fn parse_go(text: &str) -> Option<TestSummary> {
    let mut summary = TestSummary { framework: "go".to_string(), ..Default::default() };
    let mut found = false;
    let mut current: Option<usize> = None;
    // A `t.Errorf` line printed before the verdict it belongs to. Under `-v` go interleaves output
    // with the run, so the message arrives *above* its `--- FAIL:` — the opposite of the buffered
    // order — and without holding it, the next failure would inherit the previous one's message.
    let mut pending: Option<(String, String)> = None;
    for line in text.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("--- FAIL: ") {
            found = true;
            let name = rest.split_whitespace().next().unwrap_or(rest);
            let (location, message) = match pending.take() {
                Some((location, message)) => (Some(location), Some(message)),
                None => (None, None),
            };
            summary.failures.push(TestFailure { name: name.to_string(), location, message });
            current = Some(summary.failures.len() - 1);
        } else if t.starts_with("--- PASS: ") {
            found = true;
            summary.passed += 1;
            (current, pending) = (None, None);
        } else if t.starts_with("--- SKIP: ") {
            found = true;
            summary.ignored += 1;
            (current, pending) = (None, None);
        } else if t.starts_with("=== RUN") {
            (current, pending) = (None, None);
        } else if t.starts_with("ok ") || t.starts_with("ok\t") || t == "PASS" {
            found = true;
        } else if let Some((loc, msg)) = t.split_once(": ").filter(|(loc, _)| loc.contains(".go:"))
        {
            // First `file.go:line: message` around a FAIL is its location + message.
            match current {
                Some(index) if summary.failures[index].location.is_none() => {
                    summary.failures[index].location = Some(loc.to_string());
                    summary.failures[index].message = Some(msg.to_string());
                }
                _ if pending.is_none() => pending = Some((loc.to_string(), msg.to_string())),
                _ => {}
            }
        }
    }
    summary.failed = summary.failures.len();
    if found {
        Some(summary)
    } else {
        None
    }
}

#[cfg(test)]
mod tests;
