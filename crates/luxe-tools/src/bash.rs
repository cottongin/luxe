//! `bash` tool — execute shell commands with timeout and output truncation.
//!
//! Supports:
//! - Executing shell commands via tokio::process
//! - Timeout enforcement (kills child on timeout)
//! - Output truncation (keeps head+tail, notes truncation)
//! - Exit code reporting
//!
//! The call ends when the **command** does. A process it deliberately left running is not the
//! command, however long it holds the output pipe. Running, capturing and reaping are
//! `crate::process`'s, shared with `test`.
//!
//! A command that printed files — `cat`, `head`, `tail`, `nl`, `sed -n` — counts as reading them
//! for the read-before-edit guard; see `seen`.

pub(crate) mod seen;

use async_trait::async_trait;
use luxe_core::compression::{
    compress, CommandInvocation, CommandOutput, CompressionConfig, ReducerRegistry, TestReducer,
};
use luxe_core::{ContentBlock, Tool, ToolContext, ToolResultData, ToolSchema};
use serde_json::json;
use std::time::Duration;
use tracing::{debug, warn};

/// Default timeout for bash commands (30 seconds).
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Ceiling on the model-supplied `timeout`, in seconds (one hour).
///
/// The argument exists because a build or a suite legitimately needs longer than the default, and
/// the model is the one who knows which. It is clamped because it is also the one number a model can
/// set that removes its own safety net — `timeout: 99999999` is not a considered choice about a
/// build, and there is no repo task on the other side of an hour that a fresh call cannot resume.
const MAX_TIMEOUT_SECS: u64 = 3_600;

/// Final safety clamp on the model-facing body, guarding the pathological single huge line the
/// line-based compressor cannot shrink (a minified bundle, a one-line JSON response).
///
/// The compressor handles *volume* beautifully — 200,000 lines of `seq` come back as ~550
/// characters — but it works per line, so one 500 KB line passes straight through, and a 1 MB
/// ceiling is no guard at any real context window (roughly 250,000 tokens).
/// Clamping hard is safe precisely because oversized output is *also* teed to `.luxe/artifacts/`
/// with its path in the result: nothing is lost, only deferred. The same ceiling `read` keeps, so
/// one long line costs the same whichever tool printed it.
const MAX_OUTPUT_BYTES: usize = luxe_core::output_limit::RESULT_MAX_CHARS;

/// Resolves the compression config, honoring the `LUXE_COMPRESS=0` opt-out.
fn compression_config() -> CompressionConfig {
    let disabled = std::env::var("LUXE_COMPRESS").map(|v| v == "0").unwrap_or(false);
    CompressionConfig { enabled: !disabled, ..CompressionConfig::default() }
}

/// Command-specific reducers applied to bash output. The structured test reducer turns test-runner
/// noise into a pass/fail summary; unmatched commands take the generic pipeline.
fn reducer_registry() -> ReducerRegistry {
    let mut registry = ReducerRegistry::new();
    registry.register(Box::new(TestReducer));
    registry
}

/// Head+tail byte clamp — the last-resort guard when compression can't shrink a huge single line.
///
/// Cut on char boundaries: a byte offset can land inside a multi-byte character, and slicing there
/// panicked on any long enough line of non-ASCII output.
fn clamp_bytes(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let head = &text[..crate::output_limit::floor_boundary(text, max / 2)];
    let tail = &text[crate::output_limit::ceil_boundary(text, text.len() - max / 2)..];
    format!("{head}\n-- clamped {} bytes --\n{tail}", text.len() - head.len() - tail.len())
}

/// JSON-schema for the bash tool arguments.
fn bash_schema() -> ToolSchema {
    ToolSchema {
        name: "bash".to_string(),
        description: "Execute a shell command with a timeout, for what the other tools do not \
                      cover: builds, scripts, package managers, running the program.\n\
                      Do not use it for what a tool here does better, because the tool's output \
                      is bounded and structured, the person watching sees what you did, and edits \
                      are checked and formatted:\n\
                      - reading files (cat, head, tail, sed -n): `read`, which takes several \
                      paths in one call\n\
                      - listing and finding files (ls, find): `glob`; searching inside them \
                      (grep, rg): `grep`\n\
                      - changing files (heredocs, sed -i, echo >): `edit` and `write`\n\
                      - git and GitHub (git, gh): `git`, including issues and pull requests\n\
                      - running the tests: `test`\n\
                      Calls in one reply run in a single round trip, so several tool calls cost no \
                      more than one shell line doing the same.\n\
                      Each call gets a fresh shell already rooted at the project directory, so \
                      there is nothing to `cd` into first and a `cd` does not carry to the next \
                      call. Noisy output is deterministically compressed before it reaches you — \
                      ANSI/progress/duplicate lines are stripped and long logs are truncated while \
                      errors, warnings, failing tests, and file:line locations are always kept. \
                      When output is reduced, the untouched output is saved to .luxe/artifacts/ \
                      (each stream whole up to 8 MB, or its first and last 4 MB past that) and its path is \
                      included, so you can read it if you need the detail."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                // First, so the model writes it before the command and the card is named by it
                // from the start rather than renamed when the call lands
                "description": {
                    "type": "string",
                    "description": "What the command does, in 5–10 plain words, for the person \
                                    watching: \"Run the forms tests\", not the command's own text. \
                                    Their card shows this in place of the command."
                },
                "command": {
                    "type": "string",
                    "description": "Shell command to execute, from the project directory"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (optional, defaults to 30)"
                }
            },
            "required": ["command"]
        }),
    }
}

