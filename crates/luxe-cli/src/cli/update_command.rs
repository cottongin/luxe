//! `luxe update` — run the update this install's owner provides, then check that it took.
//!
//! The policy is [`luxe_core::update`]'s: which owner installed this copy, which commands update
//! it, and a refusal where Luxe cannot know them. This runs those commands one program at a time
//! with the terminal attached, so the reader sees the package manager's own output, and Luxe never
//! writes a binary itself. `/update` on the dashboard comes here too, once the TUI has handed the
//! terminal back.

use anyhow::{anyhow, bail, Context};
use luxe_core::update::{self, InstallSource};
use std::path::{Path, PathBuf};

/// Checks for a newer release now and, unless `check_only`, runs the update and checks the result.
pub(crate) async fn run(check_only: bool) -> anyhow::Result<()> {
    let current = luxe_core::version::VERSION;
    let latest = crate::update_check::check_now()
        .await
        .map_err(|error| anyhow!("could not check for a newer release: {error}"))?;
    if !update::is_newer(&latest, current) {
        println!("Luxe {current} is the newest release.");
        return Ok(());
    }
    println!("Luxe {latest} is available (you have {current}).");
    let source = InstallSource::detect();
    match (source.update_steps(), check_only) {
        (Some(steps), false) if !cfg!(windows) => {
            run_steps(steps).await?;
            report(installed_now(&latest).await)
        }
        _ => {
            println!("{}", instructions(source, check_only, cfg!(windows)));
            Ok(())
        }
    }
}

/// What to tell a reader Luxe will not run an update for: they asked only to check, Windows cannot
/// replace the running program, or no owner's command is known.
fn instructions(source: InstallSource, check_only: bool, windows: bool) -> String {
    match source.update_command() {
        Some(command) if !check_only && windows => format!(
            "Windows can't replace Luxe while it runs, so run this once it has exited: {command}"
        ),
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
            bail!("`{line}` failed ({status}), so Luxe is unchanged");
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

/// The `luxe` a shell would run now, and the outcome it shows.
async fn installed_now(latest: &str) -> Outcome {
    let found = luxe_core::program::resolve("luxe");
    let reported = match &found {
        Some(path) => version_of(path).await,
        None => None,
    };
    let updated = std::env::current_exe().unwrap_or_default();
    outcome(found.as_deref(), &updated, reported.as_deref(), latest)
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

#[cfg(test)]
mod tests;
