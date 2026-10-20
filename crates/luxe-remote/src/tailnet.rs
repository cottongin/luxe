//! Tailnet — what Tailscale says about this machine: who owns it, and its name on the tailnet.
//!
//! Read from `tailscale status --json`, the CLI's stable machine-readable output, rather than the
//! local API's socket, whose path and permissions differ on every platform.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;

/// How long `tailscale status` gets; it answers from the running daemon in milliseconds.
const STATUS_TIMEOUT: Duration = Duration::from_secs(10);

/// This machine on its tailnet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tailnet {
    /// The login of the machine's owner, as Serve names it in `Tailscale-User-Login`
    pub login: String,
    /// The machine's name on the tailnet, such as `desk.tail1234.ts.net`
    pub dns_name: String,
}

impl Tailnet {
    /// The machine's own name, the first label of [`Tailnet::dns_name`]: `desk` for
    /// `desk.tail1234.ts.net`, as the tailnet's admin console and its addresses name it.
    pub fn machine(&self) -> &str {
        self.dns_name.split('.').next().unwrap_or(&self.dns_name)
    }
}

/// Why Tailscale couldn't say.
#[derive(Debug, thiserror::Error)]
pub enum TailnetError {
    /// No `tailscale` on `PATH`
    #[error("the `tailscale` command isn't installed or isn't on PATH")]
    NotInstalled,
    /// It ran but failed
    #[error("`tailscale` failed: {0}")]
    Failed(String),
    /// It answered, but not with what this reads
    #[error("`tailscale` said something this can't read: {0}")]
    Unreadable(String),
    /// Tailscale is installed but not connected
    #[error("Tailscale isn't connected (it reports {0}); run `tailscale up`")]
    NotRunning(String),
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Status {
    backend_state: String,
    #[serde(rename = "Self")]
    this_machine: Option<Node>,
    #[serde(default)]
    user: HashMap<String, User>,
}

#[derive(Deserialize)]
struct Node {
    #[serde(rename = "UserID")]
    user_id: u64,
    #[serde(rename = "DNSName")]
    dns_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct User {
    login_name: String,
}

/// Asks Tailscale who owns this machine and what it is called.
pub async fn this_machine() -> Result<Tailnet, TailnetError> {
    parse_status(&ask_tailscale(&["status", "--json"]).await?)
}

/// What `tailscale <args>` prints, for a query that answers at once.
async fn ask_tailscale(args: &[&str]) -> Result<String, TailnetError> {
    let program = luxe_core::program::resolve("tailscale").ok_or(TailnetError::NotInstalled)?;
    let asked = tokio::process::Command::new(program).args(args).kill_on_drop(true).output();
    let output = tokio::time::timeout(STATUS_TIMEOUT, asked)
        .await
        .map_err(|_| TailnetError::Failed("it didn't answer within 10 seconds".into()))?
        .map_err(|error| TailnetError::Failed(error.to_string()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(TailnetError::Failed(format!("{} ({stderr})", output.status)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn parse_status(json: &str) -> Result<Tailnet, TailnetError> {
    let status: Status =
        serde_json::from_str(json).map_err(|error| TailnetError::Unreadable(error.to_string()))?;
    if status.backend_state != "Running" {
        return Err(TailnetError::NotRunning(status.backend_state));
    }
    let node = status
        .this_machine
        .ok_or_else(|| TailnetError::Unreadable("it names no machine as this one".into()))?;
    let owner = status.user.get(&node.user_id.to_string()).ok_or_else(|| {
        TailnetError::Unreadable(format!(
            "this machine's owner, user {}, isn't listed",
            node.user_id
        ))
    })?;
    let dns_name = node.dns_name.trim_end_matches('.').to_string();
    Ok(Tailnet { login: owner.login_name.clone(), dns_name })
}

mod serve;

pub use serve::{publish, published, Published};

#[cfg(test)]
mod tests;
