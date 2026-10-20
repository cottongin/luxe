//! `luxe join` — a session another luxe is running, over stdin and stdout.
//!
//! The running luxe shares its session on a local endpoint (`cli::share`), and this command is the
//! plain way in: it picks the session from the records in the runtime folder, connects, and copies
//! bytes both ways. What crosses is the `--rpc` protocol untouched, so a client written for
//! `luxe --rpc` drives a joined session by launching `luxe join` instead.

use std::path::Path;

use anyhow::Context;
use luxe_core::local_endpoint::{self, LiveSession};
use tokio::io::AsyncWriteExt;

use super::args::JoinArgs;

/// List the joinable sessions, or join one and pass the protocol through until either side ends.
pub(crate) async fn run_join_command(args: JoinArgs, work_root: &Path) -> anyhow::Result<()> {
    let running = local_endpoint::running_sessions(&luxe_core::user_dirs::luxe_runtime_dir());
    if args.list {
        print!("{}", listing(&running));
        return Ok(());
    }
    let cwd = work_root.to_string_lossy();
    let session = pick(&running, args.target.as_deref(), &cwd)?;
    let stream = local_endpoint::connect(&session.address)
        .await
        .with_context(|| format!("could not join session {}", session.session_id))?;
    pass_through(stream).await
}

/// The session `target` names, or with no target the only one running in `cwd`.
fn pick<'a>(
    running: &'a [LiveSession],
    target: Option<&str>,
    cwd: &str,
) -> anyhow::Result<&'a LiveSession> {
    let matching: Vec<&LiveSession> = match target {
        Some(target) => running.iter().filter(|session| names(session, target)).collect(),
        None => running.iter().filter(|session| session.cwd == cwd).collect(),
    };
    match matching.as_slice() {
        [only] => Ok(only),
        [] => anyhow::bail!("{}", nothing_to_join(running, target, cwd)),
        several => anyhow::bail!(
            "{} sessions match; name one by id or process:\n{}",
            several.len(),
            listing_of(several.iter().copied())
        ),
    }
}

/// Whether `target` is the session's process id or the start of its id.
fn names(session: &LiveSession, target: &str) -> bool {
    let target = target.trim();
    !target.is_empty()
        && (session.session_id.starts_with(target) || target == session.pid.to_string())
}

fn nothing_to_join(running: &[LiveSession], target: Option<&str>, cwd: &str) -> String {
    let wanted = match target {
        Some(target) => format!("no running session matches `{target}`"),
        None => format!("no session is running in {cwd}"),
    };
    if running.is_empty() {
        return format!(
            "{wanted}, and none is running on this machine that can be joined. A luxe shares \
             its session unless `joinable` is off or LUXE_JOINABLE=0."
        );
    }
    format!("{wanted}. These can be joined:\n{}", listing(running))
}

fn listing(running: &[LiveSession]) -> String {
    listing_of(running.iter())
}

fn listing_of<'a>(sessions: impl Iterator<Item = &'a LiveSession>) -> String {
    sessions
        .map(|session| format!("{}  pid {}  {}\n", session.session_id, session.pid, session.cwd))
        .collect()
}

/// Copy stdin to the session and the session to stdout. Ends when the session closes the
/// connection, which it also does once it has read the end of our stdin.
async fn pass_through(stream: local_endpoint::LocalStream) -> anyhow::Result<()> {
    let (mut from_session, mut to_session) = tokio::io::split(stream);
    let mut input = read_stdin();
    let upload = tokio::spawn(async move {
        while let Some(chunk) = input.recv().await {
            if to_session.write_all(&chunk).await.is_err() {
                break;
            }
        }
        let _ = to_session.shutdown().await;
    });
    let mut stdout = tokio::io::stdout();
    let downloaded = tokio::io::copy(&mut from_session, &mut stdout).await;
    upload.abort();
    downloaded.context("the joined session's output stopped")?;
    stdout.flush().await.context("could not write to stdout")?;
    Ok(())
}

/// Stdin, read on a thread of its own. Tokio's stdin reads on its blocking pool, and the runtime
/// waits for that read on the way out, so a session that closes first would leave `luxe join`
/// hanging until the next keystroke. A plain thread is simply abandoned at exit.
fn read_stdin() -> tokio::sync::mpsc::Receiver<Vec<u8>> {
    let (chunks, received) = tokio::sync::mpsc::channel(16);
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buffer = vec![0; 8192];
        loop {
            let read = match std::io::Read::read(&mut stdin, &mut buffer) {
                Ok(0) => return,
                Ok(read) => read,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return tracing::warn!(%error, "stopped reading stdin"),
            };
            if chunks.blocking_send(buffer[..read].to_vec()).is_err() {
                return;
            }
        }
    });
    received
}

#[cfg(test)]
mod tests;
