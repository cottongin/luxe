//! OpenAI Codex — signing in with a ChatGPT Plus/Pro plan.
//!
//! Two ways in, because one of them has to work on a machine with no browser:
//!
//! - **Browser** — PKCE authorization code with a loopback listener on the port OpenAI's client is
//!   registered for (`1455`), raced against a paste-the-URL prompt so it also works over SSH.
//! - **Device code** — OpenAI's own device-authorization endpoints (not RFC 8628: the poll returns
//!   an *authorization code plus verifier*, which then goes through the normal exchange).
//!
//! Requests afterwards go to the ChatGPT backend, not `api.openai.com`, and carry the account id
//! from the access token's claims — see `crate::auth::source` for the header derivation.

use luxe_core::auth::{AuthChoice, AuthError, AuthEvent, AuthInteraction, AuthPrompt};
use secrecy::ExposeSecret;
use serde_json::Value;

use super::super::credential::OAuthCredential;
use luxe_core::oauth_loopback::LoopbackError;

use super::{
    await_code, expiry_from_expires_in, post_form, post_json, CallbackServer, DevicePacing, Poll,
};
use crate::config::ApiKey;

/// The Codex client id. Public by construction — a PKCE client has no secret.
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

const AUTH_BASE: &str = "https://auth.openai.com";

/// Where the flow talks to OpenAI.
///
/// A struct rather than constants so the tests can drive the real code against a local server; the
/// public entry points always use [`Endpoints::default`].
#[derive(Debug, Clone)]
pub struct Endpoints {
    /// Authorization page the browser opens.
    pub authorize: String,
    /// Token exchange and refresh.
    pub token: String,
    /// Device-code issuance.
    pub device_usercode: String,
    /// Device-code polling.
    pub device_token: String,
    /// Page the user types the device code into.
    pub device_verification: String,
    /// Redirect URI the device flow's authorization code was issued against.
    pub device_redirect: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self::at(AUTH_BASE)
    }
}

impl Endpoints {
    /// Every endpoint under one base URL.
    pub fn at(base: &str) -> Self {
        let base = base.trim_end_matches('/');
        Self {
            authorize: format!("{base}/oauth/authorize"),
            token: format!("{base}/oauth/token"),
            device_usercode: format!("{base}/api/accounts/deviceauth/usercode"),
            device_token: format!("{base}/api/accounts/deviceauth/token"),
            device_verification: format!("{base}/codex/device"),
            device_redirect: format!("{base}/deviceauth/callback"),
        }
    }
}

/// The loopback port OpenAI's registered redirect URI uses. Not negotiable.
const CALLBACK_PORT: u16 = 1455;
const CALLBACK_PATH: &str = "/auth/callback";

const SCOPE: &str = "openid profile email offline_access";

/// Claim namespace OpenAI puts the account id under.
const AUTH_CLAIM: &str = "https://api.openai.com/auth";

/// How luxe identifies itself to the Codex backend, unless `LUXE_CODEX_ORIGINATOR` says otherwise.
///
/// Sent on the authorize request and on every Codex call. Honest by default — luxe says it is
/// luxe. The override exists because this is the one value in the flow taken from another client's
/// behavior rather than from documentation: if the backend ever refuses an unknown originator, a
/// user can get working again without waiting for a release.
const DEFAULT_ORIGINATOR: &str = "luxe";

/// The originator to send.
pub fn originator() -> String {
    std::env::var("LUXE_CODEX_ORIGINATOR")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_ORIGINATOR.to_string())
}

/// A device code stays valid for fifteen minutes.
const DEVICE_TIMEOUT_SECS: u64 = 15 * 60;

/// Run the login, asking which method to use.
pub async fn login(interaction: &dyn AuthInteraction) -> Result<OAuthCredential, AuthError> {
    login_with(&Endpoints::default(), interaction).await
}

/// Run the login against specific endpoints.
pub async fn login_with(
    endpoints: &Endpoints,
    interaction: &dyn AuthInteraction,
) -> Result<OAuthCredential, AuthError> {
    let choice = interaction
        .prompt(AuthPrompt::select(
            "How do you want to sign in?",
            vec![
                AuthChoice {
                    id: "browser".into(),
                    label: "Open a browser".into(),
                    detail: Some("Best on a desktop; you can also paste the redirect URL".into()),
                },
                AuthChoice {
                    id: "device".into(),
                    label: "Enter a code on another device".into(),
                    detail: Some("For a headless machine or an SSH session".into()),
                },
            ],
        ))
        .await?;
    match choice.expose_secret() {
        "device" => login_device_code(endpoints, interaction).await,
        _ => login_browser(endpoints, interaction).await,
    }
}

