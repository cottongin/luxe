//! `luxe update` — run the update this install's owner provides, then check that it took.
//!
//! The policy is [`luxe_core::update`]'s: which owner installed this copy, which commands update
//! it, and a refusal where luxe cannot know them. This runs those commands one program at a time
//! with the terminal attached, so the reader sees the package manager's own output. A copy luxe's
//! installer script put there is updated by running that script again ([`installer`]). Either way
//! luxe never writes a binary itself. `/update` on the dashboard comes here too, once the TUI has
//! handed the terminal back.

use anyhow::{anyhow, bail, Context};
use luxe_core::update::{self, InstallSource};
use std::path::{Path, PathBuf};

pub(crate) use aside::remove_leftover;

/// Checks for a newer release now and, unless `check_only`, runs the update and checks the result.
pub(crate) async fn run(check_only: bool) -> anyhow::Result<()> {
    let current = luxe_core::version::VERSION;
    let latest = crate::update_check::check_now()
        .await
        .map_err(|error| anyhow!("could not check for a newer release: {error}"))?;
    if !update::is_newer(&latest, current) {
        println!("luxe {current} is the newest release.");
        return Ok(());
    }
    println!("luxe {latest} is available (you have {current}).");
    let source = InstallSource::detect();
    match route(source, check_only) {
        Some(route) => update_with(route, &latest).await,
        None => {
            println!("{}", instructions(source, check_only));
            Ok(())
        }
    }
}

/// How `luxe update` updates a copy.
#[derive(Debug, PartialEq, Eq)]
enum Route {
    /// The owner's own commands, in order
    Steps(&'static [&'static [&'static str]]),
    /// The release's installer script, run again over this copy
    Installer,
}

/// The route for `source`, or `None` when luxe only says what to do: the reader asked just to
/// check, or no owner's update is known.
fn route(source: InstallSource, check_only: bool) -> Option<Route> {
    if check_only {
        return None;
    }
    match source {
        InstallSource::Installer => Some(Route::Installer),
        _ => source.update_steps().map(Route::Steps),
    }
}

/// Updates this copy by `route`, then checks the `luxe` a shell finds now.
///
/// Windows won't let a running program's file be overwritten, and `luxe update` is itself a
/// running luxe. It does allow a rename, so the running copy steps aside first and the owner
/// writes a new file where it was.
async fn update_with(route: Route, latest: &str) -> anyhow::Result<()> {
    let exe = std::env::current_exe().context("could not find the running luxe")?;
    let aside = match cfg!(windows) {
        true => Some(aside::MovedAside::move_running(&exe)?),
        false => None,
    };
    let ran = match &route {
        Route::Steps(steps) => run_steps(steps).await,
        Route::Installer => installer::run(latest, &exe).await,
    };
    if let Some(aside) = &aside {
        aside.restore_if_missing()?;
    }
    ran?;
    if route == Route::Installer {
        installer::remove_legacy_updater(&exe);
    }
    report(installed_now(latest, &exe).await)
}

/// What to tell a reader luxe will not run an update for: they asked only to check, or no owner's
/// command is known.
fn instructions(source: InstallSource, check_only: bool) -> String {
    match source.update_command() {
        Some(_) if check_only => format!("Run `luxe update`, or {}.", source.advice()),
        _ => format!("To update: {}.", source.advice()),
    }
}

/// Runs each step in turn, the terminal attached, stopping at the first that fails.
async fn run_steps(steps: &[&[&str]]) -> anyhow::Result<()> {
    for step in steps {
        let Some((program, args)) = step.split_first() else { continue };
        let line = step.join(" ");
        println!("Running: {line}");
        let status = tokio::process::Command::new(program)
            .args(args)
            .status()
            .await
            .with_context(|| format!("could not start `{program}`"))?;
        if !status.success() {
            bail!("`{line}` failed ({status}), so luxe is unchanged");
        }
    }
    Ok(())
}

/// How the update landed, read from the `luxe` a shell would run now.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// It reports the new version
    Updated,
    /// `PATH` finds another copy first: the update went to one file and the shell runs another
    Shadowed { found: PathBuf, updated: PathBuf },
    /// The same copy still reports an older version: the package did not have the release yet
    NotYet { reported: String },
    /// No `luxe` on `PATH`, or none that says its version
    Unknown,
}

/// Judges the result from what was found on `PATH`, the copy that ran the update, what the found
/// one reports, and the version the update was for.
fn outcome(found: Option<&Path>, updated: &Path, reported: Option<&str>, latest: &str) -> Outcome {
    let Some(found) = found else { return Outcome::Unknown };
    if reported.is_some_and(|reported| !update::is_newer(latest, reported)) {
        return Outcome::Updated;
    }
    let same = |a: &Path, b: &Path| {
        a.canonicalize().ok().zip(b.canonicalize().ok()).is_some_and(|(a, b)| a == b)
    };
    if !same(found, updated) {
        return Outcome::Shadowed { found: found.to_path_buf(), updated: updated.to_path_buf() };
    }
    match reported {
        Some(reported) => Outcome::NotYet { reported: reported.to_string() },
        None => Outcome::Unknown,
    }
}

/// The `luxe` a shell would run now, and the outcome it shows. `updated` is where the running copy
/// was, read before a Windows update moved it aside.
async fn installed_now(latest: &str, updated: &Path) -> Outcome {
    let found = luxe_core::program::resolve("luxe");
    let reported = match &found {
        Some(path) => version_of(path).await,
        None => None,
    };
    outcome(found.as_deref(), updated, reported.as_deref(), latest)
}

/// The version `path --version` prints, as `luxe 0.2.0`.
async fn version_of(path: &Path) -> Option<String> {
    let output = tokio::process::Command::new(path).arg("--version").output().await.ok()?;
    parse_version(&String::from_utf8_lossy(&output.stdout))
}

/// The version in `--version` output: its last word, when that is semver.
fn parse_version(output: &str) -> Option<String> {
    let word = update::normalize_version(output.split_whitespace().last()?);
    semver::Version::parse(word).is_ok().then(|| word.to_string())
}

/// Says how it went; an update that did not take effect is an error, so a script can tell.
fn report(outcome: Outcome) -> anyhow::Result<()> {
    match outcome {
        Outcome::Updated => println!("Updated. Start the new version with `luxe`."),
        Outcome::Unknown => println!("The update finished. `luxe --version` will confirm it."),
        Outcome::Shadowed { found, updated } => bail!(
            "the update went to {}, but `luxe` on your PATH is {}; remove that copy, or put the \
             updated one's directory first on PATH",
            updated.display(),
            found.display()
        ),
        Outcome::NotYet { reported } => bail!(
            "the update finished, but `luxe --version` still says {reported}; the new release may \
             not have reached your package manager yet, so try again in a while"
        ),
    }
    Ok(())
}

mod aside;
mod installer;

#[cfg(test)]
mod tests;
