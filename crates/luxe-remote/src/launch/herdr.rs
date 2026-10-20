//! Herdr — opening a pane that runs `luxe`, through herdr's CLI.
//!
//! Only when herdr's server is already running. The CLI never starts one, and a server the bridge
//! started would hold panes nobody is attached to. `HERDR_SESSION` picks the session, as it does
//! for any herdr command.

use std::path::Path;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Deserialize;

use super::LaunchError;

/// How long one herdr command gets; each answers from the running server at once.
const ASK_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether sessions may be started in herdr.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Herdr {
    /// When herdr's server is running
    WhenRunning,
    /// Never, so tests don't open panes in the developer's herdr
    Off,
}

#[derive(Deserialize)]
struct ServerStatus {
    running: bool,
    #[serde(default)]
    compatible: Option<bool>,
}

#[derive(Deserialize)]
struct Reply<T> {
    result: T,
}

#[derive(Deserialize)]
struct Created {
    root_pane: Pane,
}

#[derive(Deserialize)]
struct PaneList {
    panes: Vec<Pane>,
}

/// One herdr pane: the terminal a process runs in.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Pane {
    pane_id: String,
    workspace_id: String,
    /// The folder its shell is in, empty when herdr doesn't know
    #[serde(default)]
    pub(crate) cwd: String,
}

#[derive(Deserialize)]
struct Refusal {
    error: RefusalReason,
}

#[derive(Deserialize)]
struct RefusalReason {
    message: String,
}

impl Herdr {
    /// Whether herdr's server is up and speaks this CLI's protocol.
    pub(crate) async fn running(self) -> bool {
        if self == Herdr::Off {
            return false;
        }
        ask(&["status", "server", "--json"]).await.is_ok_and(|json| is_running(&json))
    }

    /// Opens a pane in `folder` and runs `luxe` in it: a new tab in the workspace already working
    /// there, or a new workspace named after the folder.
    pub(crate) async fn open(self, folder: &Path) -> Result<(), LaunchError> {
        let shown = folder.to_string_lossy();
        let created = match workspace_for(&self.panes().await?, folder) {
            Some(workspace) => {
                ask(&["tab", "create", "--workspace", workspace, "--cwd", &shown, "--no-focus"])
                    .await?
            }
            None => {
                let label = luxe_core::paths::dir_name(folder);
                ask(&["workspace", "create", "--cwd", &shown, "--label", &label, "--no-focus"])
                    .await?
            }
        };
        let pane = parse::<Created>(&created)?.root_pane.pane_id;
        // the `luxe` on PATH, as the person would type it in that pane
        ask(&["pane", "run", &pane, "luxe"]).await?;
        Ok(())
    }

    /// Every pane in the session, for the folders they're in.
    pub(crate) async fn panes(self) -> Result<Vec<Pane>, LaunchError> {
        if self == Herdr::Off {
            return Ok(Vec::new());
        }
        Ok(parse::<PaneList>(&ask(&["pane", "list"]).await?)?.panes)
    }
}

fn is_running(json: &str) -> bool {
    serde_json::from_str::<ServerStatus>(json)
        .is_ok_and(|status| status.running && status.compatible != Some(false))
}

/// The workspace with a pane in `folder`, where a new tab joins the work already there.
fn workspace_for<'a>(panes: &'a [Pane], folder: &Path) -> Option<&'a str> {
    panes.iter().find(|pane| Path::new(&pane.cwd) == folder).map(|pane| pane.workspace_id.as_str())
}

fn parse<T: DeserializeOwned>(json: &str) -> Result<T, LaunchError> {
    let reply: Reply<T> = serde_json::from_str(json).map_err(|error| {
        LaunchError::Herdr(format!("it said something this can't read: {error}"))
    })?;
    Ok(reply.result)
}

/// What `herdr <args>` prints, or why it failed in herdr's own words.
async fn ask(args: &[&str]) -> Result<String, LaunchError> {
    let program = luxe_core::program::resolve("herdr")
        .ok_or_else(|| LaunchError::Herdr("the `herdr` command isn't on PATH".into()))?;
    let asked = tokio::process::Command::new(program).args(args).kill_on_drop(true).output();
    let output = tokio::time::timeout(ASK_TIMEOUT, asked)
        .await
        .map_err(|_| LaunchError::Herdr("it didn't answer within 10 seconds".into()))?
        .map_err(|error| LaunchError::Herdr(error.to_string()))?;
    if !output.status.success() {
        return Err(LaunchError::Herdr(refusal(&String::from_utf8_lossy(&output.stderr))));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// herdr's reason from the JSON it prints on a failure, or the text itself when it isn't JSON.
fn refusal(stderr: &str) -> String {
    match serde_json::from_str::<Refusal>(stderr) {
        Ok(refusal) => refusal.error.message,
        Err(_) => stderr.trim().to_string(),
    }
}

#[cfg(test)]
mod tests;
