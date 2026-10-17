//! `test` tool — run the project's tests and return a structured pass/fail summary.
//!
//! Auto-detects the test command from project markers (Cargo.toml, go.mod, pyproject/pytest,
//! package.json) — preferring the environment the *project* pinned over whatever shares its name on
//! `PATH` — or takes an explicit `command`, runs it, and parses the output into a compact
//! `framework: verdict — N passed, M failed` summary with failing names + file:line (via the shared
//! [`luxe_core::compression`] test reducer), saving the untouched output under `.luxe/artifacts/`
//! as `bash` does. Unlike `bash "cargo test"`, this always renders the structured summary (even for
//! a small passing run) and figures out the command for you.

use async_trait::async_trait;
use luxe_core::compression::{
    compress, parse_test_output, CommandInvocation, CommandOutput, CompressionConfig,
    ReducerRegistry,
};
use luxe_core::{ContentBlock, Tool, ToolContext, ToolResultData, ToolSchema};
use serde_json::json;
use std::path::Path;
use std::time::Duration;

mod detect;
mod shell;

use detect::*;
use shell::{run_shell, ShellResult};

/// Default test timeout (tests can be slow: builds + suites).
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Ceiling on the model-supplied `timeout`, in seconds (one hour).
///
/// Higher than any real suite needs, and low enough that a runaway is still bounded. See
/// `bash`'s equivalent: the argument is honored because the model knows which run is slow, and
/// clamped because it is also the one number that can remove the safety net.
const MAX_TIMEOUT_SECS: u64 = 3_600;

/// The detected/overridden runner, so we can apply a filter with the right flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framework {
    Cargo,
    Go,
    Pytest,
    Node,
    Unknown,
}

#[derive(Default)]
pub(crate) struct TestTool;

impl TestTool {
    pub(crate) fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for TestTool {
    fn name(&self) -> &str {
        "test"
    }

    /// No engine-side deadline: like `bash`, this tool spawns its suite as its own process group and
    /// kills the tree itself. A test run is also the one call that legitimately outlasts every
    /// harness-wide bound — a cold `cargo test` on a large workspace is minutes of compilation
    /// before a single test runs.
    fn timeout(&self) -> Option<Duration> {
        None
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "test".to_string(),
            description: "Run the project's tests and return a structured summary: counts, each \
                          failing test with its file:line and message, and which tests were \
                          passing before your change and fail now. Auto-detects the command from project markers (Cargo.toml→cargo \
                          test/nextest, go.mod→go test, pyproject/pytest→pytest, package.json→the \
                          manifest's test script) and runs it in the environment the project \
                          pinned — its virtualenv, its package manager — rather than whatever \
                          shares the name on PATH; `command` overrides. Output is saved to \
                          .luxe/artifacts/ (each stream whole up to 8 MB, its first and \
                          last 4 MB past that). Prefer this over `bash` for running tests.\n\
                          Before calling work done, run every test that covers what you \
                          changed: the full suite when it is quick, and when it is not (a large \
                          project's can take many minutes) the modules around your change. \
                          `filter` is for iterating on one failure, and a filtered run proves \
                          nothing about the tests it skipped."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Full test command to run (overrides auto-detection), e.g. `cargo nextest run -p mycrate`"
                    },
                    "filter": {
                        "type": "string",
                        "description": "Only run tests matching this name/substring (applied with the framework's filter flag)"
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in seconds (default 300)"
                    }
                }
            }),
        }
    }

    async fn run(&self, args: serde_json::Value, cx: &ToolContext) -> ToolResultData {
        // A strict-schema model sends `filter: ""` for no filter, which read as a filtered run and
        // drew the warning that it proved nothing
        let filter = args.get("filter").and_then(|v| v.as_str()).filter(|f| !f.trim().is_empty());
        let (command, framework) = match resolve_command(&args, &cx.cwd) {
            Ok(resolved) => resolved,
            Err(message) => return ToolResultData::error(format!("Error: {}", message)),
        };
        let command = apply_filter(&command, framework, filter, crate::process::posix_shell());
        let timeout = Duration::from_secs(
            args.get("timeout")
                .and_then(|v| v.as_u64())
                .filter(|secs| *secs > 0)
                .unwrap_or(DEFAULT_TIMEOUT_SECS)
                .min(MAX_TIMEOUT_SECS),
        );
        let output = match run_shell(&command, &cx.cwd, timeout).await {
            Ok(output) => output,
            Err(message) => return ToolResultData::error(format!("Error: {}", message)),
        };
        let run = Run { command: &command, framework, filter, output: &output };
        let summary = summarize(&run, &cx.cwd);
        ToolResultData {
            content: vec![ContentBlock::Text(compose_reply(&run, &cx.cwd, summary))],
            is_error: !output.success,
        }
    }
}

