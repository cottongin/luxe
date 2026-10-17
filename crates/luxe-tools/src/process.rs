//! Process — the one way `bash` and `test` run a shell command: bounded, captured, and reaped.
//!
//! The command gets a session of its own, so a timeout or a cancel can kill everything it started
//! (`kill_on_drop` reaches only the shell). Each output stream keeps its first [`HEAD_BYTES`] and
//! its last [`TAIL_BYTES`] and counts what fell between, so a command that prints without end
//! cannot grow memory without end either. The call ends when the shell does, not when the last
//! process holding its pipe lets go — see [`wait_for_output`].

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::process::Command;

/// How much of the start of each stream a capture keeps.
pub(crate) const HEAD_BYTES: usize = 4 * 1024 * 1024;

/// How much of the end of each stream a capture keeps.
pub(crate) const TAIL_BYTES: usize = 4 * 1024 * 1024;

/// How long output is still collected after the shell itself has exited.
///
/// Anything still buffered lands within microseconds of the shell's exit, since every writer is
/// gone; this window is slack, not a wait.
const DRAIN_GRACE: Duration = Duration::from_millis(250);

/// How much of a timed-out command's output its result keeps: the tail, where it stopped.
const TIMEOUT_TAIL_BYTES: usize = 4_000;

/// The call ended with the shell, and something it started still had the pipe. Worth a line,
/// because the reader is about to compare this output against what a terminal would show, and the
/// fix is a detail of shell syntax rather than anything about their command.
pub(crate) const DETACHED_WRITER_NOTE: &str =
    "\n\nNote: a process this command left running still held its \
     output, so anything written after the shell exited is not here. An `&` on a `&&` list \
     backgrounds the whole list, which keeps that pipe open; start a long-lived process as \
     `( cmd >/tmp/out.log 2>&1 & )` to detach it properly.";

/// How a run ended.
pub(crate) enum Outcome {
    /// The shell exited, with what it printed
    Finished(Captured),
    /// The deadline passed. The tree was killed as `run_captured` returned, after the survivors in
    /// it were listed.
    TimedOut(TimedOut),
}

/// A command stopped at its deadline: what it had printed, and what was still running.
pub(crate) struct TimedOut {
    /// The end of what it printed, already cut to [`TIMEOUT_TAIL_BYTES`]
    pub(crate) printed: String,
    /// The processes still in its group when it was stopped
    pub(crate) survivors: Vec<luxe_core::program::GroupMember>,
}

/// What one command produced, and whether it outlived its own output.
pub(crate) struct Captured {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) status: std::process::ExitStatus,
    /// The shell exited while a process it started still held the output pipe.
    pub(crate) detached_writer: bool,
}

/// The shell command line `command` runs as, in `cwd`, with no terminal and a group of its own.
fn shell_command(command: &str, cwd: &Path) -> Command {
    let (shell, flag) = luxe_core::program::shell();
    let mut child = Command::new(shell);
    child
        .arg(flag)
        .arg(command)
        .current_dir(cwd)
        .kill_on_drop(true)
        // Never inherit the terminal: a command that reads stdin would eat the user's keystrokes,
        // and one that writes to the inherited fds would paint over the TUI.
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    luxe_core::program::detach_from_terminal(&mut child);
    child
}

/// Whether commands here run through a POSIX shell, which decides how an argument is quoted.
pub(crate) fn posix_shell() -> bool {
    luxe_core::program::shell().1 == "-c"
}

