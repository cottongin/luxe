//! user_dirs — where a user's luxe files live, on each platform luxe runs on.
//!
//! Every user-level file resolves through here. Re-deriving a path from `$HOME` at each call site is
//! how a platform gets forgotten: on Windows `HOME` is usually unset, which lands the file in the
//! *current working directory* — a config file per project, none of them the one the user edited.
//!
//! # What each platform gets
//!
//! - **Unix (Linux and macOS)** — the XDG variable when set, else `~/.config` and `~/.local/state`.
//!   macOS gets XDG paths rather than `~/Library/Application Support` on purpose: luxe is a terminal
//!   program, its documentation says `~/.config/luxe`, and that is where a developer looks first.
//! - **Windows** — `%APPDATA%` for configuration and `%LOCALAPPDATA%` for state, which is the split
//!   the platform intends (roaming settings, machine-local scratch). An XDG variable still wins if
//!   the user has set one, because someone who has gone to that trouble means it.
//!
//! Nothing here creates directories or touches the filesystem; callers already handle a missing path.
//!
//! Every variable here is read as text, so one that is not valid UTF-8 (`HOME`, `LUXE_CONFIG_DIR`,
//! `LUXE_STATE_DIR`, an XDG or Windows one) counts as unset and the next answer is used.
//! [`home_dir`] is the exception: it asks the platform.

use std::path::PathBuf;

/// Where user-level configuration lives: `config.json`, `themes/`, `hooks.json`, credentials.
///
/// Falls back to `./.config` when the platform offers nothing. Prefer [`config_home`] where "we
/// don't know" should mean "don't look".
pub fn config_dir() -> PathBuf {
    config_home().unwrap_or_else(|| PathBuf::from(".").join(".config"))
}

/// Where user-level configuration lives, or `None` when the platform names nowhere.
///
/// Discovery paths want this one: scanning `./.config` because an environment variable was missing
/// finds a directory that belongs to whatever project happens to be open.
pub fn config_home() -> Option<PathBuf> {
    resolve("XDG_CONFIG_HOME", "APPDATA", &[".config"], env)
}

/// Where throwaway caches live: downloaded metadata, snapshots that can be refetched.
pub fn cache_dir() -> PathBuf {
    resolve("XDG_CACHE_HOME", "LOCALAPPDATA", &[".cache"], env)
        .unwrap_or_else(|| PathBuf::from(".").join(".cache"))
}

/// Where machine-local runtime state lives — things that describe *this* checkout or session rather
/// than the user's preferences.
pub fn state_dir() -> PathBuf {
    resolve("XDG_STATE_HOME", "LOCALAPPDATA", &[".local", "state"], env)
        .unwrap_or_else(|| PathBuf::from(".").join(".local").join("state"))
}

/// luxe's own configuration directory: `$LUXE_CONFIG_DIR`, else `luxe` under [`config_dir`].
///
/// Every user-level file luxe keeps resolves through here, so the variable moves all of them
/// together, which is what the guide promises.
pub fn luxe_config_dir() -> PathBuf {
    luxe_config_home().unwrap_or_else(|| config_dir().join("luxe"))
}

/// [`luxe_config_dir`], or `None` when neither the variable nor the platform names anywhere, for
/// discovery that should not scan `./.config`.
pub fn luxe_config_home() -> Option<PathBuf> {
    luxe_dir("LUXE_CONFIG_DIR", config_home(), env)
}

/// luxe's own state directory: `$LUXE_STATE_DIR`, else `luxe` under [`state_dir`]. The state
/// record, logs and MCP tokens all live here.
pub fn luxe_state_dir() -> PathBuf {
    luxe_dir("LUXE_STATE_DIR", None, env).unwrap_or_else(|| state_dir().join("luxe"))
}

/// Where `name` in luxe's config directory is read and written: there, or where an older build
/// left it.
///
/// Builds before [`luxe_config_dir`] had one owner resolved `auth.json` and `models.json`
/// themselves and never read `%APPDATA%`, so a Windows login sat in `%HOME%\.config\luxe`. That
/// file keeps being used where it is until one exists here: nothing is moved or copied, so nothing
/// can be lost or left behind by resolving a path. Every reader and every "edit this file" message
/// goes through here, so none of them creates a new file that would hide the old one.
pub fn luxe_config_file(name: &str) -> PathBuf {
    config_file_in(name, luxe_config_dir(), legacy_luxe_config_dir())
}

/// [`luxe_config_file`] over explicit directories. Args: `(name, dir, legacy_dir)`.
fn config_file_in(name: &str, dir: PathBuf, legacy_dir: Option<PathBuf>) -> PathBuf {
    // the only two files those builds placed themselves; the rest were always read from here
    let placed = matches!(name, "auth.json" | "models.json");
    resolve_file(dir.join(name), legacy_dir.filter(|_| placed).map(|legacy| legacy.join(name)))
}

/// `path`, unless only `legacy` exists.
fn resolve_file(path: PathBuf, legacy: Option<PathBuf>) -> PathBuf {
    match legacy {
        Some(legacy) if !path.exists() && legacy.is_file() => legacy,
        _ => path,
    }
}