/// The `bash` tool implementation.
pub struct BashTool {
    /// Shared read tracker; a file a command printed is recorded as read
    tracker: Option<std::sync::Arc<luxe_core::read_tracker::ReadTracker>>,
}

impl BashTool {
    /// Create a new bash tool instance.
    pub fn new() -> Self {
        Self { tracker: None }
    }

    /// Attach the shared read tracker, so a `cat` of a file satisfies the edit guards.
    pub fn with_tracker(
        mut self,
        tracker: std::sync::Arc<luxe_core::read_tracker::ReadTracker>,
    ) -> Self {
        self.tracker = Some(tracker);
        self
    }
}

impl Default for BashTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Most surviving processes a timeout names before the rest become a count.
const MAX_SURVIVORS_LISTED: usize = 10;

/// Longest command line a timeout shows for one surviving process.
const SURVIVOR_COMMAND_CHARS: usize = 120;

/// What follows a timeout's output: the processes still running, and what to do next.
///
/// A suspended process changes the advice: it was never going to finish, so a longer `timeout`
/// would only wait longer. Commands start without a terminal, so a spinner or a prompt cannot cause
/// it; a `SIGSTOP` or a debugger still can.
fn timeout_advice(survivors: &[luxe_core::program::GroupMember]) -> String {
    let mut out = String::new();
    if !survivors.is_empty() {
        out.push_str("\n\nStill running when it was stopped (pid, state, command):");
        for member in survivors.iter().take(MAX_SURVIVORS_LISTED) {
            let command: String = member.command.chars().take(SURVIVOR_COMMAND_CHARS).collect();
            let cut =
                if member.command.chars().count() > SURVIVOR_COMMAND_CHARS { "…" } else { "" };
            out.push_str(&format!("\n  {} {} {command}{cut}", member.pid, member.state));
        }
        if survivors.len() > MAX_SURVIVORS_LISTED {
            out.push_str(&format!("\n  … +{} more", survivors.len() - MAX_SURVIVORS_LISTED));
        }
    }
    if survivors.iter().any(luxe_core::program::GroupMember::is_stopped) {
        out.push_str(
            "\n\nState T means suspended, not slow: something stopped it (a `SIGSTOP`, a job-control \
             signal, a debugger) and it will not continue on its own. More time will not help; find \
             what stops it, or ask the user to run it in their own terminal.",
        );
    } else {
        out.push_str(&format!(
            "\n\nFor a command that needs longer, pass `timeout` (up to {MAX_TIMEOUT_SECS} \
             seconds); to leave one running, start it in the background with `&` and its output \
             redirected to a file."
        ));
    }
    out
}

#[async_trait]
impl Tool for BashTool {
    /// No engine-side deadline: this tool spawns its command as its own **process group** and kills
    /// the whole tree itself, which the engine cannot do (dropping the future reaps the direct
    /// child and orphans everything it started). It owns the bound because it owns the kill — the
    /// `timeout` argument, defaulted and clamped in [`Self::run`], and `crate::process`'s tree
    /// guard for every other way the call can end.
    fn timeout(&self) -> Option<Duration> {
        None
    }

    fn name(&self) -> &str {
        "bash"
    }

    fn schema(&self) -> ToolSchema {
        bash_schema()
    }

    async fn run(&self, args: serde_json::Value, cx: &ToolContext) -> ToolResultData {
        let Some(command) = args.get("command").and_then(|v| v.as_str()) else {
            return ToolResultData::error("Error: 'command' argument is required");
        };
        let timeout_secs = timeout_secs(&args);
        debug!(command, timeout = timeout_secs, "executing bash command");
        let timeout = Duration::from_secs(timeout_secs);
        match crate::process::run_captured(command, &cx.cwd, timeout).await {
            Ok(crate::process::Outcome::Finished(output)) => self.finish(command, cx, output),
            Ok(crate::process::Outcome::TimedOut(stopped)) => {
                warn!(command, timeout = timeout_secs, "command timed out");
                timed_out(timeout_secs, &stopped)
            }
            Err(error) => {
                warn!(command, %error, "failed to run command");
                ToolResultData::error(format!("Error running command: {error}"))
            }
        }
    }
}

