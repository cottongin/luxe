//! Update notice — whether a newer Luxe exists, and what *this* user should run to get it.
//!
//! Luxe never rewrites its own binary. It notices that it is behind and says so in the dashboard
//! footer, and on request runs the update command that matches how this copy was installed, so
//! the install's owner does the replacing. Telling somebody who installed through Homebrew to run
//! `cargo install --force` is worse than saying nothing: it is wrong, and following it leaves them
//! with two Luxes and no idea which one is on `PATH`.
//!
//! Everything here is pure or reads a local file. The one network call lives in `luxe-cli`, which
//! is where outbound requests belong; this module only decides what to make of the answer. That
//! split is also what makes the interesting half testable without a server.
//!
//! # Why this is not a phone-home
//!
//! `PROJECT.md` rules out telemetry. The distinction is direction: telemetry *reports state*, and
//! this only *asks a question*. The request carries no version, no platform, no identifier and no
//! session count — comparison happens here, against a manifest the server would have served to
//! anybody. What a server learns is that an address fetched a public file, which is what it learns
//! from a browser. Keep it that way: the moment a version string goes into the request, this
//! becomes a different feature that needs a different argument.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a completed check stays current. A day: releases are not hourly, and the notice is a
/// convenience rather than a security control.
pub const CHECK_TTL: Duration = Duration::from_secs(24 * 3600);

/// Where the release manifest is published. Overridable with `LUXE_UPDATE_URL`, which is also how
/// the tests point it at a local file.
pub const RELEASE_MANIFEST_URL: &str =
    "https://api.github.com/repos/evanwhittaker/luxe/releases/latest";

/// Where a human downloads a build by hand.
pub const RELEASES_PAGE: &str = "https://github.com/evanwhittaker/luxe/releases";

/// How this copy of Luxe got onto the machine.
///
/// Determines the advice, and would determine a self-updater's *refusal* if one were ever built —
/// a binary owned by a package manager must decline and name its owner rather than fight it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallSource {
    /// `cargo install luxe-cli`, living in a Cargo bin directory.
    Cargo,
    /// A Homebrew prefix, on either macOS or Linux.
    Homebrew,
    /// A path a distribution's package manager owns (`/usr/bin`, `/bin`).
    SystemPackage,
    /// A release archive, an installer script, `/usr/local/bin`, or a local build.
    Archive,
}

impl InstallSource {
    /// Classify the running binary. Falls back to [`InstallSource::Archive`], which is the state
    /// that gives the most cautious advice ("download the new release") rather than the most
    /// confident.
    pub fn detect() -> Self {
        std::env::current_exe().map(|path| Self::from_path(&path)).unwrap_or(Self::Archive)
    }

    /// Classify a path. Split out from [`Self::detect`] because a function that can only be
    /// exercised by reinstalling Luxe four ways is a function with no tests.
    pub fn from_path(path: &Path) -> Self {
        // `paths::display` is the one owner of "a path becomes a string", forward slashes included,
        // so the needles below read the same on every platform.
        let text = crate::paths::display(path);
        let has = |needle: &str| text.contains(needle);

        // Homebrew before the system prefixes: `/usr/local/Cellar` is a Homebrew path that also
        // starts with a directory the system-package arm would otherwise claim.
        if has("/homebrew/") || has("/Cellar/") || has("/linuxbrew/") {
            return Self::Homebrew;
        }
        if has("/.cargo/bin/") || has("/.rustup/") {
            return Self::Cargo;
        }
        // `/usr/local/bin` is deliberately absent: FHS reserves it for things installed *outside*
        // the package manager, which is exactly the archive case.
        if text.starts_with("/usr/bin/") || text.starts_with("/bin/") {
            return Self::SystemPackage;
        }
        Self::Archive
    }