/// `value` as one shell word: POSIX quoting, or `cmd.exe`'s double quotes with inner ones doubled.
/// A plain name stays bare either way.
///
/// `cmd.exe` still expands `%VAR%` inside double quotes, so there a `%` can change the value; it
/// cannot end the word or start a command, since `&` and `|` stay quoted.
pub(crate) fn quote_argument(value: &str, posix: bool) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "_-.:/".contains(c);
    if value.chars().all(plain) {
        return value.to_string();
    }
    if posix {
        // NUL is the one byte no shell word can hold
        let value = value.replace('\0', "");
        return shlex::try_quote(&value).map_or_else(|_| format!("'{value}'"), |q| q.into_owned());
    }
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// Runs `command` in `cwd` until it exits or `timeout` passes.
///
/// Every ending but a clean exit takes the whole tree with it, including the ones nobody writes
/// code for: a cancel drops this future, and [`KillTreeOnDrop`] is what answers that.
pub(crate) async fn run_captured(
    command: &str,
    cwd: &Path,
    timeout: Duration,
) -> std::io::Result<Outcome> {
    let mut child = shell_command(command, cwd).spawn()?;
    let pid = child.id().unwrap_or(0);
    // Armed from here to the first normal exit: every early return, and every drop this function
    // never sees, takes the whole tree with it.
    let mut reaper = KillTreeOnDrop::arm(pid);
    let pipes = Pipes::attach(&mut child);
    let (stdout, stderr) = pipes.sinks();
    // Not `tokio::time::timeout`: that drops the wait on expiry, and with it the shell
    // (`kill_on_drop`), before the survivors can be listed. A group that loses its shell while a
    // member is suspended is sent SIGHUP by the kernel, so the very processes a timeout most
    // needs to name would be gone. Pinned here, the wait outlives the listing.
    let wait = wait_for_output(child, pipes);
    tokio::pin!(wait);
    tokio::select! {
        captured = &mut wait => {
            let captured = captured?;
            reaper.disarm();
            Ok(Outcome::Finished(captured))
        }
        () = tokio::time::sleep(timeout) => {
            let printed = printed_so_far(&stdout, &stderr);
            // asked before the guard kills them: which processes were still there, and in what
            // state, is the difference between "slow" and "stuck"
            let survivors = luxe_core::program::group_members(pid).await;
            Ok(Outcome::TimedOut(TimedOut { printed, survivors }))
        }
    }
}

/// Kills the command's whole process group unless the command finished on its own.
///
/// `kill_on_drop` reaps the direct child — the shell — and orphans everything the shell started.
/// A **cancel** ends the call by *dropping* its future, so there is no code path to hang an
/// explicit kill on: pressing Cancel on a `cargo build` left the build running, holding its locks
/// and its CPU, with nothing left watching it.
///
/// Disarmed once the child has been reaped, because a reaped pid can be reused and killing a
/// *group* by a recycled id is the one mistake worse than leaking one.
struct KillTreeOnDrop(Option<u32>);

impl KillTreeOnDrop {
    fn arm(pid: u32) -> Self {
        Self((pid != 0).then_some(pid))
    }

    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for KillTreeOnDrop {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            luxe_core::program::kill_tree(pid);
        }
    }
}

/// One stream's bounded capture: its first bytes, its last bytes, and a count of what fell between.
#[derive(Default)]
pub(crate) struct Capture {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    dropped: u64,
}

impl Capture {
    /// Takes `bytes` into the head until it is full, then into the tail, dropping the tail's oldest.
    pub(crate) fn push(&mut self, mut bytes: &[u8]) {
        let room = HEAD_BYTES.saturating_sub(self.head.len());
        let (into_head, rest) = bytes.split_at(room.min(bytes.len()));
        self.head.extend_from_slice(into_head);
        bytes = rest;
        self.tail.extend(bytes);
        if self.tail.len() > TAIL_BYTES {
            let excess = self.tail.len() - TAIL_BYTES;
            self.tail.drain(..excess);
            self.dropped += excess as u64;
        }
    }

    /// Bytes held, which never passes [`HEAD_BYTES`] + [`TAIL_BYTES`].
    #[cfg(test)]
    pub(crate) fn kept(&self) -> usize {
        self.head.len() + self.tail.len()
    }

    /// Bytes that fell between head and tail.
    #[cfg(test)]
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The capture as text, with a line saying how much is missing where it went missing.
    ///
    /// Head and tail are decoded together when nothing fell between them, so a character that
    /// straddles the head's end survives whole.
    pub(crate) fn text(&self) -> String {
        let (front, back) = self.tail.as_slices();
        if self.dropped == 0 {
            return String::from_utf8_lossy(&[self.head.as_slice(), front, back].concat())
                .into_owned();
        }
        let head = String::from_utf8_lossy(&self.head);
        let tail = String::from_utf8_lossy(&[front, back].concat()).into_owned();
        format!("{head}\n… {} bytes of output not kept …\n{tail}", self.dropped)
    }
}

type Sink = Arc<Mutex<Capture>>;

/// One pipe's reader, which goes when the call does.
///
/// Aborting on drop is the obligation every spawn inside a tool carries: a timed-out or cancelled
/// call must not leave a task behind holding a pipe nobody will read.
struct Pump(tokio::task::JoinHandle<()>);

