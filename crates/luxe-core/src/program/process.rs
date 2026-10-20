//! Process — starting a program away from the terminal, and reaching the tree it starts.

use std::path::Path;

#[cfg(windows)]
mod job;

/// Kill the process `pid` started *and everything it started*.
///
/// Every timeout path in luxe needs this, and none of them can use the child handle alone:
/// `kill_on_drop` reaps the shell, not the `cargo` the shell spawned, so a timed-out build keeps
/// compiling with nobody waiting for it.
///
/// On unix this is the `kill(2)` syscall against the negative pid — the process *group* the child
/// leads, which is what [`detach_from_terminal`] arranged at spawn. Not `kill(1)`: that is a
/// blocking spawn on an async thread, a `PATH` lookup that can miss, and an argument shape the
/// util-linux and procps-ng binaries disagree about.
///
/// Windows keeps `taskkill /T`, which walks the parent-child links; there is no group to signal and
/// no equivalent syscall.
pub fn kill_tree(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(unix)]
    {
        if let Ok(pid) = i32::try_from(pid) {
            if let Some(pid) = rustix::process::Pid::from_raw(pid) {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
            }
        }
    }
    #[cfg(windows)]
    {
        // `/T` takes the descendants, `/F` does not ask.
        let _ = std::process::Command::new("taskkill")
            .arg("/F")
            .arg("/T")
            .arg("/PID")
            .arg(pid.to_string())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// Whether a process with this id currently exists.
///
/// The question a durable state file has to be able to ask: a file that says `running: true` is
/// only evidence of what the writer intended, and a run that was killed, crashed, or lost its
/// terminal never gets to correct it. Asking the operating system is the only way to tell an
/// unattended run from an abandoned record of one.
///
/// On unix this is `kill(pid, 0)` — the standard existence check, which sends nothing. Windows has
/// no equivalent syscall reachable without a new dependency, so it asks `tasklist` for that id;
/// slower, but this is a status path, not a hot one.
///
/// Two honest limits. A process id is reused eventually, so a very old id can name somebody else's
/// process; a caller that cares pairs this with the hostname and a start time, which is what
/// `LoopState` does. And on unix a process owned by another user answers `EPERM` rather than
/// `ESRCH` — it exists, which is the answer, so that counts as alive.
pub fn process_is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        let Ok(raw) = i32::try_from(pid) else { return false };
        let Some(pid) = rustix::process::Pid::from_raw(raw) else { return false };
        match rustix::process::test_kill_process(pid) {
            Ok(()) => true,
            // It exists; we are simply not allowed to signal it.
            Err(error) => error == rustix::io::Errno::PERM,
        }
    }
    #[cfg(windows)]
    {
        let Ok(output) = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
        else {
            // No `tasklist` is not evidence the process is gone, and claiming it is would let a
            // second loop start over a live one.
            return true;
        };
        // A filter that matches nothing prints an INFO line rather than an empty result, so the
        // test is whether the id came back in a row of its own.
        String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        true
    }
}

/// Which machine this is, for a record that outlives the process that wrote it.
///
/// A process id only means something on the box that issued it, and a project directory is not
/// always on one box: a synced folder or a network share carries `.luxe/loop_state.json` between
/// machines, where a stale id can name a live stranger. Pairing the id with this makes the
/// question answerable instead of merely plausible.
///
/// Best-effort by design — an unknown host is not an error, it just means the record cannot be
/// trusted to be about this machine, which is the safer of the two ways to be wrong.
pub fn hostname() -> String {
    #[cfg(unix)]
    {
        let uname = rustix::system::uname();
        let name = uname.nodename().to_string_lossy().trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }
    #[cfg(windows)]
    {
        if let Ok(name) = std::env::var("COMPUTERNAME") {
            let name = name.trim().to_string();
            if !name.is_empty() {
                return name;
            }
        }
    }
    "unknown".to_string()
}

