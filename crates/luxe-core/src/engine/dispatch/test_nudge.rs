//! Test nudge — pointing a failing test run at the debugger.

use super::*;

impl Engine {
    /// Nudge toward the `debug` tool when tests keep failing — runtime reinforcement for models
    /// that default to print-and-retry (the debugger is a luxe differentiator, but under-used).
    /// Fires once per failing streak (≥3 consecutive failing test runs); a passing test resets it.
    /// Disable with `LUXE_DEBUG_NUDGE=0`.
    pub(super) async fn maybe_nudge_debugger(&self, is_test: bool, result: &mut ToolResultData) {
        const THRESHOLD: u32 = 3;
        if !is_test || std::env::var("LUXE_DEBUG_NUDGE").as_deref() == Ok("0") {
            return;
        }
        let mut state = self.test_failure_state.lock().await;
        if !result.is_error {
            *state = (0, false);
            return;
        }
        state.0 += 1;
        if state.0 >= THRESHOLD && !state.1 {
            state.1 = true;
            result.content.push(ContentBlock::Text(format!(
                "\n[hint] Tests have failed {} times in a row. For a complex or stateful bug, \
                 drive the `debug` tool — set a breakpoint at the failure and inspect real state \
                 instead of another edit-and-retry.",
                state.0
            )));
        }
    }
}

/// Test-runner substrings used to recognize a `bash`-driven test invocation for the debugger nudge.
const TEST_RUNNERS: &[&str] = &[
    "cargo test",
    "cargo nextest",
    "pytest",
    "go test",
    "npm test",
    "npm run test",
    "pnpm test",
    "yarn test",
    "jest",
    "vitest",
    "mvn test",
    "gradle test",
    "ctest",
    "rspec",
    "phpunit",
    "dotnet test",
];

/// Whether a tool call is a test run: the dedicated `test` tool, or a `bash` command whose text
/// invokes a known test runner. Kept conservative (requires a recognizable runner) so the nudge
/// streak isn't tripped by unrelated commands.
pub(super) fn is_test_command(tool: &str, args: &serde_json::Value) -> bool {
    if tool == "test" {
        return true;
    }
    if tool == "bash" {
        if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
            let lowered = cmd.to_lowercase();
            return TEST_RUNNERS.iter().any(|runner| lowered.contains(runner));
        }
    }
    false
}
