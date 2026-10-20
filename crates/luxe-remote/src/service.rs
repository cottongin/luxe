//! Service — running the bridge at login, as the platform's own per-user service.
//!
//! systemd on Linux, launchd on macOS, and the `HKCU` Run key on Windows: each one is per user and
//! needs no admin rights. What runs is written down at install ([`Launch`]), with the owner's login
//! as an `--allow`, so a service started before the network (or without `tailscale` on its `PATH`,
//! as launchd's is) still knows who to let in.

use std::path::PathBuf;

#[cfg(any(test, target_os = "macos"))]
mod launchd;
#[cfg(any(test, windows))]
mod run_key;
#[cfg(any(test, all(unix, not(target_os = "macos"))))]
mod systemd;

/// What the service runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// The `luxe` binary
    pub program: PathBuf,
    /// Its arguments: `remote` and the bridge's flags
    pub args: Vec<String>,
}

/// Why the service couldn't be installed or removed.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// A file couldn't be written or removed
    #[error("could not {action} {path}: {source}")]
    Io {
        /// What was being done
        action: &'static str,
        /// To which file
        path: PathBuf,
        /// The operating system's reason
        #[source]
        source: std::io::Error,
    },
    /// The platform's service manager refused
    #[error("`{command}` failed: {detail}")]
    Command {
        /// The command line that failed
        command: String,
        /// What it said
        detail: String,
    },
    /// Nowhere to put a service: no home folder, or a platform without a per-user service manager
    #[error("this system has no per-user service manager Luxe knows how to use")]
    Unsupported,
}

/// Installs and starts the service, replacing one already there. Returns where it was recorded.
pub fn install(launch: &Launch) -> Result<String, ServiceError> {
    #[cfg(target_os = "macos")]
    return launchd::install(launch);
    #[cfg(all(unix, not(target_os = "macos")))]
    return systemd::install(launch);
    #[cfg(windows)]
    return run_key::install(launch);
    #[cfg(not(any(unix, windows)))]
    {
        let _ = launch;
        Err(ServiceError::Unsupported)
    }
}

/// Stops and removes the service. `false` when there was none.
pub fn remove() -> Result<bool, ServiceError> {
    #[cfg(target_os = "macos")]
    return launchd::remove();
    #[cfg(all(unix, not(target_os = "macos")))]
    return systemd::remove();
    #[cfg(windows)]
    return run_key::remove();
    #[cfg(not(any(unix, windows)))]
    Err(ServiceError::Unsupported)
}

/// Whether the service is installed.
pub fn installed() -> bool {
    #[cfg(target_os = "macos")]
    return launchd::installed();
    #[cfg(all(unix, not(target_os = "macos")))]
    return systemd::installed();
    #[cfg(windows)]
    return run_key::installed();
    #[cfg(not(any(unix, windows)))]
    false
}

/// Runs a service manager's command, turning a failure into what it said.
#[cfg(any(unix, windows))]
fn run(program: &str, args: &[&str]) -> Result<(), ServiceError> {
    let command = || format!("{program} {}", args.join(" "));
    let output = std::process::Command::new(program).args(args).output();
    let output = output
        .map_err(|error| ServiceError::Command { command: command(), detail: error.to_string() })?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(ServiceError::Command { command: command(), detail })
}

#[cfg(unix)]
fn write_file(path: &std::path::Path, contents: &str) -> Result<(), ServiceError> {
    let io = |action| move |source| ServiceError::Io { action, path: path.to_path_buf(), source };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io("create the folder for"))?;
    }
    luxe_core::fs_atomic::replace(path, contents.as_bytes()).map_err(io("write"))
}

/// Removes `path`, `false` when it wasn't there.
#[cfg(unix)]
fn remove_file(path: &std::path::Path) -> Result<bool, ServiceError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(ServiceError::Io { action: "remove", path: path.to_path_buf(), source }),
    }
}
