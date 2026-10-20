//! Serve — whether Tailscale Serve already carries the bridge to the tailnet, and asking it to.
//!
//! Read from `tailscale serve status --json`, which prints Tailscale's `ServeConfig`: a `Web` map
//! from `host:port` to the handlers on each path, and a `Foreground` map holding the same shape for
//! each `tailscale serve` running in a terminal rather than in the background.

use serde_json::Value;

use super::{ask_tailscale, TailnetError};

/// What `https://<this machine>/` leads to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Published {
    /// The bridge on this port
    Here,
    /// Something else, which setup leaves alone
    Elsewhere(String),
    /// Nothing yet
    Nowhere,
}

/// What Serve does with `https://<this machine>/`, judged against the bridge's `port`.
pub async fn published(port: u16) -> Result<Published, TailnetError> {
    let status = ask_tailscale(&["serve", "status", "--json"]).await?;
    let config: Value = serde_json::from_str(&status)
        .map_err(|error| TailnetError::Unreadable(error.to_string()))?;
    Ok(judge(&config, port))
}

/// Asks Serve to carry `https://<this machine>/` to the bridge, in the background so it outlasts
/// the terminal. The output is the user's: Tailscale may print a link to turn HTTPS on for the
/// tailnet and wait while they do, so there is no timeout.
pub async fn publish(port: u16) -> Result<(), TailnetError> {
    let program = luxe_core::program::resolve("tailscale").ok_or(TailnetError::NotInstalled)?;
    let status = tokio::process::Command::new(program)
        .args(["serve", "--bg", &target(port)])
        .kill_on_drop(true)
        .status()
        .await
        .map_err(|error| TailnetError::Failed(error.to_string()))?;
    if !status.success() {
        return Err(TailnetError::Failed(format!("serve ended with {status}")));
    }
    Ok(())
}

/// The address Serve proxies to. IPv4 because the bridge binds `127.0.0.1`, where `localhost`
/// could resolve to `::1` first.
fn target(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

fn judge(config: &Value, port: u16) -> Published {
    let roots = root_handlers(config);
    if roots.iter().any(|handler| proxies_to(handler, port)) {
        return Published::Here;
    }
    match roots.first() {
        Some(handler) => Published::Elsewhere(describe(handler)),
        None => Published::Nowhere,
    }
}

/// The handlers for `/` on port 443, in the background config and every foreground one.
fn root_handlers(config: &Value) -> Vec<&Value> {
    let foreground = config.get("Foreground").and_then(Value::as_object);
    let configs =
        std::iter::once(config).chain(foreground.into_iter().flat_map(|map| map.values()));
    configs
        .filter_map(|config| config.get("Web")?.as_object())
        .flat_map(|web| web.iter())
        .filter(|(host_port, _)| host_port.ends_with(":443"))
        .filter_map(|(_, site)| site.get("Handlers")?.get("/"))
        .collect()
}

fn proxies_to(handler: &Value, port: u16) -> bool {
    let Some(proxy) = handler.get("Proxy").and_then(Value::as_str) else {
        return false;
    };
    let authority = proxy.strip_prefix("http://").unwrap_or(proxy).trim_end_matches('/');
    let Some((host, listening)) = authority.rsplit_once(':') else {
        return false;
    };
    let loopback = matches!(host, "127.0.0.1" | "localhost" | "[::1]");
    loopback && listening.parse::<u16>().is_ok_and(|listening| listening == port)
}

fn describe(handler: &Value) -> String {
    ["Proxy", "Path", "Text"]
        .iter()
        .find_map(|kind| handler.get(*kind).and_then(Value::as_str))
        .unwrap_or("another handler")
        .to_string()
}

#[cfg(test)]
mod tests;