/// The model's `timeout`, defaulted and clamped.
///
/// A negative or absent value takes the default; anything past the ceiling is clamped rather than
/// refused, since the intent ("this one is slow") is still worth honoring.
fn timeout_secs(args: &serde_json::Value) -> u64 {
    args.get("timeout")
        .and_then(|v| v.as_i64())
        .and_then(|secs| u64::try_from(secs).ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .min(MAX_TIMEOUT_SECS)
}

/// A timeout's reply: what the command had printed, which is the evidence of where it got to, and
/// the way out. A bare "timed out" left the agent to guess both, and usually to run it again.
fn timed_out(timeout_secs: u64, stopped: &crate::process::TimedOut) -> ToolResultData {
    let printed = match stopped.printed.is_empty() {
        true => "It printed nothing before it was stopped.".to_string(),
        false => format!("Output before it was stopped:\n{}", stopped.printed),
    };
    ToolResultData::error(format!(
        "Command timed out after {timeout_secs} seconds and was stopped. {printed}{}",
        timeout_advice(&stopped.survivors)
    ))
}

impl BashTool {
    /// The reply for a command that ran to its end.
    fn finish(
        &self,
        command: &str,
        cx: &ToolContext,
        output: crate::process::Captured,
    ) -> ToolResultData {
        let crate::process::Captured { stdout, stderr, status, detached_writer } = output;
        // A failed command may have printed half of what it was asked to, so only a clean exit counts
        if let Some(tracker) = self.tracker.as_ref().filter(|_| status.success()) {
            for path in seen::files_shown(command, &cx.cwd, &stdout) {
                tracker.record(&cx.session_id, &path);
            }
        }
        let exit = status.code();
        if detached_writer {
            debug!(command, "a process outlived the shell still holding its pipe");
        }
        let mut body = compressed_body(command, cx, (&stdout, &stderr, exit));
        if detached_writer {
            body.push_str(crate::process::DETACHED_WRITER_NOTE);
        }
        // A failure on a Windows box with no POSIX shell is usually about the *shell*, not the
        // command. Said once, on failure only, so `cargo test` through `cmd` stays quiet.
        if !crate::process::posix_shell() && !status.success() {
            body.push_str(NO_POSIX_SHELL_NOTE);
        }
        // `grep` and `rg` exit 1 for "nothing matched", which is an answer rather than a failure,
        // when the search ends the command and said nothing at all
        let no_match = exit == Some(1)
            && stdout.trim().is_empty()
            && stderr.trim().is_empty()
            && ends_in_search(command);
        let exit_note = if no_match {
            " — the search matched nothing, which is what exit code 1 means for it"
        } else {
            ""
        };
        ToolResultData {
            content: vec![ContentBlock::Text(format!(
                "{body}\n\nExit code: {}{exit_note}",
                exit.unwrap_or(-1)
            ))],
            is_error: !status.success() && !no_match,
        }
    }
}

/// The output as the model reads it: compressed, led by a test run's regressions, pointed at the
/// untouched copy whenever anything was dropped, and clamped last.
fn compressed_body(
    command: &str,
    cx: &ToolContext,
    (stdout, stderr, exit): (&str, &str, Option<i32>),
) -> String {
    // Noisy output keeps its signal — errors, warnings, failing tests, file:line, exit codes — and
    // the test reducer turns `cargo test`/`pytest` output into a pass/fail summary.
    let invocation = CommandInvocation::parse(command);
    let captured =
        CommandOutput { stdout: stdout.to_owned(), stderr: stderr.to_owned(), exit_code: exit };
    let result = compress(&invocation, &captured, &compression_config(), &reducer_registry());
    let mut body =
        if result.output.is_empty() { "(no output)".to_string() } else { result.output.clone() };
    if let Some(note) = test_delta(command, cx, &invocation, stdout, stderr) {
        body = format!("{note}{body}");
    }
    // A body too long to send whole is a reduction too, so its untouched output is saved the same
    // way, or the clamp below would drop text with no path back to it
    if result.was_reduced() || body.len() > MAX_OUTPUT_BYTES {
        // The schema promises the untouched output is recoverable whenever this reduces, so a tee
        // that could not write has to say so rather than leave the reader looking for a path
        match crate::process::tee_artifact(&cx.cwd, "cmd", command, (stdout, stderr, exit)) {
            Some(path) => {
                body.push_str(&format!("\n\nFull output: {}", luxe_core::paths::display(&path)))
            }
            None => body.push_str(
                "\n\nThe full output could not be saved to .luxe/artifacts/ — re-run with a \
                 narrower command if you need the part that was dropped.",
            ),
        }
        if let Some(footer) = result.footer().filter(|_| result.was_reduced()) {
            body.push('\n');
            body.push_str(&footer);
        }
    }
    clamp_bytes(&body, MAX_OUTPUT_BYTES)
}

/// A suite run through the shell gets the `test` tool's "what broke since last time" too. Keyed by
/// the whole command, so each subset a model runs is compared only with itself.
fn test_delta(
    command: &str,
    cx: &ToolContext,
    invocation: &CommandInvocation,
    stdout: &str,
    stderr: &str,
) -> Option<String> {
    if !luxe_core::compression::is_test_command(invocation) {
        return None;
    }
    let summary = luxe_core::compression::parse_test_output(stdout, stderr)?;
    let failing = summary.failures.iter().map(|f| f.name.clone()).collect();
    crate::test_baseline::diff_and_record(
        &cx.cwd,
        command,
        &failing,
        summary.passed,
        summary.failed,
        false,
    )
    .render()
}

/// `cmd`'s complaint about a pipe or a quote reads like the command was wrong.
const NO_POSIX_SHELL_NOTE: &str = "\n\nNote: no POSIX shell was found, so this ran through \
     cmd.exe, which does not understand `|`, `&&`, single quotes, `$VAR` or unix tool names. \
     Install Git for Windows (its `bash` is picked up automatically) to run commands as written.";

/// Whether the last command `command` runs is a search whose exit 1 means "no match".
///
/// The last one, because that is whose status the shell reports: `npm test | grep x` exits as
/// `grep` did. Quotes are respected — a pattern like `"^ok|^not ok"` is full of pipes — and anything
/// unclear after the search, like a redirection's `2>&1`, makes this false, which keeps the call
/// reported exactly as it was.
fn ends_in_search(command: &str) -> bool {
    let bytes = command.as_bytes();
    let (mut single, mut double, mut start) = (false, false, 0);
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' if !double => single = !single,
            b'"' if !single => double = !double,
            b'\\' if !single => index += 1,
            b'|' | b';' | b'&' | b'\n' if !single && !double => start = index + 1,
            _ => {}
        }
        index += 1;
    }
    let Some(words) = shlex::split(&command[start..]) else { return false };
    let program = words
        .iter()
        .find(|word| !word.contains('='))
        .map(|word| word.rsplit('/').next().unwrap_or(word));
    matches!(program, Some("grep" | "egrep" | "fgrep" | "rg"))
}