/// Start this command away from luxe's terminal: leading a session of its own, and with it a process
/// group, so [`kill_tree`] can reach its descendants and nothing in it can use the terminal.
///
/// Everything luxe runs on its own comes through here (`bash`, [`captured_command`], hooks,
/// formatters, MCP servers, debug adapters), and the terminal belongs to the TUI. A process group
/// alone was not enough: still in luxe's session, a member that read `/dev/tty` or changed its
/// settings (a spinner's `stty -echo`, a password prompt) had the kernel suspend the whole group,
/// foreground shell included, until the call timed out, and a plain write drew over the screen. A
/// session of its own has no controlling terminal: `/dev/tty` does not open, and a program that
/// probes for one takes its non-interactive path. Codex starts its commands the same way, and
/// opencode, Pi and Claude Code get it from Node's `detached`.
///
/// On Windows, where there is no controlling terminal to leave, the child gets a console of its own
/// that no window shows: one it shared with luxe was luxe's terminal tab, and a program that names
/// its console, as npm does, renamed the tab. Its tree is reached through [`TreeGuard`]'s job.
pub fn detach_from_terminal(command: &mut tokio::process::Command) {
    // SAFETY: the hook runs in the forked child before `exec`, where only async-signal-safe work is
    // allowed. `new_session` makes one or two raw syscalls through rustix and touches nothing else:
    // no allocation, no lock, no state shared with the parent. `std`'s own `setsid` for `Command`
    // is not stable.
    #[cfg(unix)]
    #[allow(unsafe_code)]
    unsafe {
        command.pre_exec(new_session);
    }
    #[cfg(windows)]
    {
        // `CREATE_NO_WINDOW`: a console for the child, shown in no window
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(any(unix, windows)))]
    let _ = command;
}

/// A started program's whole tree, killed when dropped armed: on a timeout, and on a cancel, which
/// ends a call by dropping it. Disarm it once the program has exited on its own; nothing is killed
/// then, and a background process it left keeps running, as one does from a terminal.
///
/// On unix the tree is the process group [`detach_from_terminal`] made it lead. On Windows it is a
/// Job Object the child joins as it starts: `taskkill /T` walks parent links, and a shell killed
/// first, as `kill_on_drop` kills it, left its children unlinked and running.
pub struct TreeGuard {
    pid: Option<u32>,
    #[cfg(windows)]
    job: Option<job::Job>,
}

impl TreeGuard {
    /// Arms a guard over `child` and everything it starts from here on.
    pub fn arm(child: &tokio::process::Child) -> Self {
        Self {
            pid: child.id().filter(|pid| *pid != 0),
            #[cfg(windows)]
            job: child.raw_handle().and_then(job::Job::holding),
        }
    }

    /// The program exited on its own: leave what it started alone. A reaped id can be reused, and
    /// killing a tree by a recycled id is the one mistake worse than leaking one.
    pub fn disarm(&mut self) {
        self.pid = None;
    }
}

impl Drop for TreeGuard {
    fn drop(&mut self) {
        let Some(pid) = self.pid.take() else { return };
        #[cfg(windows)]
        if let Some(job) = &self.job {
            job.terminate();
        }
        kill_tree(pid);
    }
}

/// Takes stdin for the caller alone, and gives every process luxe starts the null device instead.
///
/// `luxe --rpc` reads its protocol from stdin for as long as it runs, and everything it starts
/// would otherwise share that pipe. On Unix, a child that reads stdin eats frames meant for luxe. On
/// Windows it is worse: synchronous I/O on one pipe is serialized, so a child holding the inherited
/// handle stalls behind luxe's pending read until the client next writes, and a client waiting on
/// that child never does. Every git a review load runs sat there, and so would any tool call.
///
/// The returned file is a private copy nothing inherits: close-on-exec on Unix, not inheritable on
/// Windows, where the original handle loses its inheritance too, because Windows hands a child every
/// inheritable handle, not just its stdio. Call it once, before anything is started.
pub fn take_stdin() -> std::io::Result<std::fs::File> {
    let null = std::fs::File::open(if cfg!(windows) { "NUL" } else { "/dev/null" })?;
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;
        let private = std::io::stdin().as_fd().try_clone_to_owned()?;
        rustix::stdio::dup2_stdin(&null)?;
        Ok(private.into())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsHandle, AsRawHandle, IntoRawHandle};
        use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT};
        use windows_sys::Win32::System::Console::{SetStdHandle, STD_INPUT_HANDLE};
        let stdin = std::io::stdin();
        let private = stdin.as_handle().try_clone_to_owned()?;
        let original = stdin.as_raw_handle();
        // the process's stdin from here on, so it is never closed
        let null = null.into_raw_handle();
        // SAFETY: both calls take handles by value and touch no Rust memory. `null` is a live handle
        // the process now owns for good. `original` is whatever stdin was, which may be no handle at
        // all, and `SetHandleInformation` refuses that with an error rather than misbehaving. Std
        // reads the std handle afresh on every access, so nothing caches the one replaced.
        #[allow(unsafe_code)]
        let (replaced, _) = unsafe {
            (
                SetStdHandle(STD_INPUT_HANDLE, null),
                SetHandleInformation(original, HANDLE_FLAG_INHERIT, 0),
            )
        };
        if replaced == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(private.into())
    }
}

