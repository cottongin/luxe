//! Bounded runs — a program held to a deadline, its whole tree killed when it passes.

/// Why a bounded run produced no output.
#[derive(Debug, thiserror::Error)]
pub enum BoundedRunError {
    /// The program could not be started.
    #[error("could not start the program")]
    Spawn(#[source] std::io::Error),
    /// The deadline passed; the whole process tree has been killed.
    #[error("timed out after {}s", after.as_secs())]
    Timeout {
        /// The deadline that expired
        after: std::time::Duration,
    },
    /// The program started but waiting on it failed.
    #[error("lost the program while waiting for it")]
    Wait(#[source] std::io::Error),
}

/// Run `command` to completion under `deadline`, killing its whole tree if it expires or the
/// caller stops waiting.
///
/// Meant for a command built by [`super::captured_command`]: the tree kill reaches descendants only because
/// the child leads its own process group. A caller owns the wording of its failures — the `git` tool
/// names the usual culprit for a timeout, GitHub turns one into a typed error — so this returns
/// the variant and leaves the sentence to them.
pub async fn run_bounded(
    command: tokio::process::Command,
    deadline: std::time::Duration,
) -> Result<std::process::Output, BoundedRunError> {
    run_bounded_with_input(command, &[], deadline).await
}

/// [`run_bounded`] with `input` written to the program's stdin, the write held to the same
/// deadline as the run.
///
/// Feeding a pipe is part of the run, not a step before it: a patch routinely outgrows the pipe's
/// buffer, and the write then waits for the program to drain it — forever, if the program has
/// stalled. A program that exits without reading all of `input` is not an error here; its status
/// says what happened. Empty `input` leaves stdin as `command` had it.
pub async fn run_bounded_with_input(
    mut command: tokio::process::Command,
    input: &[u8],
    deadline: std::time::Duration,
) -> Result<std::process::Output, BoundedRunError> {
    if !input.is_empty() {
        command.stdin(std::process::Stdio::piped());
    }
    let mut child = command.spawn().map_err(BoundedRunError::Spawn)?;
    let mut reaper = super::TreeGuard::arm(&child);
    let stdin = child.stdin.take().filter(|_| !input.is_empty());
    let feed = async {
        if let Some(mut stdin) = stdin {
            use tokio::io::AsyncWriteExt;
            // a broken pipe is the program having heard enough
            let _ = stdin.write_all(input).await;
            let _ = stdin.shutdown().await;
        }
    };
    let run = async { tokio::join!(feed, child.wait_with_output()).1 };
    match tokio::time::timeout(deadline, run).await {
        Ok(result) => {
            reaper.disarm();
            result.map_err(BoundedRunError::Wait)
        }
        // the reaper takes the tree on the way out
        Err(_) => Err(BoundedRunError::Timeout { after: deadline }),
    }
}
