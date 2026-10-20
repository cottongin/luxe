//! Access — who may reach the bridge, checked on every request.
//!
//! Two layers, both required for anything that touches a session:
//!
//! - **A tailnet login on the allowlist.** Tailscale Serve sets `Tailscale-User-Login` on what it
//!   forwards and drops any copy a client sent, so the header names the person behind the request.
//!   A request without it did not come through Serve, and is refused.
//! - **A paired device.** The header names a person, not a device, and any process on this machine
//!   can reach the loopback port and write the header itself. The device's own credential, from
//!   pairing, is what a process on this machine doesn't have.
//!
//! A browser also says which page sent a request (`Origin`), and one from another site is refused,
//! so a page elsewhere can't use the device's cookie to open a session socket.

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

use crate::devices::Device;
use crate::routes::Bridged;

/// The header Tailscale Serve sets to the signed-in tailnet user.
pub(crate) const LOGIN_HEADER: &str = "tailscale-user-login";
/// The cookie a paired device carries its credential in.
pub(crate) const CREDENTIAL_COOKIE: &str = "luxe_device";

/// The tailnet login behind a request, once checked.
#[derive(Debug, Clone)]
pub(crate) struct Login(pub(crate) String);

/// Why a request was turned away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    NotThroughTailscale,
    LoginNotAllowed(String),
    CrossSite,
    NotPaired,
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let (status, reason) = match self {
            Self::NotThroughTailscale => (
                StatusCode::UNAUTHORIZED,
                "this bridge answers only through Tailscale Serve".to_string(),
            ),
            Self::LoginNotAllowed(login) => {
                (StatusCode::FORBIDDEN, format!("{login} is not allowed on this bridge"))
            }
            Self::CrossSite => (StatusCode::FORBIDDEN, "cross-site requests are refused".into()),
            Self::NotPaired => (StatusCode::UNAUTHORIZED, "this device isn't paired".into()),
        };
        (status, reason).into_response()
    }
}

/// The allowed tailnet login a request came from, or why it has none.
pub(crate) fn tailnet_login(headers: &HeaderMap, allowed: &[String]) -> Result<Login, Refusal> {
    let login = headers
        .get(LOGIN_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|login| !login.is_empty())
        .ok_or(Refusal::NotThroughTailscale)?;
    // an email address, which Tailscale has reported in different cases for the same account
    let login_folded = login.to_lowercase();
    if !allowed.iter().any(|allowed| allowed.to_lowercase() == login_folded) {
        return Err(Refusal::LoginNotAllowed(login.to_string()));
    }
    Ok(Login(login.to_string()))
}

/// Refuses a request a page on another site sent. One with no `Origin` is not from a browser
/// page, so it carries no cookie it didn't own.
pub(crate) fn same_site(headers: &HeaderMap) -> Result<(), Refusal> {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|value| value.to_str().ok()) else {
        return Ok(());
    };
    let authority = origin.split_once("://").map_or(origin, |(_, rest)| rest);
    // Serve may hand the original host on in either
    let mut hosts = [header::HOST.as_str(), "x-forwarded-host"]
        .into_iter()
        .filter_map(|name| headers.get(name)?.to_str().ok());
    if hosts.any(|host| host.eq_ignore_ascii_case(authority)) {
        return Ok(());
    }
    Err(Refusal::CrossSite)
}

/// The device credential a request carries, if any.
pub(crate) fn credential(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|cookies| cookies.split(';'))
        .filter_map(|cookie| cookie.trim().split_once('='))
        .find_map(|(name, value)| (name == CREDENTIAL_COOKIE).then_some(value))
}

/// The cookie that pairs a device: unreadable to scripts, HTTPS only, never sent by another site,
/// and kept for as long as a browser keeps any cookie.
pub(crate) fn credential_cookie(credential: &str) -> String {
    format!(
        "{CREDENTIAL_COOKIE}={credential}; HttpOnly; Secure; SameSite=Strict; Path=/; \
         Max-Age=34560000"
    )
}

/// Every route: a tailnet login on the allowlist, from this site.
pub(crate) async fn require_login(
    State(bridged): State<Arc<Bridged>>,
    mut request: Request,
    next: Next,
) -> Response {
    let checked = tailnet_login(request.headers(), &bridged.logins)
        .and_then(|login| same_site(request.headers()).map(|()| login));
    match checked {
        Ok(login) => {
            request.extensions_mut().insert(login);
            next.run(request).await
        }
        Err(refusal) => refusal.into_response(),
    }
}

/// Session routes: a paired device as well.
pub(crate) async fn require_device(
    State(bridged): State<Arc<Bridged>>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(device) = paired_device(&bridged, request.headers()).await else {
        return Refusal::NotPaired.into_response();
    };
    request.extensions_mut().insert(device);
    next.run(request).await
}

async fn paired_device(bridged: &Bridged, headers: &HeaderMap) -> Option<Device> {
    let credential = credential(headers)?.to_string();
    let devices = bridged.devices.clone();
    tokio::task::spawn_blocking(move || devices.holder(&credential)).await.ok().flatten()
}
