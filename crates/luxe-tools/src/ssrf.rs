//! SSRF guard for outbound web fetches.
//!
//! An agent can be steered — by a prompt, or by a page it just fetched — into requesting a URL the
//! user never meant it to reach. Before every fetch we require http/https, resolve the host, and
//! classify the address; redirects are followed manually so **each hop** is re-validated, and the
//! connection is pinned to the validated IP to close the DNS-rebinding window.
//!
//! # What the guard is actually for
//!
//! The addresses split into two groups, and the guard treats them differently:
//!
//! - **Somewhere else** — cloud metadata (`169.254.169.254` and its IPv6 form), private LAN ranges,
//!   CGNAT, ULA. Reaching these is lateral movement: credentials the user never handed over, or a
//!   machine that merely shares their network. Denied, and a much stronger claim than the tool's
//!   convenience.
//! - **This machine** — loopback. luxe is a coding agent with an unrestricted `bash`, so
//!   `curl http://127.0.0.1:4321` is already one tool call away and permitted by default. Refusing
//!   the same origin to `web_fetch` and `screenshot` grants no protection whatsoever; it only breaks
//!   the thing `screenshot mode=url` exists to do, and the model works around it with `bash` and a
//!   headless browser.
//!
//! So loopback is [`AddressClass::Loopback`] and allowed, and everything non-public is
//! [`AddressClass::Blocked`] and refused. `file://` stays refused separately: a local file read is
//! not a network fetch, and `read` already does it with the project's own path rules.
//!
//! `LUXE_WEB_ALLOW_PRIVATE=1` remains the coarse opt-out for someone who intentionally fetches a LAN
//! host or a TUN/fake-IP proxy.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use reqwest::header::LOCATION;
use url::Url;

use crate::web::WebError;

/// Max redirect hops before giving up.
const MAX_REDIRECTS: usize = 6;

/// Default request timeout in seconds.
const DEFAULT_WEB_TIMEOUT_SECS: u64 = 30;

/// How long name resolution gets before the host counts as unreachable.
///
/// The system resolver has its own timeouts and they are not ours: `getaddrinfo` against a
/// misconfigured or blackholed nameserver can sit for tens of seconds, and it happens *outside*
/// reqwest's request timeout, so the per-request bound below never sees it.
const RESOLVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Returns the per-request timeout for outbound web fetches.
///
/// Reads `LUXE_WEB_TIMEOUT_SECS` from the environment. If the variable is unset, not a valid
/// positive integer, or zero, the default of 30 seconds is returned instead.
pub(crate) fn web_timeout() -> std::time::Duration {
    let secs = std::env::var("LUXE_WEB_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .unwrap_or(DEFAULT_WEB_TIMEOUT_SECS);
    std::time::Duration::from_secs(secs)
}

/// Whether the guard is disabled via the explicit opt-out.
pub(crate) fn guard_disabled() -> bool {
    std::env::var("LUXE_WEB_ALLOW_PRIVATE").as_deref() == Ok("1")
}

/// Where an address lives, which is what decides whether reaching it is a problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AddressClass {
    /// Globally routable. Ordinary web traffic.
    Public,
    /// This machine (`127.0.0.0/8`, `::1`, and their IPv4-mapped forms).
    Loopback,
    /// Somewhere else that is not the public internet: cloud metadata, private LAN, CGNAT, ULA,
    /// link-local, and the reserved/documentation ranges.
    Blocked,
}

impl AddressClass {
    /// Whether a fetch under `local` may reach this class of address.
    pub(crate) fn is_reachable(self, local: LocalAccess) -> bool {
        match self {
            Self::Public => true,
            Self::Loopback => local == LocalAccess::Allowed,
            Self::Blocked => false,
        }
    }
}

/// Whether a fetch may reach the machine it is running on.
///
/// A named type rather than a `bool`, because it is the switch that decides what a model-supplied URL
/// can touch, and `safe_get(url, ua, true)` says nothing at the call site about which `true` that is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalAccess {
    /// A local origin is part of this tool's job — a dev server on this machine.
    Allowed,
    /// The public internet only.
    Denied,
}