/// PKCE authorization-code login through a loopback listener.
async fn login_browser(
    endpoints: &Endpoints,
    interaction: &dyn AuthInteraction,
) -> Result<OAuthCredential, AuthError> {
    let pkce = super::pkce();
    let state = super::random_state();
    let mut server = CallbackServer::bind(CALLBACK_PORT, CALLBACK_PATH, Some(state.clone()))
        .await
        .map_err(|error| match error {
            LoopbackError::InUse { .. } => AuthError::Network(format!(
                "{error} — OpenAI requires this exact port, so close whatever is using it (often \
                 the Codex CLI mid-login) and try again, or choose the device-code method"
            )),
            other => other.into(),
        })?;
    let redirect_uri = server.redirect_uri();

    let url = authorize_url(endpoints, &pkce.challenge, &state, &redirect_uri);
    interaction.notify(AuthEvent::OpenUrl {
        url,
        instructions: Some(
            "Sign in with the ChatGPT account that has your Plus or Pro plan.".to_string(),
        ),
    });

    let callback = await_code(&mut server, interaction, Some(&state)).await?;
    interaction
        .notify(AuthEvent::Progress { message: "Exchanging the authorization code…".into() });
    exchange_code(endpoints, &callback.code, &pkce.verifier, &redirect_uri).await
}

/// OpenAI's device-authorization login.
async fn login_device_code(
    endpoints: &Endpoints,
    interaction: &dyn AuthInteraction,
) -> Result<OAuthCredential, AuthError> {
    let start =
        post_json(&endpoints.device_usercode, serde_json::json!({ "client_id": CLIENT_ID }))
            .await
            .map_err(|error| {
                match error {
            AuthError::Rejected(message) if message.contains("404") => AuthError::Rejected(
                "OpenAI's device-code sign-in isn't available for this account; use the browser \
                 method instead"
                    .to_string(),
            ),
            other => other,
        }
            })?;

    let device_auth_id = string_field(&start, "device_auth_id").ok_or_else(|| {
        AuthError::Protocol("the device-code response carried no device_auth_id".to_string())
    })?;
    let user_code = string_field(&start, "user_code").ok_or_else(|| {
        AuthError::Protocol("the device-code response carried no user_code".to_string())
    })?;
    let interval_secs = number_field(&start, "interval").unwrap_or(5);

    interaction.notify(AuthEvent::DeviceCode {
        user_code: user_code.clone(),
        verification_uri: endpoints.device_verification.clone(),
        interval_secs,
        expires_in_secs: DEVICE_TIMEOUT_SECS,
    });

    let pacing =
        DevicePacing { interval_secs, expires_in_secs: DEVICE_TIMEOUT_SECS, wait_first: false };
    let authorized = super::poll_device_code(pacing, || {
        let body = serde_json::json!({
            "device_auth_id": device_auth_id,
            "user_code": user_code,
        });
        let url = endpoints.device_token.clone();
        async move {
            match post_json(&url, body).await {
                Ok(value) => match (
                    string_field(&value, "authorization_code"),
                    string_field(&value, "code_verifier"),
                ) {
                    (Some(code), Some(verifier)) => Poll::Done((code, verifier)),
                    _ => Poll::Failed(
                        "the device-code response carried no authorization code".to_string(),
                    ),
                },
                Err(AuthError::Rejected(message)) => classify_device_error(&message),
                Err(error) => Poll::Failed(error.to_string()),
            }
        }
    })
    .await?;

    interaction
        .notify(AuthEvent::Progress { message: "Exchanging the authorization code…".into() });
    exchange_code(endpoints, &authorized.0, &authorized.1, &endpoints.device_redirect).await
}

/// Pending-vs-fatal for a device poll. OpenAI answers a not-yet-authorized poll with 403/404 as
/// often as with a proper `authorization_pending`, so both are "keep waiting".
fn classify_device_error<T>(message: &str) -> Poll<T> {
    let lower = message.to_ascii_lowercase();
    if lower.contains("authorization_pending")
        || lower.contains("deviceauth_authorization_pending")
        || lower.contains(" 403")
        || lower.contains(" 404")
    {
        return Poll::Pending;
    }
    if lower.contains("slow_down") {
        return Poll::SlowDown(None);
    }
    Poll::Failed(message.to_string())
}

