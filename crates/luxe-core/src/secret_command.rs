//! Secret commands — a `!command` that prints a key or token, run bounded and off the runtime.
//!
//! One runner for every crate that resolves a secret written as `!pass show openai`: provider keys
//! and MCP client secrets and bearers. It runs from the home directory, so a project cannot put a
//! script of the same name in its way; it stops at a deadline, since a password manager that wants
//! a passphrase and has no terminal would otherwise hang the turn; and its errors carry stderr,
//! never what the command printed, since a command that fails halfway may still have printed the
//! secret.

use std::time::Duration;

/// How long a secret command may take. Long enough for a password manager to put up its unlock
/// prompt and be answered; short enough that a refresh inside a tool call, which has 60 seconds in
/// all, still leaves the call time to run.
pub const SECRET_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Runs a secret command from the home directory, so a project cannot put a script of the same
/// name in its way, and reads the secret from its first line.
pub async fn run(command: &str, what: &str, deadline: Duration) -> Result<String, String> {
    use crate::program::{captured_command, run_bounded, shell, BoundedRunError};

    let (program, flag) = shell();
    let home = crate::user_dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    let output =
        match run_bounded(captured_command(program, &[flag, command], &home), deadline).await {
            Ok(output) => output,
            Err(BoundedRunError::Timeout { after }) => {
                return Err(format!(
                    "the command for {what} did not finish in {}s — if it asks for a passphrase, \
                 it needs a graphical prompt: it has no terminal to ask on",
                    after.as_secs()
                ))
            }
            Err(error) => return Err(format!("the command for {what} could not run: {error}")),
        };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("");
        let reason: String = reason.trim().chars().take(200).collect();
        return Err(format!(
            "the command for {what} failed ({}){}",
            output.status,
            if reason.is_empty() { String::new() } else { format!(": {reason}") }
        ));
    }
    // `pass show` prints the secret and then whatever else the entry holds, one per line
    let stdout = String::from_utf8_lossy(&output.stdout);
    match stdout.lines().next().map(str::trim).filter(|line| !line.is_empty()) {
        Some(secret) => Ok(secret.to_owned()),
        None => Err(format!("the command for {what} printed nothing")),
    }
}

/// [`run`] for a caller with no runtime to await on, such as startup settings.
///
/// The command still runs under the same deadline, on a thread of its own with its own runtime, so
/// it works whether or not the caller is inside one.
pub fn run_blocking(command: &str, what: &str, deadline: Duration) -> Result<String, String> {
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| format!("the command for {what} could not run: {error}"))?;
                runtime.block_on(run(command, what, deadline))
            })
            .join()
            .unwrap_or_else(|_| Err(format!("the command for {what} could not run")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHAT: &str = "the client secret for 'asana'";

    /// A command that never finishes is stopped at the deadline, and the error names the likely
    /// reason: a passphrase prompt with no terminal to draw on.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_that_hangs_is_stopped_at_its_deadline() {
        let started = std::time::Instant::now();
        let hung = run("sleep 30", WHAT, Duration::from_millis(300)).await.unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(5), "bounded by the deadline");
        assert!(hung.contains("did not finish") && hung.contains("graphical prompt"), "{hung}");
    }

    /// The blocking form keeps the deadline, inside a runtime or out of one.
    #[cfg(unix)]
    #[test]
    fn the_blocking_form_is_bounded_too() {
        let started = std::time::Instant::now();
        let hung = run_blocking("sleep 30", WHAT, Duration::from_millis(300)).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(5), "bounded by the deadline");
        assert!(hung.contains("did not finish"), "{hung}");
    }

    /// The blocking form answers like the async one, on every platform's shell.
    #[test]
    fn the_blocking_form_reads_the_first_line() {
        let secret = run_blocking("echo s3cret-value", WHAT, SECRET_COMMAND_TIMEOUT).unwrap();
        assert_eq!(secret, "s3cret-value");
    }
}
