//! GitHub Copilot — device flow, then a Copilot token exchange.
//!
//! Two tokens are in play. The device flow yields a long-lived **GitHub** token, which is what gets
//! stored as the refresh token; exchanging it at `copilot_internal/v2/token` yields a short-lived
//! **Copilot** token that requests actually carry. "Refresh" is therefore a re-exchange, not an
//! OAuth refresh grant.
//!
//! The Copilot token also encodes the endpoint this account must use (`proxy-ep=…`), which is why
//! `crate::auth::source` lets a credential override the adapter's base URL. Enterprise accounts go
//! through their own domain end to end.

use std::collections::BTreeMap;

mod models;
mod usage;

use luxe_core::auth::{AuthChoice, AuthError, AuthEvent, AuthInteraction, AuthPrompt};
use secrecy::ExposeSecret;
use serde_json::Value;

use super::super::credential::OAuthCredential;
use super::{post_form, provider_error_code, provider_error_message, DevicePacing, Poll};
use crate::config::ApiKey;
use models::{enable_and_list_models, fetch_models};
#[cfg(test)]
pub(crate) use usage::parse_plan_usage;
pub use usage::plan_usage;

/// GitHub's own editor-integration client id (public; a device-flow client has no secret).
const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";

/// Scope the editor integrations request.
const SCOPE: &str = "read:user";

/// Copilot's `/models` API version.
const API_VERSION: &str = "2026-06-01";

/// Identity headers Copilot's endpoint requires. Sent on every request, not just at login.
pub const EDITOR_HEADERS: &[(&str, &str)] = &[
    ("Editor-Version", "vscode/1.107.0"),
    ("Editor-Plugin-Version", "copilot-chat/0.35.0"),
    ("Copilot-Integration-Id", "vscode-chat"),
];

/// Endpoint used when a token doesn't name one and the account isn't on Enterprise.
const DEFAULT_API_BASE: &str = "https://api.individual.githubcopilot.com";

/// The GitHub everyone means.
const GITHUB_COM: &str = "github.com";

/// Where the flow talks to GitHub.
///
/// A struct rather than formatted constants so tests can drive the real code against a local
/// server, and so GitHub Enterprise is the same code path as github.com.
#[derive(Debug, Clone)]
pub struct Endpoints {
    /// Device-code issuance.
    pub device_code: String,
    /// Device-code polling.
    pub access_token: String,
    /// GitHub token → Copilot token exchange.
    pub copilot_token: String,
}

impl Endpoints {
    /// The endpoints for a GitHub domain (`github.com`, or an Enterprise host).
    pub fn for_domain(domain: &str) -> Self {
        Self {
            device_code: format!("https://{domain}/login/device/code"),
            access_token: format!("https://{domain}/login/oauth/access_token"),
            copilot_token: format!("https://api.{domain}/copilot_internal/v2/token"),
        }
    }
}

/// Run the device flow and exchange for a Copilot token.
pub async fn login(interaction: &dyn AuthInteraction) -> Result<OAuthCredential, AuthError> {
    let enterprise = choose_domain(interaction).await?;
    let domain = enterprise.clone().unwrap_or_else(|| GITHUB_COM.to_string());
    run_device_flow(&Endpoints::for_domain(&domain), enterprise.as_deref(), interaction).await
}

/// Ask which GitHub the subscription lives on, returning the Enterprise domain if there is one.
///
/// A question rather than a blank-for-default text box: almost everyone wants github.com, and
/// "leave it blank" is a worse way to say "yes, the normal one". Separated from the flow so the
/// question order is testable without a socket.
pub async fn choose_domain(interaction: &dyn AuthInteraction) -> Result<Option<String>, AuthError> {
    let choice = interaction
        .prompt(AuthPrompt::select(
            "Which GitHub account is your Copilot subscription on?",
            vec![
                AuthChoice {
                    id: "github".into(),
                    label: "GitHub".into(),
                    detail: Some("github.com — the usual choice".into()),
                },
                AuthChoice {
                    id: "enterprise".into(),
                    label: "GitHub Enterprise".into(),
                    detail: Some("A GitHub Enterprise Server at your own domain".into()),
                },
            ],
        ))
        .await?;
    if choice.expose_secret() != "enterprise" {
        return Ok(None);
    }

    let answer = interaction
        .prompt(AuthPrompt::text("GitHub Enterprise domain:", Some("company.ghe.com".to_string())))
        .await?;
    // Having chosen Enterprise, an empty domain is a mistake, not a default: silently falling back
    // to github.com would send the credential to the wrong host.
    normalize_domain(answer.expose_secret())?
        .map(Some)
        .ok_or_else(|| AuthError::Input("no domain entered".to_string()))
}