/// The authorization URL, with the parameters the Codex client sends.
fn authorize_url(
    endpoints: &Endpoints,
    challenge: &str,
    state: &str,
    redirect_uri: &str,
) -> String {
    let Ok(mut url) = reqwest::Url::parse(&endpoints.authorize) else {
        return endpoints.authorize.clone();
    };
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", SCOPE)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("originator", &originator());
    url.to_string()
}

/// Trade an authorization code for tokens against specific endpoints.
pub async fn exchange_code(
    endpoints: &Endpoints,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<OAuthCredential, AuthError> {
    let body = post_form(
        &endpoints.token,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect_uri),
        ],
    )
    .await?;
    credential_from_tokens(&body)
}

/// Exchange a refresh token for a new access token.
pub async fn refresh(credential: &OAuthCredential) -> Result<OAuthCredential, AuthError> {
    refresh_with(&Endpoints::default(), credential).await
}

/// Refresh against specific endpoints.
pub async fn refresh_with(
    endpoints: &Endpoints,
    credential: &OAuthCredential,
) -> Result<OAuthCredential, AuthError> {
    let body = post_form(
        &endpoints.token,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", CLIENT_ID),
            ("refresh_token", credential.refresh.expose()),
        ],
    )
    .await?;
    let mut refreshed = credential_from_tokens(&body)?;
    // OpenAI usually rotates the refresh token; when it doesn't, keep the one we have.
    if refreshed.refresh.is_empty() {
        refreshed.refresh = credential.refresh.clone();
    }
    Ok(refreshed)
}

/// Build a credential from a token response, reading the account id out of the access token.
pub fn credential_from_tokens(body: &Value) -> Result<OAuthCredential, AuthError> {
    let access = string_field(body, "access_token").ok_or_else(|| {
        AuthError::Protocol("the token response carried no access_token".to_string())
    })?;
    let account = account_id(&access).ok_or_else(|| {
        AuthError::Protocol(
            "the ChatGPT token carries no account id — is this account on a Plus or Pro plan?"
                .to_string(),
        )
    })?;
    let expires_in = number_field(body, "expires_in").unwrap_or(3600);
    Ok(OAuthCredential {
        access: ApiKey::new(access),
        refresh: ApiKey::new(string_field(body, "refresh_token").unwrap_or_default()),
        expires_at_ms: expiry_from_expires_in(expires_in),
        account: Some(account),
        endpoint: None,
        enterprise_domain: None,
        available_models: Vec::new(),
    })
}

