//! Logging — a redacting, file-based `tracing` subscriber.
//!
//! The TUI owns the terminal, so diagnostics can't go to stdout/stderr; they go to a file instead.
//! Every line is passed through the same secret redaction as the transcript, so `tracing` never
//! leaks a key.
//!
//! Which file depends on what the run *is*, and that is a correctness question rather than a tidy
//! one. A session writes to the project it works on (`.luxe/logs/luxe.log`, gitignored). Everything
//! else — `auth`, `doctor`, `models`, `wire` — writes to the user's state directory, because
//! creating `.luxe/` is not a side effect a reporting command is entitled to: [`luxe_core::project`]
//! ranks a directory holding one above a nearer checkout, so a stray left in `/tmp` re-roots every
//! repository beneath it.
//!
//! Level is controlled by `LUXE_LOG` (or `RUST_LOG`). The default is `info` for Luxe's own crates
//! and `warn` for everything else — a normal run leaves a readable account of what the agent did,
//! without `hyper` and `rustls` narrating every socket. Init is best-effort: logging must never
//! crash or block the app.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

/// A `MakeWriter` that redacts each formatted log line before appending it to the file.
#[derive(Clone)]
struct RedactingFile(Arc<Mutex<File>>);

impl io::Write for RedactingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // The same scrub as tool output: patterns plus every literal the process resolved. A token
        // is contiguous within one field write, so a per-write pass is enough.
        let redacted = luxe_core::redact::process_secrets().redact(&String::from_utf8_lossy(buf));
        if let Ok(mut file) = self.0.lock() {
            file.write_all(redacted.as_bytes())?;
        }
        // Always report the original length consumed so the fmt layer never loops.
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.0.lock() {
            Ok(mut file) => file.flush(),
            Err(_) => Ok(()),
        }
    }
}

impl<'a> MakeWriter<'a> for RedactingFile {
    type Writer = RedactingFile;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Luxe's own crates, which is what the default filter turns up.
///
/// Spelled out rather than matched by prefix because `EnvFilter` has no glob for a target prefix, and
/// a filter that quietly matched nothing would look exactly like a quiet run.
const LUXE_TARGETS: [&str; 8] = [
    "luxe",
    "luxe_cli",
    "luxe_core",
    "luxe_dap",
    "luxe_lsp",
    "luxe_mcp",
    "luxe_providers",
    "luxe_tools",
];

/// The default filter: Luxe at `info`, everything else at `warn`.
///
/// Two things have to be true at once. A normal run should leave an account someone can read — which
/// `warn` did not, and which is why routine loop behavior had drifted up to `warn!` just to be
/// visible at all. And it must not become a firehose: `hyper`, `rustls` and `reqwest` log per-socket
/// detail at `debug`, and `tokio` at `trace`, none of which belongs in a session log by default.
///
/// `LUXE_LOG` or `RUST_LOG` overrides it wholesale, in the usual `EnvFilter` syntax.
fn env_filter() -> tracing_subscriber::EnvFilter {
    use tracing_subscriber::EnvFilter;

    for variable in ["LUXE_LOG", "RUST_LOG"] {
        let Ok(value) = std::env::var(variable) else { continue };
        if value.trim().is_empty() {
            continue;
        }
        match EnvFilter::try_new(&value) {
            Ok(filter) => return filter,
            // A typo in a filter directive must not silence the log it was meant to widen.
            Err(error) => eprintln!("luxe: ignoring {variable}=\"{value}\": {error}"),
        }
    }

    let mut filter = EnvFilter::new("warn");
    for target in LUXE_TARGETS {
        if let Ok(directive) = format!("{target}=info").parse() {
            filter = filter.add_directive(directive);
        }
    }
    filter
}

/// Where a run's log goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LogHome {
    /// The project this run works on: `<root>/.luxe/logs/`, beside its sessions.
    Project(std::path::PathBuf),
    /// The user's state directory, for a command that owns no project state.
    User,
}

impl LogHome {
    /// The directory to append `luxe.log` to, creating it and — for a project — its `.gitignore`.
    fn prepare(&self) -> Option<std::path::PathBuf> {
        let log_dir = match self {
            Self::Project(root) => root.join(".luxe").join("logs"),
            Self::User => luxe_core::user_dirs::luxe_state_dir().join("logs"),
        };
        std::fs::create_dir_all(&log_dir).ok()?;
        if let Self::Project(root) = self {
            ensure_self_ignored(&root.join(".luxe"));
        }
        Some(log_dir)
    }
}