/// Classify an address: globally routable, this machine, or somewhere it must not reach.
pub(crate) fn classify_ip(ip: IpAddr) -> AddressClass {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        // IPv4-mapped (`::ffff:a.b.c.d`) is the same address wearing a different hat, so classify
        // the address it actually names — otherwise `::ffff:127.0.0.1` slips past as its own case.
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => classify_v4(v4),
            None => classify_v6(v6),
        },
    }
}

fn classify_v4(ip: Ipv4Addr) -> AddressClass {
    if ip.is_loopback() {
        return AddressClass::Loopback;
    }
    if is_public_v4(ip) {
        AddressClass::Public
    } else {
        AddressClass::Blocked
    }
}

fn classify_v6(ip: Ipv6Addr) -> AddressClass {
    if ip.is_loopback() {
        return AddressClass::Loopback;
    }
    // a translated address reaches the IPv4 it carries, so that is the one to judge; even an
    // embedded loopback is someone else's machine by then
    if let Some(v4) = embedded_v4(ip) {
        return match classify_v4(v4) {
            AddressClass::Public => AddressClass::Public,
            _ => AddressClass::Blocked,
        };
    }
    if is_public_v6(ip) {
        AddressClass::Public
    } else {
        AddressClass::Blocked
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    if ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
    {
        return false;
    }
    // 0.0.0.0/8 (this network), CGNAT 100.64/10, benchmarking 198.18/15, reserved 240/4.
    if a == 0
        || (a == 100 && (64..=127).contains(&b))
        || (a == 198 && (b == 18 || b == 19))
        || a >= 240
    {
        return false;
    }
    true
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
        return false;
    }
    // IPv4-mapped (::ffff:a.b.c.d) — validate the embedded v4.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_v4(v4);
    }
    let [first, second, ..] = ip.segments();
    // ULA fc00::/7, link-local fe80::/10, site-local fec0::/10, documentation 2001:db8::/32
    !((0xfc00..=0xfdff).contains(&first)
        || (0xfe80..=0xfeff).contains(&first)
        || (first, second) == (0x2001, 0x0db8))
}

/// The IPv4 address an IPv6 one carries when it is a translation of it: NAT64 (`64:ff9b::/96`),
/// 6to4 (`2002::/16`), Teredo (`2001::/32`, whose client address is stored inverted) and the old
/// IPv4-compatible form (`::a.b.c.d`).
fn embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let v4 = |high: u16, low: u16| Ipv4Addr::from((u32::from(high) << 16) | u32::from(low));
    match ip.segments() {
        [0x64, 0xff9b, 0, 0, 0, 0, high, low] => Some(v4(high, low)),
        [0x2002, high, low, ..] => Some(v4(high, low)),
        [0x2001, 0, _, _, _, _, high, low] => Some(v4(!high, !low)),
        // `::` and `::1` are their own addresses, not IPv4-compatible ones
        [0, 0, 0, 0, 0, 0, high, low] if (high, low) > (0, 1) => Some(v4(high, low)),
        _ => None,
    }
}