/// The ChatGPT account id from an access token's claims.
pub fn account_id(access_token: &str) -> Option<String> {
    let claims = super::decode_jwt_payload(access_token)?;
    claims
        .get(AUTH_CLAIM)?
        .get("chatgpt_account_id")?
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn string_field(body: &Value, key: &str) -> Option<String> {
    body.get(key).and_then(Value::as_str).filter(|value| !value.is_empty()).map(str::to_string)
}

/// Read a number that a provider may have encoded as a string (OpenAI's device endpoint does).
fn number_field(body: &Value, key: &str) -> Option<u64> {
    let value = body.get(key)?;
    value.as_u64().or_else(|| value.as_str()?.trim().parse().ok())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use base64::Engine as _;

    /// Serializes the tests that read or write `LUXE_CODEX_ORIGINATOR`.
    ///
    /// It is a process-global and `cargo test` runs tests as threads, so "restore it at the end of my
    /// own body" is not isolation: a sibling reading `originator()` mid-flight sees whatever this test
    /// last set, and the failure reads as a wrong authorize URL.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn jwt_with_account(account: &str) -> String {
        let claims = serde_json::json!({ AUTH_CLAIM: { "chatgpt_account_id": account } });
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&claims).unwrap());
        format!("header.{payload}.signature")
    }

    #[test]
    fn the_authorize_url_carries_every_parameter_the_backend_requires() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let url = authorize_url(
            &Endpoints::default(),
            "chal",
            "st",
            "http://localhost:1455/auth/callback",
        );
        let parsed = reqwest::Url::parse(&url).unwrap();
        let pairs: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(pairs["client_id"], CLIENT_ID);
        assert_eq!(pairs["response_type"], "code");
        assert_eq!(pairs["code_challenge_method"], "S256");
        assert_eq!(pairs["code_challenge"], "chal");
        assert_eq!(pairs["state"], "st");
        assert_eq!(pairs["redirect_uri"], "http://localhost:1455/auth/callback");
        assert_eq!(pairs["scope"], SCOPE);
        assert_eq!(pairs["originator"], originator());
        assert_eq!(pairs["codex_cli_simplified_flow"], "true");
        assert!(parsed.as_str().starts_with("https://auth.openai.com/oauth/authorize?"));
    }

    #[test]
    fn the_originator_defaults_to_luxe_and_is_overridable() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let previous = std::env::var("LUXE_CODEX_ORIGINATOR").ok();
        std::env::remove_var("LUXE_CODEX_ORIGINATOR");
        assert_eq!(originator(), "luxe", "luxe identifies itself honestly by default");

        std::env::set_var("LUXE_CODEX_ORIGINATOR", "codex_cli_rs");
        assert_eq!(originator(), "codex_cli_rs");
        let url = authorize_url(&Endpoints::default(), "c", "s", "http://localhost:1455/cb");
        assert!(url.contains("originator=codex_cli_rs"), "{url}");

        // Blank or whitespace falls back rather than sending an empty originator.
        std::env::set_var("LUXE_CODEX_ORIGINATOR", "   ");
        assert_eq!(originator(), "luxe");

        match previous {
            Some(value) => std::env::set_var("LUXE_CODEX_ORIGINATOR", value),
            None => std::env::remove_var("LUXE_CODEX_ORIGINATOR"),
        }
    }

    #[test]
    fn a_token_response_becomes_a_credential_with_the_account_id() {
        let body = serde_json::json!({
            "access_token": jwt_with_account("acct-9"),
            "refresh_token": "refresh-1",
            "expires_in": 3600,
        });
        let credential = credential_from_tokens(&body).expect("valid");
        assert_eq!(credential.account.as_deref(), Some("acct-9"));
        assert_eq!(credential.refresh.expose(), "refresh-1");
        assert!(credential.expires_at_ms > super::super::super::credential::now_ms());
    }

    /// A ChatGPT token with no account claim can't address the Codex backend, and the failure has to
    /// say that rather than 400 later.
    #[test]
    fn a_token_without_an_account_claim_is_rejected_up_front() {
        let body = serde_json::json!({ "access_token": "header.e30.sig", "expires_in": 60 });
        let error = credential_from_tokens(&body).expect_err("must reject");
        assert!(error.to_string().contains("account id"), "unhelpful: {error}");
    }

    #[test]
    fn a_missing_access_token_is_a_protocol_error() {
        let error = credential_from_tokens(&serde_json::json!({})).expect_err("must reject");
        assert!(matches!(error, AuthError::Protocol(_)));
    }

    #[test]
    fn account_id_is_read_from_the_claim_namespace() {
        assert_eq!(account_id(&jwt_with_account("a1")).as_deref(), Some("a1"));
        assert!(account_id("garbage").is_none());
        assert!(account_id(&jwt_with_account("")).is_none(), "an empty id is no id");
    }

    /// OpenAI answers an unauthorized device poll with 403/404 as often as with a proper error code;
    /// treating those as failures would abort every device login immediately.
    #[test]
    fn device_poll_errors_are_classified_conservatively() {
        assert!(matches!(
            classify_device_error::<()>("https://… returned 403 Forbidden: {}"),
            Poll::Pending
        ));
        assert!(matches!(classify_device_error::<()>("returned 404: not found"), Poll::Pending));
        assert!(matches!(
            classify_device_error::<()>("error: deviceauth_authorization_pending"),
            Poll::Pending
        ));
        assert!(matches!(classify_device_error::<()>("error: slow_down"), Poll::SlowDown(None)));
        assert!(matches!(classify_device_error::<()>("returned 400: bad_client"), Poll::Failed(_)));
    }

    #[test]
    fn numeric_fields_tolerate_string_encoding() {
        assert_eq!(number_field(&serde_json::json!({ "interval": 7 }), "interval"), Some(7));
        assert_eq!(number_field(&serde_json::json!({ "interval": " 7 " }), "interval"), Some(7));
        assert_eq!(number_field(&serde_json::json!({ "interval": "x" }), "interval"), None);
        assert_eq!(number_field(&serde_json::json!({}), "interval"), None);
    }
}
