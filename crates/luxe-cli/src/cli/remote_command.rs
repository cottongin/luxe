//! `luxe remote` — the bridge that serves this machine's sessions to your own devices.
//!
//! The serving is `luxe_remote`'s; this is the terminal around it: who is let in, where it
//! listens, setting it up, pairing and revoking devices, and stopping on Ctrl-C.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use luxe_remote::{Access, DeviceBook, Tailnet};

use super::args::{DevicesCommand, RemoteArgs, RemoteCommand, ServiceCommand};
use crate::output::outln;

mod invitation;
mod render;
mod setup;

/// Runs `luxe remote` and its subcommands.
pub(crate) async fn run_remote_command(args: RemoteArgs) -> anyhow::Result<()> {
    let devices = DeviceBook::at(DeviceBook::default_path());
    match args.command {
        None => serve(args.port, args.allow, devices).await,
        Some(RemoteCommand::Setup) => setup::setup(args.port, args.allow, &devices).await,
        Some(RemoteCommand::Pair) => invitation::pair(args.port, &devices).await,
        Some(RemoteCommand::Devices { command: None }) => list_devices(&devices),
        Some(RemoteCommand::Devices { command: Some(DevicesCommand::Revoke { id }) }) => {
            revoke(&devices, &id)
        }
        Some(RemoteCommand::Service { command: ServiceCommand::Remove }) => remove_service(),
    }
}

fn remove_service() -> anyhow::Result<()> {
    match luxe_remote::service::remove()? {
        true => outln!("The bridge's service is stopped and won't start at login."),
        false => outln!("No service for the bridge is installed."),
    }
    Ok(())
}

/// Serves until Ctrl-C.
async fn serve(port: u16, allow: Vec<String>, devices: DeviceBook) -> anyhow::Result<()> {
    let tailnet = luxe_remote::this_machine().await;
    let machine = tailnet.as_ref().ok().map(|tailnet| tailnet.machine().to_owned());
    let logins = allowed_logins(tailnet, allow)?;
    let access = Access { logins: logins.clone(), devices };
    let bridge = bridge(port, access, machine).await?;
    outln!("Luxe remote is listening on http://{}. Ctrl-C stops it.", bridge.address());
    outln!("It lets in {}, from a paired device.", logins.join(", "));
    bridge
        .serve(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

/// The bridge `luxe remote` runs: starting sessions with this luxe, drawing them as the TUI does,
/// and named for the computer when Tailscale said what it's called.
async fn bridge(
    port: u16,
    access: Access,
    machine: Option<String>,
) -> anyhow::Result<luxe_remote::Bridge> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let runtime_dir = luxe_core::user_dirs::luxe_runtime_dir();
    let luxe = std::env::current_exe()
        .map_err(|error| anyhow::anyhow!("could not find this luxe to start sessions: {error}"))?;
    let bridge = luxe_remote::Bridge::bind(address, runtime_dir, access).await?;
    let bridge = bridge.starting_sessions(luxe);
    let bridge = bridge.rendering_with(std::sync::Arc::new(render::TuiRenderer));
    Ok(match machine {
        Some(machine) => bridge.named(machine),
        None => bridge,
    })
}

/// The owner, as Tailscale reports them, plus any `--allow`ed logins. Without the owner, only an
/// explicit `--allow` lets anyone in: guessing would mean an allowlist nobody chose.
fn allowed_logins(
    owner: Result<Tailnet, luxe_remote::TailnetError>,
    mut allow: Vec<String>,
) -> anyhow::Result<Vec<String>> {
    match owner {
        Ok(tailnet) => allow.insert(0, tailnet.login),
        Err(error) if allow.is_empty() => {
            return Err(error).context(
                "couldn't learn this machine's tailnet login; pass `--allow <login>` to name who \
                 may use the bridge",
            );
        }
        Err(error) => tracing::warn!(%error, "serving only the --allow logins"),
    }
    let mut seen = std::collections::HashSet::new();
    allow.retain(|login| seen.insert(login.clone()));
    Ok(allow)
}

fn list_devices(devices: &DeviceBook) -> anyhow::Result<()> {
    let paired = devices.devices();
    if paired.is_empty() {
        outln!("No devices are paired. Pair one with `luxe remote pair`.");
        return Ok(());
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
    for device in paired {
        let age = age(now.saturating_sub(device.paired_at));
        outln!("  {}  {}  ({}, paired {age})", device.id, device.name, device.login);
    }
    outln!("\nUnpair one with `luxe remote devices revoke <id>`.");
    Ok(())
}

fn revoke(devices: &DeviceBook, id: &str) -> anyhow::Result<()> {
    match devices.revoke(id)? {
        Some(device) => outln!("Unpaired {} ({id}); its open sessions close too.", device.name),
        None => anyhow::bail!("no paired device has the id {id}; see `luxe remote devices`"),
    }
    Ok(())
}

/// How long ago, in the largest whole unit that fits.
fn age(seconds: u64) -> String {
    const DAY: u64 = 24 * 60 * 60;
    match seconds / DAY {
        0 => "today".to_string(),
        1 => "yesterday".to_string(),
        days => format!("{days} days ago"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The owner always leads the allowlist, and nobody is let in when no one was named.
    ///
    /// The break this catches: serving an empty allowlist when Tailscale can't be asked (which
    /// shuts everyone out with no reason given), or dropping the owner once `--allow` is used.
    #[test]
    fn the_allowlist_is_the_owner_and_whoever_is_named() {
        let owner = || Ok(Tailnet { login: "alex@github".into(), dns_name: "desk".into() });
        let missing = || Err(luxe_remote::TailnetError::NotInstalled);
        let both = allowed_logins(owner(), vec!["sam@github".into()]).unwrap();
        assert_eq!(both, ["alex@github", "sam@github"]);
        let repeated = allowed_logins(owner(), vec!["alex@github".into()]).unwrap();
        assert_eq!(repeated, ["alex@github"]);
        assert_eq!(allowed_logins(missing(), vec!["sam@github".into()]).unwrap(), ["sam@github"]);
        assert!(allowed_logins(missing(), vec![]).is_err());
    }
}