#[cfg(test)]
mod tests;

/// A cancelled command has to stop, and so does everything it started.
///
/// The engine cancels a tool by dropping its future, and `kill_on_drop` reaps only the shell — so
/// this is the one property no amount of testing the timeout path establishes. Both endings are
/// here, because they are the same obligation reached two different ways.
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tree_teardown {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::{mpsc, watch};

    fn context(cwd: &std::path::Path) -> ToolContext {
        let (event_tx, _event_rx) = mpsc::channel(64);
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        ToolContext {
            cwd: cwd.to_path_buf(),
            session_id: "tree-teardown".to_string(),
            cancelled: Arc::new(cancel_rx),
            event_tx,
            hook_ctx: None,
            question_broker: None,
        }
    }

    /// A shell that backgrounds a grandchild and waits on it. Killing the shell alone leaves the
    /// grandchild touching `marker` — which is how a survivor makes itself known.
    fn script(marker: &std::path::Path) -> String {
        format!("(for _ in $(seq 1 40); do touch {}; sleep 0.25; done) & wait", marker.display())
    }

    /// Whether anything is still touching `marker` after it is removed.
    async fn survivor_still_running(marker: &std::path::Path) -> bool {
        let _ = std::fs::remove_file(marker);
        tokio::time::sleep(Duration::from_millis(1_200)).await;
        marker.exists()
    }

    #[tokio::test]
    async fn a_dropped_call_takes_the_whole_tree_with_it() {
        if !cfg!(unix) {
            return; // the script is a POSIX one
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("alive");
        let cx = context(dir.path());
        {
            let tool = BashTool::new();
            let run = tool.run(serde_json::json!({"command": script(&marker)}), &cx);
            tokio::pin!(run);
            // Poll far enough to spawn the tree, then drop it — exactly what a cancel does.
            let _ = tokio::time::timeout(Duration::from_millis(900), &mut run).await;
        }
        assert!(
            !survivor_still_running(&marker).await,
            "a cancelled command left its grandchild running"
        );
    }

    #[tokio::test]
    async fn a_timed_out_call_takes_the_whole_tree_with_it() {
        if !cfg!(unix) {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("alive");
        let cx = context(dir.path());
        let result = BashTool::new()
            .run(serde_json::json!({"command": script(&marker), "timeout": 1}), &cx)
            .await;
        assert!(result.is_error, "a command that outlives its timeout is an error");
        assert!(
            !survivor_still_running(&marker).await,
            "a timed-out command left its grandchild running"
        );
    }
}