/// Initialize the global tracing subscriber to append redacted logs under `home`.
///
/// Best-effort and idempotent: a filesystem error or an already-installed subscriber is
/// swallowed so logging can never take down the process.
pub(crate) fn init(home: &LogHome) {
    let filter = env_filter();

    let Some(log_dir) = home.prepare() else {
        return;
    };
    let Ok((file, rotation_failed)) = open_log(&log_dir, ROTATE_AT_BYTES) else {
        return;
    };

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_target(true)
        .with_writer(RedactingFile(Arc::new(Mutex::new(file))))
        .try_init();
    if let Some(error) = rotation_failed {
        tracing::warn!(%error, "could not rotate the log, so it keeps growing");
    }
}

/// Past this size a log is rotated when it is opened, keeping one previous file as `luxe.log.1`.
const ROTATE_AT_BYTES: u64 = 5 * 1024 * 1024;

/// Open `luxe.log` in `dir` for appending, owner-only, first moving aside one past `rotate_at`.
///
/// A rotation that fails is handed back rather than logged: nothing is listening yet.
fn open_log(dir: &Path, rotate_at: u64) -> io::Result<(File, Option<io::Error>)> {
    let path = dir.join("luxe.log");
    let rotated = match std::fs::metadata(&path) {
        Ok(meta) if meta.len() > rotate_at => std::fs::rename(&path, dir.join("luxe.log.1")).err(),
        _ => None,
    };
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let file = options.open(&path)?;
    // a log an older Luxe created at the umask's default is tightened too
    #[cfg(unix)]
    file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    Ok((file, rotated))
}

/// Drop a self-ignoring `.gitignore` into `.luxe/`, and upgrade one Luxe wrote itself.
///
/// The directory ignores itself (the trick pytest/ruff/venv use), so running Luxe inside any repo
/// needs no root-`.gitignore` edit or global git config. The rule is an *allowlist*: everything is
/// machine-local by default (`*` — sessions, logs, caches, review memory, per-user UI prefs), with
/// explicit exceptions for the handful of project-config files a team may want to version
/// (`permissions.json`, `mcp.json`, `hooks.json`, `skills/`). Ignoring by default is the safe
/// direction: a new artifact kind Luxe writes later stays out of git until someone opts it in.
///
/// A file Luxe did not author is never touched, so a user's own curation survives.
fn ensure_self_ignored(luxe_dir: &Path) {
    let ignore = luxe_dir.join(".gitignore");
    match std::fs::read_to_string(&ignore) {
        // Ours and out of date: an earlier version un-ignored itself, which kept `.luxe/`
        // permanently visible to `git status`. Rewrite it.
        Ok(existing)
            if existing.starts_with(SELF_IGNORE_HEADER) && existing != SELF_IGNORE_BODY =>
        {
            let _ = std::fs::write(&ignore, SELF_IGNORE_BODY);
        }
        Ok(_) => {}
        Err(_) => {
            let _ = std::fs::write(&ignore, SELF_IGNORE_BODY);
        }
    }
}

/// First line of a luxe-authored `.luxe/.gitignore`, so an upgrade never touches a hand-edited one.
const SELF_IGNORE_HEADER: &str = "# Created automatically by luxe";

/// Body of the generated `.luxe/.gitignore`. Ignore-all with carve-outs for shareable config;
/// `skills/` needs both the dir and `skills/**` to re-include its contents past `*`.
///
/// The file ignores *itself* too: nobody commits it, so a `!.gitignore` exception would only leave
/// one un-ignored file inside `.luxe/`, which is all Git needs to report `?? .luxe/` in every
/// status, every review and every changes list, forever. Git reads the file whether or not it is
/// tracked, and ignore rules never apply to already-tracked files, so the carve-outs below still
/// work and a clone simply regenerates this on first run.
const SELF_IGNORE_BODY: &str = "\
# Created automatically by luxe — do not delete.
#
# Everything under .luxe/ is machine-local and ignored by default: sessions, logs,
# caches, todos, review memory, and per-user UI prefs. None of it belongs in git,
# and luxe keeps it out of its own reviews and changes lists for the same reason.
*

# Exceptions: project config you may want to version and share. Luxe rewrites
# this file while its first line says it made it; to choose for yourself, replace
# the file, first line included, with your own.
!permissions.json
!mcp.json
!hooks.json
!skills/
!skills/**
";

#[cfg(test)]
mod tests;
