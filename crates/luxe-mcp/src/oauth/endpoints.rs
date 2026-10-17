//! Endpoints — where an authorization code, a client secret or a token may be sent.
//!
//! Every endpoint is checked before it is used, discovered or configured: https, or plain http on
//! this machine. A metadata document must also name its issuer and be the one it was asked for,
//! so a document served from one place cannot send a token somewhere else.

use serde_json::Value;

use super::OAuthError;

/// The resolved OAuth endpoints for a server.
#[derive(Debug, Clone, Default)]
pub(super) struct Endpoints {
    pub(super) authorization: Option<String>,
    pub(super) token: Option<String>,
    pub(super) registration: Option<String>,
}

impl Endpoints {
    /// These endpoints, once every one of them is somewhere a secret may go.
    pub(super) fn checked(self) -> Result<Self, OAuthError> {
        for (endpoint, what) in [
            (&self.authorization, "the authorization endpoint"),
            (&self.token, "the token endpoint"),
            (&self.registration, "the registration endpoint"),
        ] {
            if let Some(endpoint) = endpoint {
                require_secure(endpoint, what)?;
            }
        }
        Ok(self)
    }
}

/// `url` is somewhere a code or a token may go: https, or plain http on this machine.
pub(crate) fn require_secure(url: &str, what: &str) -> Result<(), OAuthError> {
    let parsed = url::Url::parse(url)
        .map_err(|e| OAuthError::Insecure(format!("{what} {url} is not a URL: {e}")))?;
    match parsed.scheme() {
        "https" => Ok(()),
        "http" if is_loopback(&parsed) => Ok(()),
        _ => Err(OAuthError::Insecure(format!(
            "{what} {url} is not https, so nothing will be sent to it"
        ))),
    }
}

pub(super) fn is_loopback(url: &url::Url) -> bool {
    matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
}

/// The endpoints the authorization-server document at `candidate` describes, refused unless it
/// names `issuer` as itself (RFC 8414 requires the field) and every endpoint is secure.
pub(super) fn endpoints_from(
    doc: &Value,
    candidate: &str,
    issuer: &str,
) -> Result<Endpoints, OAuthError> {
    match doc.get("issuer").and_then(Value::as_str) {
        Some(claimed) if claimed.trim_end_matches('/') == issuer.trim_end_matches('/') => {}
        Some(claimed) => {
            return Err(OAuthError::Discovery(format!(
                "{candidate} says it is {claimed:?}, not {issuer} — refusing it"
            )))
        }
        None => {
            return Err(OAuthError::Discovery(format!("{candidate} names no issuer — refusing it")))
        }
    }
    let field = |k: &str| doc.get(k).and_then(Value::as_str).map(str::to_string);
    Endpoints {
        authorization: field("authorization_endpoint"),
        token: field("token_endpoint"),
        registration: field("registration_endpoint"),
    }
    .checked()
}