/// The command to run: the caller's, or the one the project's markers name.
fn resolve_command(args: &serde_json::Value, cwd: &Path) -> Result<(String, Framework), String> {
    match args.get("command").and_then(|v| v.as_str()) {
        Some(explicit) => Ok((explicit.to_string(), classify_command(explicit))),
        None => detect_command(cwd).map(|detected| (detected.command, detected.framework)),
    }
}

/// One finished run, as the reply is written from it.
struct Run<'a> {
    command: &'a str,
    framework: Framework,
    filter: Option<&'a str>,
    output: &'a ShellResult,
}

/// What a run comes to: its rendered body, the regressions since the last run, and whether it ran
/// anything at all.
struct Summary {
    body: String,
    delta_note: Option<String>,
    ran_nothing: bool,
}

/// The structured summary for a recognized runner; otherwise the generic compression, so output
/// isn't lost.
fn summarize(run: &Run<'_>, cwd: &Path) -> Summary {
    let output = run.output;
    let Some(summary) = parse_test_output(&output.stdout, &output.stderr) else {
        let captured = CommandOutput {
            stdout: output.stdout.clone(),
            stderr: output.stderr.clone(),
            exit_code: output.exit_code,
        };
        let invocation = CommandInvocation::parse(run.command);
        let config = CompressionConfig::default();
        return Summary {
            body: compress(&invocation, &captured, &config, &ReducerRegistry::new()).output,
            delta_note: None,
            ran_nothing: executed_nothing(&output.stdout, &output.stderr),
        };
    };
    // A count answers "is it green?". After an edit the live question is "did I break something
    // that worked?", and only a diff against the last run answers that.
    let failing: std::collections::BTreeSet<String> =
        summary.failures.iter().map(|f| f.name.clone()).collect();
    let delta_note = crate::test_baseline::diff_and_record(
        cwd,
        run.command,
        &failing,
        summary.passed,
        summary.failed,
        run.filter.is_some(),
    )
    .render();
    Summary {
        ran_nothing: summary.passed == 0 && summary.failed == 0 && summary.ignored == 0,
        body: summary.render(),
        delta_note,
    }
}

/// The reply: the command, its regressions first, the capped body, and every note that applies.
fn compose_reply(run: &Run<'_>, cwd: &Path, summary: Summary) -> String {
    let output = run.output;
    // Teed before the cap, because the cap's hint has to name what actually happened: no artifact
    // on a read-only tree or a full disk
    let streams = (output.stdout.as_str(), output.stderr.as_str(), output.exit_code);
    let artifact = crate::process::tee_artifact(cwd, "test", run.command, streams);
    // A broken suite is the case that produces the most output: the first failures reveal the
    // pattern, the rest repeat it, and the untouched run is on disk
    let hint = match &artifact {
        Some(path) => format!("Fix these first. Saved run: {}", luxe_core::paths::display(path)),
        None => "Fix these first, then run again — or pass `filter` to narrow the run.".to_string(),
    };
    let body = crate::output_limit::cap_lines(&summary.body, TEST_MAX_LINES, "lines", &hint);
    // The regression note leads: it is the one part of the reply a reader must not scroll past.
    let command = run.command;
    let mut text = match &summary.delta_note {
        Some(note) => format!("$ {command}\n{note}{body}"),
        None => format!("$ {command}\n{body}"),
    };
    if summary.ran_nothing {
        text.push_str(&format!(
            "\n\nNo tests actually ran{} — that is not a green suite, whatever the exit code \
             says. Check the path or filter you passed.",
            run.filter.map(|f| format!(" (filter: `{f}`)")).unwrap_or_default()
        ));
    }
    if let Some(active) = run.filter {
        text.push_str(&format!(
            "\n\nThis run was filtered to `{active}` — it says nothing about the tests it \
             skipped. Run everything your change touches before calling the work done."
        ));
    }
    if let Some(note) = foreign_interpreter_note(cwd, command, run.framework, output) {
        text.push_str(&format!("\n\n{note}"));
    }
    if let Some(path) = &artifact {
        text.push_str(&format!("\n\nFull output: {}", luxe_core::paths::display(path)));
    }
    if output.detached_writer {
        text.push_str(crate::process::DETACHED_WRITER_NOTE);
    }
    text.push_str(&format!("\n\nExit code: {}", output.exit_code.unwrap_or(-1)));
    text
}

