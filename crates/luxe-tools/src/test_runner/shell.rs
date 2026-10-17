//! Shell — how a test command runs, and what comes back.

use std::path::Path;
use std::time::Duration;

/// Captured shell result.
pub(super) struct ShellResult {
    pub(super) stdout: String,
    pub(super) stderr: String,
    pub(super) exit_code: Option<i32>,
    pub(super) success: bool,
    /// The shell exited while something it started still held the output pipe
    pub(super) detached_writer: bool,
}

/// Runs `command` through [`crate::process`], the runner `bash` uses: its own process group,
/// killed whole on a timeout or a cancel, output bounded.
pub(super) async fn run_shell(
    command: &str,
    cwd: &Path,
    timeout: Duration,
) -> Result<ShellResult, String> {
    match crate::process::run_captured(command, cwd, timeout).await {
        Ok(crate::process::Outcome::Finished(output)) => Ok(ShellResult {
            exit_code: output.status.code(),
            success: output.status.success(),
            stdout: output.stdout,
            stderr: output.stderr,
            detached_writer: output.detached_writer,
        }),
        Ok(crate::process::Outcome::TimedOut(_)) => {
            Err(format!("tests timed out after {}s", timeout.as_secs()))
        }
        Err(error) => Err(format!("could not run the test command: {error}")),
    }
}
