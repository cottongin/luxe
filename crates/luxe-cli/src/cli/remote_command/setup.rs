//! `luxe remote setup` — from nothing to a paired phone, asking before each step that changes
//! something outside Luxe.
//!
//! Four steps: Tailscale is there and signed in; Serve carries `https://<this machine>/` to the
//! bridge; a user service runs the bridge at login; and an invitation to scan. Each step that is
//! already done says so and moves on, so running setup again is how to check it.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;

use luxe_remote::service::Launch;
use luxe_remote::{DeviceBook, Published, Tailnet, TailnetError};

use crate::output::{out, outln};

/// Runs every step in order. Only a missing or signed-out Tailscale stops it.
pub(super) async fn setup(
    port: u16,
    allow: Vec<String>,
    devices: &DeviceBook,
) -> anyhow::Result<()> {
    let tailnet = check_tailscale().await?;
    publish_step(port, &tailnet).await;
    service_step(port, &tailnet, allow);
    wait_for_bridge(port).await;
    outln!();
    super::invitation::show(port, devices, &tailnet)
}

/// Gives a service that just started up to two seconds to bind its port.
async fn wait_for_bridge(port: u16) {
    for _ in 0..10 {
        if super::invitation::bridge_listening(port) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

async fn check_tailscale() -> anyhow::Result<Tailnet> {
    match luxe_remote::this_machine().await {
        Ok(tailnet) => {
            outln!("Tailscale: signed in as {} on {}.", tailnet.login, tailnet.dns_name);
            Ok(tailnet)
        }
        Err(error) => {
            outln!("{}", tailscale_advice(&error));
            Err(error.into())
        }
    }
}

fn tailscale_advice(error: &TailnetError) -> &'static str {
    match error {
        TailnetError::NotInstalled => {
            "Install Tailscale from https://tailscale.com/download and sign in, then run \
             `luxe remote setup` again."
        }
        TailnetError::NotRunning(_) => {
            "Sign in to Tailscale with `tailscale up`, then run `luxe remote setup` again."
        }
        TailnetError::Failed(_) | TailnetError::Unreadable(_) => {
            "Check that Tailscale is running with `tailscale status`, then run `luxe remote setup` \
             again."
        }
    }
}

async fn publish_step(port: u16, tailnet: &Tailnet) {
    let address = format!("https://{}/", tailnet.dns_name);
    match luxe_remote::published(port).await {
        Ok(Published::Here) => outln!("Serve: {address} leads to the bridge."),
        Ok(Published::Elsewhere(target)) => outln!(
            "Serve: {address} already leads to {target}, so setup leaves it alone. \
             `tailscale serve --bg {port}` would point it at the bridge instead."
        ),
        Ok(Published::Nowhere) => offer_publish(port, &address).await,
        Err(error) => outln!("Serve: couldn't read its settings ({error})."),
    }
}

async fn offer_publish(port: u16, address: &str) {
    let question = format!("Publish the bridge at {address}, for your tailnet only?");
    if !confirm(&question) {
        outln!("Skipped. `tailscale serve --bg {port}` publishes it when you're ready.");
        return;
    }
    match luxe_remote::publish(port).await {
        Ok(()) => outln!("Serve: {address} now leads to the bridge."),
        Err(error) => {
            outln!("Serve: {error}");
            outln!("{}", SERVE_HINT);
        }
    }
}

/// What usually stops `tailscale serve` for an ordinary account.
const SERVE_HINT: &str = if cfg!(target_os = "linux") {
    "If it said access denied, let your account manage Tailscale with \
     `sudo tailscale set --operator=$USER`, then run setup again."
} else {
    "Fix what Tailscale said above, then run setup again."
};

fn service_step(port: u16, tailnet: &Tailnet, allow: Vec<String>) {
    if luxe_remote::service::installed() {
        outln!("Service: installed, so the bridge runs whenever you're signed in.");
        return;
    }
    if !confirm("Run the bridge whenever you're signed in, as a user service?") {
        outln!("Skipped. Run `luxe remote` yourself whenever your devices should reach Luxe.");
        return;
    }
    let launch =
        Launch { program: luxe_program(), args: service_args(port, &tailnet.login, allow) };
    match luxe_remote::service::install(&launch) {
        Ok(recorded) => outln!("Service: installed ({recorded}) and started."),
        Err(error) => outln!("Service: {error}"),
    }
}

/// The bridge's flags as the service runs it, the owner named outright so the service doesn't
/// depend on reaching Tailscale as it starts.
fn service_args(port: u16, owner: &str, allow: Vec<String>) -> Vec<String> {
    let mut args = vec!["remote".into(), "--port".into(), port.to_string()];
    for login in std::iter::once(owner.to_string()).chain(allow) {
        args.extend(["--allow".into(), login]);
    }
    args
}

/// This binary, by the path on `PATH` when that is the same file, since an upgrade replaces the
/// file a package manager links from `PATH` and leaves a versioned path behind.
fn luxe_program() -> PathBuf {
    let running = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("luxe"));
    let on_path = luxe_core::program::resolve("luxe");
    let same = |path: &PathBuf| path.canonicalize().ok() == running.canonicalize().ok();
    on_path.filter(same).unwrap_or(running)
}

/// Asks a yes-or-no question, yes by default. Without a terminal to ask, the answer is no.
fn confirm(question: &str) -> bool {
    if !std::io::stdin().is_terminal() {
        outln!("{question} Not asked, with no terminal to answer in.");
        return false;
    }
    out!("{question} [Y/n] ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    if std::io::stdin().lock().read_line(&mut answer).is_err() {
        return false;
    }
    let answer = answer.trim().to_lowercase();
    answer.is_empty() || answer.starts_with('y')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The service names the owner and every `--allow`, on the port setup was given.
    ///
    /// The break this catches: a service that relies on finding Tailscale as it starts, which
    /// launchd's bare `PATH` doesn't, so the bridge comes up letting nobody in.
    #[test]
    fn the_service_names_who_it_lets_in() {
        let args = service_args(4600, "alex@github", vec!["sam@github".into()]);
        let expected =
            ["remote", "--port", "4600", "--allow", "alex@github", "--allow", "sam@github"];
        assert_eq!(args, expected);
    }
}