/// Whether a run selected no tests at all, for output the summary parsers don't recognize.
///
/// Every runner reports an empty selection quietly and exits 0 for it, so "1 deselected" reads as
/// success to anyone skimming the exit code — including a model that just filtered by a name it
/// guessed slightly wrong.
fn executed_nothing(stdout: &str, stderr: &str) -> bool {
    const EMPTY_RUN: [&str; 6] = [
        "no tests ran",
        "no tests collected",
        "collected 0 items",
        "no test files",
        "no tests found",
        "0 tests run",
    ];
    let haystack = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    if EMPTY_RUN.iter().any(|needle| haystack.contains(needle)) {
        return true;
    }
    // pytest's deselect line: "N deselected" with nothing selected alongside it.
    haystack.contains("deselected")
        && !haystack.contains(" passed")
        && !haystack.contains(" failed")
}

/// Most rendered lines a test run returns before the rest defers to the artifact file.
const TEST_MAX_LINES: usize = 120;

/// Whether a program can be run, injected so detection is decidable without the host's `PATH`.
type Available<'a> = &'a dyn Fn(&str) -> bool;

/// A note for a pytest run that died on an import under an interpreter that is not the project's.
///
/// Only reachable when the caller passed `command` itself — detection prefers the environment — and
/// that is exactly when it is needed, because the traceback names a missing plugin and says nothing
/// about the interpreter that was missing it.
fn foreign_interpreter_note(
    cwd: &Path,
    command: &str,
    framework: Framework,
    output: &ShellResult,
) -> Option<String> {
    if framework != Framework::Pytest || output.success {
        return None;
    }
    if !format!("{}\n{}", output.stdout, output.stderr).contains("No module named") {
        return None;
    }
    if ["uv run", "poetry run", "pipenv run", "pdm run", "hatch run", "tox"]
        .iter()
        .any(|runner| command.contains(runner))
    {
        return None;
    }
    let venv = luxe_core::program::project_venv(cwd)?;
    let python = luxe_core::program::venv_program(&venv, "python")?;
    let interpreter = shell_word(cwd, &python);
    if command.contains(&interpreter) {
        return None;
    }
    Some(format!(
        "That ran outside `{}`, which is where this project's dependencies are installed — a missing \
         module is the symptom. Retry with `command: \"{interpreter} -m pytest\"`.",
        shell_word(cwd, &venv)
    ))
}

/// Best-effort framework classification of an explicit command (for filter flag selection).
fn classify_command(command: &str) -> Framework {
    let lower = command.to_ascii_lowercase();
    if lower.contains("cargo") {
        Framework::Cargo
    } else if lower.contains("go test") {
        Framework::Go
    } else if lower.contains("pytest") {
        Framework::Pytest
    } else if ["jest", "vitest", "mocha", "npm", "pnpm", "yarn", "bun", "deno"]
        .iter()
        .any(|runner| lower.contains(runner))
    {
        Framework::Node
    } else {
        Framework::Unknown
    }
}

/// Append a name filter using the framework's flag, quoted as one argument for the shell that
/// runs it.
///
/// The filter is one value, and pytest's `-k "a or b"` is the ordinary case: pasted in raw, a space
/// split it and a `;` ran whatever followed.
fn apply_filter(command: &str, framework: Framework, filter: Option<&str>, posix: bool) -> String {
    let Some(filter) = filter.filter(|f| !f.is_empty()) else {
        return command.to_string();
    };
    let filter = crate::process::quote_argument(filter, posix);
    match framework {
        Framework::Cargo | Framework::Unknown => format!("{command} {filter}"),
        Framework::Go => format!("{command} -run {filter}"),
        Framework::Pytest => format!("{command} -k {filter}"),
        Framework::Node => format!("{command} -- {filter}"),
    }
}

#[cfg(test)]
mod tests;