/// `setsid`, or, where that is refused, at least a process group of its own.
///
/// `setsid` fails only for a process that already leads a group, which a freshly forked child
/// never does; the fallback keeps [`kill_tree`] working regardless.
#[cfg(unix)]
fn new_session() -> std::io::Result<()> {
    match rustix::process::setsid() {
        Ok(_) => Ok(()),
        Err(rustix::io::Errno::PERM) => {
            rustix::process::setpgid(None, None).map_err(std::io::Error::from)
        }
        Err(error) => Err(error.into()),
    }
}

/// A command wired for a bounded run nobody is watching: no terminal, captured output, and a process
/// tree [`super::run_bounded`] can reap.
///
/// - **stdin is closed.** `gh pr create` without a `--title` *prompts*, and a prompt here would sit
///   invisibly behind the TUI eating the user's keystrokes. Closed, it fails at once and says what
///   it wanted.
/// - **stdout and stderr are piped explicitly**, because `spawn` does not: `Command::output` sets
///   these for you and `spawn` leaves them **inherited**, which would write over the TUI's screen
///   and hand the model an error with the reason missing.
/// - **It leads its own session, and so its own process group** ([`detach_from_terminal`]).
///   `kill_on_drop` reaps the child and nothing it started, and the programs run this way start
///   things: a pre-commit hook, a `gpg` pinentry, a credential helper. None of them can reach the
///   terminal either, so a prompt fails at once instead of waiting behind the TUI.
pub fn captured_command<S: AsRef<std::ffi::OsStr>>(
    program: S,
    args: &[&str],
    cwd: &Path,
) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    detach_from_terminal(&mut command);
    command
}

/// One process in a process group, as `ps` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupMember {
    /// Its process id
    pub pid: u32,
    /// `ps`'s state code: `S` sleeping, `R` running, `T` stopped, `Z` exited and unreaped, with
    /// platform modifiers after the first letter
    pub state: String,
    /// Its command line
    pub command: String,
}

impl GroupMember {
    /// Whether it is suspended, which no amount of waiting will undo.
    ///
    /// A command started by [`detach_from_terminal`] has no terminal to be suspended by, so this is
    /// now something the command did to itself: a `SIGSTOP`, `kill -TSTP`, or a debugger holding it.
    pub fn is_stopped(&self) -> bool {
        self.state.starts_with('T')
    }
}

/// The processes still in process group `pgid`, by pid.
///
/// Asked of `ps` rather than `/proc` because macOS has no `/proc`, and `-A -ww -o
/// pid=,pgid=,stat=,args=` means the same to procps and to BSD `ps`. Empty on Windows, where there
/// is no group to ask about, and wherever `ps` cannot answer within a few seconds.
pub async fn group_members(pgid: u32) -> Vec<GroupMember> {
    #[cfg(unix)]
    {
        let Some(ps) = super::resolve("ps") else { return Vec::new() };
        let command = captured_command(
            ps,
            &["-A", "-ww", "-o", "pid=,pgid=,stat=,args="],
            &std::env::temp_dir(),
        );
        match super::run_bounded(command, std::time::Duration::from_secs(3)).await {
            Ok(output) => parse_group(&String::from_utf8_lossy(&output.stdout), pgid),
            Err(_) => Vec::new(),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pgid;
        Vec::new()
    }
}

/// The rows of `ps -o pid=,pgid=,stat=,args=` output that belong to group `pgid`.
#[cfg(any(unix, test))]
fn parse_group(listing: &str, pgid: u32) -> Vec<GroupMember> {
    let mut members: Vec<GroupMember> = listing
        .lines()
        .filter_map(|line| {
            let mut rest = line.trim_start();
            let mut field = || {
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                let (head, tail) = rest.split_at(end);
                rest = tail.trim_start();
                head
            };
            let pid = field().parse::<u32>().ok()?;
            let group = field().parse::<u32>().ok()?;
            let state = field().to_string();
            (group == pgid && !state.is_empty()).then(|| GroupMember {
                pid,
                state,
                command: rest.trim_end().to_string(),
            })
        })
        .collect();
    members.sort_by_key(|member| member.pid);
    members
}

#[cfg(test)]
mod tests;