/// Run the device flow against `endpoints` and exchange the result for a Copilot credential.
pub async fn run_device_flow(
    endpoints: &Endpoints,
    enterprise: Option<&str>,
    interaction: &dyn AuthInteraction,
) -> Result<OAuthCredential, AuthError> {
    let device = start_device_flow(endpoints).await?;
    interaction.notify(AuthEvent::DeviceCode {
        user_code: device.user_code.clone(),
        verification_uri: device.verification_uri.clone(),
        interval_secs: device.interval_secs,
        expires_in_secs: device.expires_in_secs,
    });

    let github_token = poll_for_github_token(endpoints, &device).await?;
    interaction
        .notify(AuthEvent::Progress { message: "Requesting a Copilot token…".to_string() });
    let mut credential = exchange_copilot_token(endpoints, &github_token, enterprise).await?;

    interaction.notify(AuthEvent::Progress { message: "Checking available models…".to_string() });
    credential.available_models = enable_and_list_models(&credential).await;
    Ok(credential)
}

/// Re-exchange the stored GitHub token for a fresh Copilot token.
pub async fn refresh(credential: &OAuthCredential) -> Result<OAuthCredential, AuthError> {
    let domain = credential.enterprise_domain.as_deref().unwrap_or(GITHUB_COM);
    let mut refreshed = exchange_copilot_token(
        &Endpoints::for_domain(domain),
        credential.refresh.expose(),
        credential.enterprise_domain.as_deref(),
    )
    .await?;
    // A plan's models change under it, and a list kept from login went stale both ways: it offered
    // models the account had lost and hid ones it had gained. One request per token is the price.
    // Listing only: accepting a model's policy stays a step the user takes, at `/login`.
    refreshed.available_models =
        list_models(&refreshed).await.unwrap_or_else(|| credential.available_models.clone());
    Ok(refreshed)
}

/// The models this account may use now, or `None` when the list cannot be had.
async fn list_models(credential: &OAuthCredential) -> Option<Vec<String>> {
    let base = credential.endpoint.clone().unwrap_or_else(|| DEFAULT_API_BASE.to_string());
    let models = fetch_models(&base, credential.access.expose()).await?;
    Some(models.into_iter().filter(|model| model.selectable).map(|model| model.id).collect())
}

/// One device-flow authorization request.
#[derive(Debug)]
struct DeviceFlow {
    device_code: String,
    user_code: String,
    verification_uri: String,
    interval_secs: u64,
    expires_in_secs: u64,
}

async fn start_device_flow(endpoints: &Endpoints) -> Result<DeviceFlow, AuthError> {
    let body =
        post_form(&endpoints.device_code, &[("client_id", CLIENT_ID), ("scope", SCOPE)]).await?;

    let field = |key: &str| body.get(key).and_then(Value::as_str).map(str::to_string);
    let device_code = field("device_code").ok_or_else(|| missing("device_code"))?;
    let user_code = field("user_code").ok_or_else(|| missing("user_code"))?;
    let verification_uri = field("verification_uri").ok_or_else(|| missing("verification_uri"))?;
    // The URI is handed to a browser launcher, so it has to be a real http(s) URL and not, say, a
    // path to something executable.
    let parsed = reqwest::Url::parse(&verification_uri)
        .map_err(|_| AuthError::Protocol("GitHub returned an unusable verification URL".into()))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(AuthError::Protocol(
            "GitHub returned a verification URL that isn't http(s)".to_string(),
        ));
    }

    Ok(DeviceFlow {
        device_code,
        user_code,
        verification_uri: parsed.to_string(),
        interval_secs: body.get("interval").and_then(Value::as_u64).unwrap_or(5),
        expires_in_secs: body.get("expires_in").and_then(Value::as_u64).unwrap_or(900),
    })
}