impl Drop for Pump {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Reads `pipe` into a capture the caller can still read after the reader stops.
fn pump(pipe: Option<impl tokio::io::AsyncRead + Unpin + Send + 'static>) -> (Sink, Pump) {
    use tokio::io::AsyncReadExt;

    let sink = Sink::default();
    let into = sink.clone();
    let task = tokio::spawn(async move {
        let Some(mut pipe) = pipe else { return };
        let mut buffer = [0u8; 8192];
        while let Ok(read) = pipe.read(&mut buffer).await {
            if read == 0 {
                return;
            }
            if let Ok(mut capture) = into.lock() {
                capture.push(&buffer[..read]);
            }
        }
    });
    (sink, Pump(task))
}

/// A command's two output pipes, each being read into a capture the caller keeps a handle to.
struct Pipes {
    stdout: Sink,
    stdout_pump: Pump,
    stderr: Sink,
    stderr_pump: Pump,
}

impl Pipes {
    fn attach(child: &mut tokio::process::Child) -> Self {
        let (stdout, stdout_pump) = pump(child.stdout.take());
        let (stderr, stderr_pump) = pump(child.stderr.take());
        Self { stdout, stdout_pump, stderr, stderr_pump }
    }

    fn sinks(&self) -> (Sink, Sink) {
        (self.stdout.clone(), self.stderr.clone())
    }
}

fn text_of(sink: &Sink) -> String {
    sink.lock().map(|capture| capture.text()).unwrap_or_default()
}

/// The end of what a command printed before it was stopped, for a result that has to say where
/// it got to.
fn printed_so_far(stdout: &Sink, stderr: &Sink) -> String {
    let both = [text_of(stdout), text_of(stderr)]
        .into_iter()
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let both = both.trim_end();
    if both.len() <= TIMEOUT_TAIL_BYTES {
        return both.to_string();
    }
    let start = crate::output_limit::ceil_boundary(both, both.len() - TIMEOUT_TAIL_BYTES);
    format!("… {} earlier bytes not shown …\n{}", start, &both[start..])
}

/// Waits for the shell to exit, then takes what it wrote.
///
/// Deliberately not `wait_with_output`, which waits for the pipe to reach **EOF** — held open by
/// every process that inherited it, however long it lives. Launching a GUI app or a daemon from a
/// `&&` list is enough to do it: the `&` backgrounds the whole list, so the subshell keeps this
/// call's pipe while only the last command gets the redirections. A command that reported `sleep
/// 8` then sat for a minute is what that looks like from the outside.
///
/// The captures are the caller's (see [`Pipes`]), so a deadline that drops this future still
/// leaves it whatever the command had printed.
async fn wait_for_output(
    mut child: tokio::process::Child,
    pipes: Pipes,
) -> std::io::Result<Captured> {
    let Pipes { stdout, mut stdout_pump, stderr, mut stderr_pump } = pipes;
    let status = child.wait().await?;
    let mut detached_writer = false;
    for reader in [&mut stdout_pump, &mut stderr_pump] {
        // Still reading after the grace means something that outlived the shell has the write
        // end. The reader is dropped either way, so it learns its reader is gone.
        if tokio::time::timeout(DRAIN_GRACE, &mut reader.0).await.is_err() {
            detached_writer = true;
        }
    }
    Ok(Captured { stdout: text_of(&stdout), stderr: text_of(&stderr), status, detached_writer })
}

/// Tees a command's output to `.luxe/artifacts/<prefix>-<millis>.log` so the model can recover
/// what a summary dropped. Returns the path, or `None` when nothing could be written.
///
/// Created exclusively, with a counter after the time when that name is taken: the calls of one
/// reply run together, and two finishing in one millisecond must not overwrite each other.
pub(crate) fn tee_artifact(
    cwd: &Path,
    prefix: &str,
    command: &str,
    captured: (&str, &str, Option<i32>),
) -> Option<PathBuf> {
    use std::io::Write;

    let (stdout, stderr, exit) = captured;
    let dir = cwd.join(".luxe").join("artifacts");
    std::fs::create_dir_all(&dir).ok()?;
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let body = format!(
        "$ {command}\n\n=== STDOUT ===\n{stdout}\n=== STDERR ===\n{stderr}\n=== EXIT ===\n{}\n",
        exit.unwrap_or(-1)
    );
    for attempt in 0..1_000u32 {
        let name = match attempt {
            0 => format!("{prefix}-{millis}.log"),
            n => format!("{prefix}-{millis}-{n}.log"),
        };
        let path = dir.join(name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => return file.write_all(body.as_bytes()).ok().map(|()| path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests;