/// Validate a URL's scheme + host and resolve it to a set of reachable socket addresses.
///
/// `allow_loopback` admits this machine's own addresses; see [`AddressClass`] for why that is a
/// different question from admitting a private LAN host. Errors if the scheme isn't http/https,
/// resolution fails or times out, or any resolved address is one the caller may not reach.
///
/// Every address is checked, not just the one that gets pinned: a host that resolves to both a public
/// and a blocked address is a rebinding attempt wearing a single DNS answer.
pub(crate) async fn resolve_validated(
    url: &Url,
    local: LocalAccess,
) -> Result<Vec<SocketAddr>, WebError> {
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(WebError::Network(format!(
            "only http/https URLs are allowed (got {})",
            url.scheme()
        )));
    }
    let host = url.host_str().ok_or_else(|| WebError::Network("URL has no host".to_string()))?;
    let lower = host.to_ascii_lowercase();
    // `localhost` (and the reserved `.localhost` suffix) names this machine by definition, so it
    // follows the loopback policy rather than getting a name-based rule of its own. It still has to
    // resolve: a hosts file can point `localhost` anywhere.
    if (lower == "localhost" || lower.ends_with(".localhost")) && local == LocalAccess::Denied {
        return Err(WebError::Network(format!(
            "blocked local host '{host}': this tool does not fetch from the machine it runs on"
        )));
    }
    let port = url.port_or_known_default().unwrap_or(80);
    // An IP literal is already an address, and handing one to the resolver is worse than
    // unnecessary: `host_str` returns an IPv6 literal *with its brackets* (`[::1]`), which
    // `getaddrinfo` has never heard of — so every `http://[::1]:4321/` would fail as a name lookup.
    // That is the address a dual-stack dev server prints.
    let addrs: Vec<SocketAddr> = match url.host() {
        Some(url::Host::Ipv4(ip)) => vec![SocketAddr::new(IpAddr::V4(ip), port)],
        Some(url::Host::Ipv6(ip)) => vec![SocketAddr::new(IpAddr::V6(ip), port)],
        Some(url::Host::Domain(name)) => resolve_host(name, port).await?,
        None => return Err(WebError::Network("URL has no host".to_string())),
    };
    if addrs.is_empty() {
        return Err(WebError::Network(format!("no addresses for '{host}'")));
    }
    for addr in &addrs {
        let class = classify_ip(addr.ip());
        if class.is_reachable(local) {
            continue;
        }
        return Err(match class {
            AddressClass::Loopback => WebError::Network(format!(
                "blocked loopback address {} for '{host}': this tool does not fetch from the \
                 machine it runs on",
                addr.ip()
            )),
            _ => WebError::Network(format!(
                "blocked {} for '{host}': not a public address, and not this machine — private \
                 networks and cloud metadata endpoints are off limits. Set \
                 LUXE_WEB_ALLOW_PRIVATE=1 if you meant to reach it.",
                addr.ip()
            )),
        });
    }
    Ok(addrs)
}

/// Resolve a domain name, bounded by [`RESOLVE_TIMEOUT`].
async fn resolve_host(name: &str, port: u16) -> Result<Vec<SocketAddr>, WebError> {
    match tokio::time::timeout(RESOLVE_TIMEOUT, tokio::net::lookup_host((name, port))).await {
        Ok(Ok(addrs)) => Ok(addrs.collect()),
        Ok(Err(e)) => Err(WebError::Network(format!("failed to resolve '{name}': {e}"))),
        Err(_) => Err(WebError::Timeout(format!(
            "resolving '{name}' took longer than {}s",
            RESOLVE_TIMEOUT.as_secs()
        ))),
    }
}

/// SSRF-safe HTTP GET returning the response body. Redirects are followed manually, re-validating
/// each hop and pinning the connection to the validated IP. The opt-out bypasses validation but
/// still fetches (for intentional internal fetches).
///
/// [`web_timeout`] bounds the **whole** fetch, not each hop. Per-hop was the same number multiplied
/// by six, and then doubled again by `web_fetch`'s stealth retry: a host that completed the
/// handshake and then went quiet cost six minutes against a setting that reads as thirty seconds.
pub(crate) async fn safe_get(
    url_str: &str,
    user_agent: &str,
    local: LocalAccess,
) -> Result<Fetched, WebError> {
    follow(url_str, user_agent, local, Hops { origin_local: None, body_cap: MAX_BODY_BYTES }).await
}

/// Most of a body a fetch reads; past it the rest is left unread.
pub(crate) const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

/// A fetched body, and whether it stopped at [`MAX_BODY_BYTES`].
#[derive(Debug)]
pub(crate) struct Fetched {
    /// The body as text, in the charset the server declared
    pub(crate) body: String,
    /// Whether the rest was left unread; a caller says so after it has reduced the page
    pub(crate) truncated: bool,
}

impl Fetched {
    /// The body reduced by `reduce` (to markdown, say), with a note after it when the body was
    /// cut: one written into the HTML might not survive the reduction.
    pub(crate) fn reduced(self, reduce: impl FnOnce(&str) -> String) -> String {
        let mut text = reduce(&self.body);
        if self.truncated {
            let megabytes = MAX_BODY_BYTES / (1024 * 1024);
            text.push_str(&format!(
                "\n\n[truncated: the page passed {megabytes} MB, and the rest was not read]"
            ));
        }
        text
    }
}

/// Whether every address names this machine: the fetch started here, and may come back here.
fn is_local(addrs: &[SocketAddr]) -> bool {
    addrs.iter().all(|addr| classify_ip(addr.ip()) == AddressClass::Loopback)
}