async fn poll_for_github_token(
    endpoints: &Endpoints,
    device: &DeviceFlow,
) -> Result<String, AuthError> {
    let url = endpoints.access_token.clone();
    let pacing = DevicePacing {
        interval_secs: device.interval_secs,
        expires_in_secs: device.expires_in_secs,
        // GitHub rejects a poll that arrives before the first interval has elapsed.
        wait_first: true,
    };
    super::poll_device_code(pacing, || {
        let url = url.clone();
        let device_code = device.device_code.clone();
        async move {
            let form = [
                ("client_id", CLIENT_ID),
                ("device_code", device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ];
            match post_form(&url, &form).await {
                Ok(body) => match body.get("access_token").and_then(Value::as_str) {
                    Some(token) if !token.is_empty() => Poll::Done(token.to_string()),
                    _ => classify(&body),
                },
                Err(AuthError::Rejected(message)) => Poll::Failed(message),
                Err(error) => Poll::Failed(error.to_string()),
            }
        }
    })
    .await
}

/// GitHub answers a pending device poll with HTTP 200 and an `error` field.
fn classify(body: &Value) -> Poll<String> {
    match provider_error_code(body).as_deref() {
        Some("authorization_pending") => Poll::Pending,
        Some("slow_down") => Poll::SlowDown(body.get("interval").and_then(Value::as_u64)),
        Some(code) => Poll::Failed(
            provider_error_message(body).unwrap_or_else(|| format!("device flow failed: {code}")),
        ),
        None => Poll::Failed("GitHub returned no token and no error".to_string()),
    }
}

/// Trade a GitHub token for a Copilot token.
pub async fn exchange_copilot_token(
    endpoints: &Endpoints,
    github_token: &str,
    enterprise_domain: Option<&str>,
) -> Result<OAuthCredential, AuthError> {
    let domain = enterprise_domain.unwrap_or(GITHUB_COM);
    let url = endpoints.copilot_token.clone();
    let mut request = crate::http::shared_client()
        .get(&url)
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {github_token}"));
    for (name, value) in EDITOR_HEADERS {
        request = request.header(*name, *value);
    }
    let response =
        request.send().await.map_err(|error| AuthError::Network(format!("{url}: {error}")))?;
    let body = super::read_json(&url, response).await.map_err(|error| match error {
        AuthError::Rejected(message) if message.contains("403") => AuthError::Rejected(format!(
            "{message} — this GitHub account doesn't have an active Copilot subscription"
        )),
        other => other,
    })?;

    let token = body
        .get("token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| missing("token"))?;
    // `expires_at` is unix *seconds*.
    let expires_at_secs = body.get("expires_at").and_then(Value::as_u64).unwrap_or(0);
    let expires_at_ms = match expires_at_secs {
        0 => super::expiry_from_expires_in(25 * 60),
        secs => secs.saturating_mul(1000).saturating_sub(5 * 60 * 1000),
    };

    Ok(OAuthCredential {
        access: ApiKey::new(token),
        refresh: ApiKey::new(github_token),
        expires_at_ms,
        account: token_field(token, "tid").or_else(|| Some(domain.to_string())),
        endpoint: Some(api_base_url(token, enterprise_domain)),
        enterprise_domain: enterprise_domain.map(str::to_string),
        available_models: Vec::new(),
    })
}

/// The API base this credential must use.
///
/// The Copilot token carries `proxy-ep=proxy.<tenant>.githubcopilot.com`; the REST base is the same
/// host with `api.` in place of `proxy.`. Individual, Business, and Enterprise accounts all land on
/// different hosts, and using the wrong one 401s.
pub fn api_base_url(copilot_token: &str, enterprise_domain: Option<&str>) -> String {
    if let Some(proxy) = token_field(copilot_token, "proxy-ep") {
        let host = proxy.strip_prefix("proxy.").map(|rest| format!("api.{rest}")).unwrap_or(proxy);
        return format!("https://{host}");
    }
    match enterprise_domain {
        Some(domain) => format!("https://copilot-api.{domain}"),
        None => DEFAULT_API_BASE.to_string(),
    }
}

/// Read one `key=value;` field out of a Copilot token.
fn token_field(token: &str, key: &str) -> Option<String> {
    parse_token_fields(token).get(key).cloned()
}

fn parse_token_fields(token: &str) -> BTreeMap<String, String> {
    token
        .split(';')
        .filter_map(|part| part.split_once('='))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .collect()
}

/// Normalize whatever the user typed into a bare hostname. Empty means github.com.
fn normalize_domain(input: &str) -> Result<Option<String>, AuthError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let candidate =
        if trimmed.contains("://") { trimmed.to_string() } else { format!("https://{trimmed}") };
    let host = reqwest::Url::parse(&candidate)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .filter(|host| host.contains('.'));
    match host {
        Some(host) => Ok(Some(host)),
        None => Err(AuthError::Input(format!("'{trimmed}' is not a GitHub Enterprise domain"))),
    }
}

fn missing(field: &str) -> AuthError {
    AuthError::Protocol(format!("GitHub's response carried no {field}"))
}

#[cfg(test)]
mod tests;