/// Where builds before [`luxe_config_dir`] had one owner kept `auth.json` and `models.json`:
/// `$XDG_CONFIG_HOME/luxe`, else `$HOME/.config/luxe`, never `%APPDATA%`. `None` under
/// `LUXE_CONFIG_DIR`, which they read the same way, and with no `HOME`, where they fell back to a
/// `.config` under whatever directory luxe started in, which is not theirs to read.
fn legacy_luxe_config_dir() -> Option<PathBuf> {
    legacy_config(env)
}

/// Where a running luxe leaves the way in to its session: `$LUXE_RUNTIME_DIR`, else `luxe` under a
/// folder only this user can open.
///
/// That is `$XDG_RUNTIME_DIR` on Linux and `$TMPDIR` on macOS, both private to the user. Linux's
/// `$TMPDIR`, when set at all, is usually the shared `/tmp`, so it is never used; without a private
/// folder, and on Windows, it is `run` under [`luxe_state_dir`].
pub fn luxe_runtime_dir() -> PathBuf {
    runtime_in(env, cfg!(target_os = "macos")).unwrap_or_else(|| luxe_state_dir().join("run"))
}

/// [`luxe_runtime_dir`] under the lookup `var`, or `None` for the state-directory fallback.
fn runtime_in(var: impl Fn(&str) -> Option<String>, macos: bool) -> Option<PathBuf> {
    let var = |name: &str| var(name).filter(|value| !value.trim().is_empty());
    if let Some(explicit) = var("LUXE_RUNTIME_DIR") {
        return Some(PathBuf::from(explicit));
    }
    let private = match macos {
        true => var("TMPDIR"),
        false if cfg!(windows) => None,
        false => var("XDG_RUNTIME_DIR"),
    };
    Some(PathBuf::from(private?).join("luxe"))
}

/// luxe's own cache directory, `luxe` under [`cache_dir`].
pub fn luxe_cache_dir() -> PathBuf {
    cache_dir().join("luxe")
}

/// The user's home directory, on whatever platform this is.
///
/// `$HOME` is not set on Windows; `std::env::home_dir` is the one answer that knows about
/// `%USERPROFILE%` and the shell folder behind it.
///
/// NOTE: this is why the workspace pins `rust-version = "1.85"`. Before it, `home_dir` consulted
/// `$HOME` on Windows too, which under Cygwin or MSYS is a path no Windows API can open, and an
/// older toolchain compiles this happily and writes every config file somewhere unreachable.
pub fn home_dir() -> Option<PathBuf> {
    std::env::home_dir()
}

/// A user-facing rendering of where one of luxe's config files lives.
///
/// For the messages that tell someone to go and edit a file: a literal `~/.config/luxe/…` sends a
/// Windows user to a directory that does not exist. Home is collapsed to `~` where it applies.
pub fn config_path_display(file: &str) -> String {
    crate::paths::home_relative(&luxe_config_file(file))
}

/// [`legacy_luxe_config_dir`] under the lookup `var`. Blank is unset.
fn legacy_config(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let var = |name: &str| var(name).filter(|value| !value.trim().is_empty());
    if var("LUXE_CONFIG_DIR").is_some() {
        return None;
    }
    let home = || Some(PathBuf::from(var("HOME")?).join(".config"));
    Some(var("XDG_CONFIG_HOME").map(PathBuf::from).or_else(home)?.join("luxe"))
}

/// The directory `variable` names, else `luxe` under `base`, else `None`. Blank is unset.
fn luxe_dir(
    variable: &str,
    base: Option<PathBuf>,
    var: impl Fn(&str) -> Option<String>,
) -> Option<PathBuf> {
    let named = var(variable).filter(|value| !value.trim().is_empty());
    named.map(PathBuf::from).or_else(|| Some(base?.join("luxe")))
}

/// Reads a variable, treating blank as unset — an exported-but-empty `XDG_CONFIG_HOME` is a shell
/// artifact, not an instruction.
fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.trim().is_empty())
}

/// The directory an XDG variable, a Windows variable, or `$HOME` plus `tail` resolves to.
///
/// Split out from the public functions so the Windows answer is testable from anywhere: pass a
/// lookup that reports whatever environment you want to ask about.
///
/// Args: `(xdg variable, Windows variable, path under home, env lookup)`.
fn resolve(
    xdg: &str,
    windows: &str,
    tail: &[&str],
    var: impl Fn(&str) -> Option<String>,
) -> Option<PathBuf> {
    let var = |name: &str| var(name).filter(|value| !value.trim().is_empty());
    if let Some(explicit) = var(xdg) {
        return Some(PathBuf::from(explicit));
    }
    if cfg!(windows) {
        if let Some(native) = var(windows) {
            return Some(PathBuf::from(native));
        }
    }
    // `HOME` first because a unix user may have overridden it; `USERPROFILE` is Windows' spelling for
    // the same idea, and the last thing standing before "we don't know".
    let mut path = PathBuf::from(var("HOME").or_else(|| var("USERPROFILE"))?);
    for segment in tail {
        path.push(segment);
    }
    Some(path)
}

#[cfg(test)]
mod tests;
