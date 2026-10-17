//! Important-line classification — the "keep the signal" heuristic shared by the generic pipeline
//! and (later) the structured test reducer.
//!
//! Deterministic, language-agnostic substring/shape checks. A line is important when dropping it
//! would cost the model actionable information: errors, warnings, failing tests, file:line
//! locations, exit/summary lines, stack frames, or auth/device-code prompts. When in doubt, keep.

/// Whether a line carries actionable signal that truncation/filtering must preserve.
pub fn is_important(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();

    // Diagnostics + failures (covers rustc `error[E…]`, `error:`, gcc/clang, python, generic).
    const MARKERS: &[&str] = &[
        "error",
        "warning:",
        "warning[",
        "warn:",
        "failed",
        "failure",
        "fail:",
        "panic",
        "fatal",
        "exception",
        "traceback",
        "assert",
        "cannot find",
        "not found",
        "unresolved",
        "undefined",
        "undeclared",
        "no such",
        "denied",
        "timed out",
        "timeout",
        "exit code",
        "exit status",
        "test result",
        "segmentation fault",
        "core dumped",
    ];
    if MARKERS.iter().any(|m| lower.contains(m)) {
        return true;
    }

    // Test-runner summaries and per-test verdicts.
    if is_test_signal(&lower) {
        return true;
    }

    // Stack frames: gdb `#0 …`, JS/Java `    at …`, python `  File "…", line N`.
    if trimmed.starts_with("at ")
        || (trimmed.starts_with('#') && trimmed[1..].starts_with(|c: char| c.is_ascii_digit()))
        || (lower.starts_with("file \"") && lower.contains("line "))
    {
        return true;
    }

    // Auth / device-code prompts the user must act on.
    if lower.contains("device code")
        || lower.contains("verification")
        || lower.contains("authenticate")
        || lower.contains("sign in")
    {
        return true;
    }

    // A file:line(:col) location reference anywhere in the line.
    contains_file_location(trimmed)
}

/// Test-runner signal: verdict tokens and "N passed / M failed"-style summaries.
fn is_test_signal(lower: &str) -> bool {
    const TOKENS: &[&str] = &[
        "test result",
        "tests run",
        "0 passed",
        "assertion failed",
        " passed",
        " failed",
        "passed;",
        "failed;",
        "passed,",
        "failed,",
    ];
    if TOKENS.iter().any(|t| lower.contains(t)) {
        return true;
    }
    (lower.contains("running") && lower.contains("test"))
        || (lower.contains("passed") && lower.contains("failed"))
}

/// Detects a `path:line` or `path:line:col` reference (a token with a `.`/`/` before a `:number`).
fn contains_file_location(line: &str) -> bool {
    for token in line.split(|c: char| c.is_whitespace() || c == '(' || c == ')') {
        let bytes = token.as_bytes();
        let Some(first_colon) = token.find(':') else {
            continue;
        };
        // Path-ish prefix (has a separator or extension dot) before the colon.
        let prefix = &token[..first_colon];
        if prefix.is_empty()
            || !(prefix.contains('/') || prefix.contains('.') || prefix.contains('\\'))
        {
            continue;
        }
        // Digits immediately after the colon → a line number.
        let after = &bytes[first_colon + 1..];
        if after.first().is_some_and(u8::is_ascii_digit) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_errors_and_warnings() {
        assert!(is_important("error[E0308]: mismatched types"));
        assert!(is_important("warning: unused variable `x`"));
        assert!(is_important("thread 'main' panicked at src/main.rs:5"));
        assert!(is_important("FAILED: 3 tests failed"));
    }

    #[test]
    fn keeps_file_locations_and_frames() {
        assert!(is_important("   --> src/lib.rs:12:9"));
        assert!(is_important("    at Object.<anonymous> (/app/index.js:4:10)"));
        assert!(is_important("  File \"app.py\", line 7, in <module>"));
        assert!(is_important("#0  main::run (main.rs:4)"));
    }

    #[test]
    fn keeps_test_summaries_and_auth() {
        assert!(is_important("test result: FAILED. 1 passed; 2 failed"));
        assert!(is_important("running 12 tests"));
        assert!(is_important("Enter the device code ABCD-1234 to authenticate"));
    }

    #[test]
    fn drops_plain_noise() {
        assert!(!is_important("   Compiling serde v1.0.0"));
        assert!(!is_important("Downloading crates ..."));
        assert!(!is_important("hello world"));
        assert!(!is_important(""));
    }
}