    /// The commands that update this install, in order, each as a program and its arguments, or
    /// `None` when the honest answer is "however you installed it". A wrong command is worse than
    /// an absent one, so this refuses to guess.
    ///
    /// Run directly rather than through a shell, so nothing in them is ever interpreted. Homebrew
    /// refreshes its index first, or `upgrade` can find nothing newer than the release the notice
    /// already read; cargo installs `--locked`, the dependency versions the release was tested with.
    pub fn update_steps(&self) -> Option<&'static [&'static [&'static str]]> {
        match self {
            Self::Cargo => Some(&[&["cargo", "install", "--locked", "--force", "luxe-cli"]]),
            Self::Homebrew => Some(&[&["brew", "update"], &["brew", "upgrade", "luxe"]]),
            // Which package manager owns it is not knowable without querying one, and guessing
            // `apt` on an Arch machine is the failure this whole function exists to avoid.
            Self::SystemPackage => None,
            Self::Archive => None,
        }
    }

    /// [`Self::update_steps`] as one line a person could type: the steps joined with `&&`.
    pub fn update_command(&self) -> Option<String> {
        let steps = self.update_steps()?;
        Some(steps.iter().map(|step| step.join(" ")).collect::<Vec<_>>().join(" && "))
    }

    /// The advice, as a sentence fragment that follows "— ".
    pub fn advice(&self) -> String {
        match self.update_command() {
            Some(command) => format!("`{command}`"),
            None if *self == Self::SystemPackage => {
                "update it with your system package manager".to_string()
            }
            None => format!("download it from {RELEASES_PAGE}"),
        }
    }
}

/// The cached answer to "what is the newest release", plus when we last asked.
///
/// Shaped after `luxe_providers::catalog::store::Meta` on purpose: same freshness question, same
/// answer. Every field defaults, so a cache written by an older Luxe still loads.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateState {
    /// Newest version the manifest reported, without any `v` prefix.
    pub latest: Option<String>,
    /// Unix seconds of the last completed check, successful or not.
    pub checked_at: u64,
}

impl UpdateState {
    /// Whether the last check is older than `ttl`. A clock that moved backwards reads as fresh
    /// rather than as an infinitely old cache that re-checks on every launch.
    pub fn is_stale(&self, ttl: Duration) -> bool {
        let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else { return true };
        now.as_secs().saturating_sub(self.checked_at) >= ttl.as_secs()
    }

    /// Read the cache, or a never-checked default when it is absent or corrupt.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|body| serde_json::from_str(&body).ok())
            .unwrap_or_default()
    }

    /// Persist atomically, so a launch racing a background check never reads half a file.
    pub fn store(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        crate::fs_atomic::replace(path, &body)
    }
}

/// Where the cached check lives. Derived data, so it sits under the cache directory and losing it
/// costs one request.
pub fn state_path() -> PathBuf {
    crate::user_dirs::luxe_cache_dir().join("update.json")
}

/// Strip a leading `v` and surrounding whitespace from a release tag (`v0.2.0` → `0.2.0`).
pub fn normalize_version(raw: &str) -> &str {
    raw.trim().strip_prefix('v').unwrap_or(raw.trim())
}

/// Whether `latest` is a genuinely newer release than `current`.
///
/// Real semver rather than a string compare, because the orderings disagree exactly where it
/// matters: `0.10.0` sorts before `0.9.0` lexically, and a pre-release must rank *below* the
/// version it leads up to so an `0.2.0-rc.1` user is still told about `0.2.0`. An unparseable
/// version on either side means "say nothing" — a malformed tag is not worth a wrong notice.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |raw: &str| semver::Version::parse(normalize_version(raw)).ok();
    match (parse(latest), parse(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

/// A newer release worth telling the reader about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateNotice {
    /// The newer version.
    pub latest: String,
    /// What is running now.
    pub current: String,
    /// How this copy was installed, which decides the advice.
    pub source: InstallSource,
}

impl UpdateNotice {
    /// The one line the dashboard prints.
    pub fn line(&self) -> String {
        format!(
            "Luxe {} is available (you have {}) — {}",
            self.latest,
            self.current,
            self.source.advice()
        )
    }
}

/// The notice to show, if any: `None` when the cache is empty, unparseable, or not actually behind.
pub fn notice(state: &UpdateState, current: &str, source: InstallSource) -> Option<UpdateNotice> {
    let latest = state.latest.as_deref()?;
    is_newer(latest, current).then(|| UpdateNotice {
        latest: normalize_version(latest).to_string(),
        current: normalize_version(current).to_string(),
        source,
    })
}

#[cfg(test)]
mod tests;
