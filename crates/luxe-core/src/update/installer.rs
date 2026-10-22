//! Installer updates — what `luxe update` needs to know to rerun luxe's own installer script.
//!
//! dist's installer writes a receipt naming where it installed luxe. `luxe update` reads it to tell
//! an installer copy from any other, then downloads the release's installer script and runs it
//! over the same directory. The script does the writing, exactly as on the first install; this
//! module only decides which script and which directory, and the request and the process live in
//! `luxe-cli`.

use serde::Deserialize;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// dist's name for the app, which names the receipt, its folder and the installer's variables.
const APP_NAME: &str = "luxe-cli";

/// The prefix of every variable the installer script reads for this app (`LUXE_CLI_INSTALL_DIR`).
const APP_VARIABLE_PREFIX: &str = "LUXE_CLI";

/// The updater older installer scripts put beside the binary, named `<package>-update` by dist.
///
/// Releases no longer ship it, since `luxe update` does its job. A copy installed before that
/// still has one, which marks it as an installer copy and is removed once an update succeeds.
pub const LEGACY_UPDATER: &str = "luxe-cli-update";

/// What dist's installer recorded about an install: where it went and whether it edited `PATH`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct InstallReceipt {
    /// The directory the installer installed into
    pub install_prefix: PathBuf,
    /// Whether the installer added that directory to `PATH`
    #[serde(default = "modify_path_default")]
    pub modify_path: bool,
}

/// dist's own default: a receipt from before the field existed came from an installer that edited
/// `PATH`.
fn modify_path_default() -> bool {
    true
}

impl InstallReceipt {
    /// Reads the receipt from where the installer writes it, or `None` when there is none or it
    /// does not parse. Either way luxe was not installed by the script, as far as it can tell.
    pub fn load() -> Option<Self> {
        let path = receipt_path()?;
        let text = std::fs::read_to_string(&path).ok()?;
        Self::parse(&text)
    }

    /// Parses a receipt's JSON.
    pub fn parse(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }

    /// Whether this receipt describes `binary`: it sits in the prefix, or in the prefix's `bin`
    /// for an install that used Cargo's layout.
    pub fn covers(&self, binary: &Path) -> bool {
        let Some(dir) = binary.parent() else { return false };
        let prefix = &self.install_prefix;
        same_dir(dir, prefix) || same_dir(dir, &prefix.join("bin"))
    }
}

/// Where the installer writes its receipt: `luxe-cli-receipt.json` in the folder
/// [`crate::user_dirs::installer_receipt_dir`] names.
pub fn receipt_path() -> Option<PathBuf> {
    let dir = crate::user_dirs::installer_receipt_dir(APP_NAME)?;
    Some(dir.join(format!("{APP_NAME}-receipt.json")))
}

/// Whether two paths are one directory, compared as the file system resolves them when it can,
/// so a symlinked or differently-cased spelling of the prefix still matches.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Where an installer update puts the new copy, and whether the script may touch `PATH`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallTarget {
    /// The directory to install into, handed to the script as its install directory
    pub dir: PathBuf,
    /// Whether the script may add `dir` to `PATH`
    pub modify_path: bool,
}

impl InstallTarget {
    /// The receipt's answer when it describes `binary`, so the script installs where it did last
    /// time. Without one (a copy only the legacy updater marks), the binary's own directory, with
    /// `PATH` left as the first install set it up.
    pub fn for_binary(binary: &Path, receipt: Option<&InstallReceipt>) -> Option<Self> {
        match receipt.filter(|receipt| receipt.covers(binary)) {
            Some(receipt) => {
                Some(Self { dir: receipt.install_prefix.clone(), modify_path: receipt.modify_path })
            }
            None => Some(Self { dir: binary.parent()?.to_path_buf(), modify_path: false }),
        }
    }

    /// The variables that point the installer script at this target. The script's own
    /// "disable updates" switch is never set, because it also stops the script writing the receipt
    /// the next update reads.
    pub fn script_variables(&self) -> Vec<(String, OsString)> {
        let mut variables =
            vec![(format!("{APP_VARIABLE_PREFIX}_INSTALL_DIR"), self.dir.clone().into_os_string())];
        if !self.modify_path {
            variables.push((format!("{APP_VARIABLE_PREFIX}_NO_MODIFY_PATH"), "1".into()));
        }
        variables
    }
}

/// The installer script's file name on the release: PowerShell on Windows, `sh` elsewhere.
pub fn installer_script_name() -> String {
    let extension = if cfg!(windows) { "ps1" } else { "sh" };
    format!("{APP_NAME}-installer.{extension}")
}

/// Where the installer script for `version` is published, as the install instructions name it.
pub fn installer_script_url(version: &str) -> String {
    let version = super::normalize_version(version);
    format!("{}/download/v{version}/{}", super::RELEASES_PAGE, installer_script_name())
}

#[cfg(test)]
mod tests;