/// Whether a hop would land on this machine though the fetch started elsewhere: SSRF by redirect,
/// since the carve-out for this machine covers a URL the caller asked for, not one a page picked.
/// One loopback address is enough, as the connection may fall back to it.
fn lands_here_from_elsewhere(origin_local: bool, hop: &[SocketAddr]) -> bool {
    !origin_local && hop.iter().any(|addr| classify_ip(addr.ip()) == AddressClass::Loopback)
}

/// What a fetch holds itself to across its hops.
struct Hops {
    /// Whether the first URL is on this machine; `None` works it out from that URL's addresses.
    origin_local: Option<bool>,
    body_cap: usize,
}

async fn follow(
    url_str: &str,
    user_agent: &str,
    local: LocalAccess,
    mut hops: Hops,
) -> Result<Fetched, WebError> {
    let mut current =
        Url::parse(url_str).map_err(|e| WebError::Network(format!("invalid URL: {e}")))?;
    let origin = current.clone();
    let budget = web_timeout();
    let deadline = tokio::time::Instant::now() + budget;

    for hop in 0..MAX_REDIRECTS {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(WebError::Timeout(format!(
                "fetching {current} took longer than {}s",
                budget.as_secs()
            )));
        }
        let mut builder = reqwest::Client::builder()
            .user_agent(user_agent)
            .timeout(remaining)
            .redirect(reqwest::redirect::Policy::none());

        if !guard_disabled() {
            let addrs = resolve_validated(&current, local).await?;
            let origin_local = *hops.origin_local.get_or_insert_with(|| is_local(&addrs));
            if hop > 0 && lands_here_from_elsewhere(origin_local, &addrs) {
                return Err(WebError::Network(format!(
                    "refused a redirect from {origin} to {current}: a page on another host may not \
                     send this fetch to the machine it runs on"
                )));
            }
            // Pin only a *name*: the rebinding window this closes is the gap between our lookup and
            // the client's, and an IP literal never had one. `host_str` also brackets an IPv6
            // literal, which is not a name reqwest could match anyway.
            //
            // Pin the whole validated set, not its first entry. Every address in it passed the same
            // check, and handing over only one throws away the connect fallback a dual-stack host
            // depends on — a name resolving to `::1` and `127.0.0.1` where the server listens on just
            // one of them is the ordinary shape of a local dev server, not an edge case.
            if let Some(url::Host::Domain(host)) = current.host() {
                builder = builder.resolve_to_addrs(host, &addrs);
            }
        }

        let client = builder.build().map_err(|e| WebError::Network(e.to_string()))?;
        // Ask reqwest which kind of failure this was rather than reading its message: a timeout must
        // be typed as one, because it is the failure a retry cannot improve on.
        let response = client.get(current.clone()).send().await.map_err(|e| {
            if e.is_timeout() {
                WebError::Timeout(format!("{current} did not answer within {}s", budget.as_secs()))
            } else {
                WebError::Network(e.to_string())
            }
        })?;
        let status = response.status();

        if status.is_redirection() {
            let location =
                response.headers().get(LOCATION).and_then(|v| v.to_str().ok()).ok_or_else(
                    || WebError::Http(format!("redirect {status} without a Location header")),
                )?;
            current = current
                .join(location)
                .map_err(|e| WebError::Network(format!("bad redirect target: {e}")))?;
            continue;
        }
        if !status.is_success() {
            return Err(WebError::Http(format!("HTTP {status} for {current}")));
        }
        return bounded_body(response, hops.body_cap).await;
    }
    Err(WebError::Http("too many redirects".to_string()))
}

/// The body, reading no more than `cap` bytes of it: a server streaming without end must not fill
/// memory inside the time budget. Decoded in the declared charset, as `text()` would.
async fn bounded_body(mut response: reqwest::Response, cap: usize) -> Result<Fetched, WebError> {
    let charset = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').find_map(|part| part.trim().strip_prefix("charset=")))
        .and_then(|label| encoding_rs::Encoding::for_label(label.trim_matches('"').as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    let mut body = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = response.chunk().await.map_err(|e| WebError::Parse(e.to_string()))? {
        let room = cap - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if chunk.len() > room {
            truncated = true;
            break;
        }
    }
    let (text, _, _) = charset.decode(&body);
    Ok(Fetched { body: text.into_owned(), truncated })
}

#[cfg(test)]
mod tests;
