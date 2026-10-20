//! OAuth — PKCE, the loopback callback, device-code polling, and the subscription flows.
//!
//! Three flows ship:
//! [`openai_codex`] for a ChatGPT Plus/Pro plan, [`github_copilot`] for a Copilot subscription, and
//! [`openrouter`], whose published flow mints an API key the user owns. Everything they share —
//! PKCE and a localhost callback that also works over SSH (from [`luxe_core::oauth_loopback`],
//! which MCP shares too), and an RFC 8628 poller — lives here. Anthropic and Google stay API-key-only
//! because their subscription terms prohibit these third-party flows.
//!
//! Every flow is cancellable by dropping its future: the callback listener closes with the server
//! value, and the poller checks for cancellation between attempts.

pub mod github_copilot;
pub mod openai_codex;
pub mod openrouter;

use std::time::Duration;

// The poller's deadline rides tokio's clock, not `std::time::Instant`: `tokio::time::pause()` moves
// one and not the other, and a test that has to sleep through a 15-minute device-code window in
// real time is a test nobody runs.
use tokio::time::Instant;

use base64::Engine as _;
use luxe_core::auth::{AuthError, AuthInteraction, AuthPrompt};
use luxe_core::oauth_loopback::{LoopbackCallback, LoopbackError};
use serde_json::Value;

use super::catalog::OauthFlow;
use super::credential::{Credential, OAuthCredential};

/// Run a subscription login and return the credential to store.
pub async fn login(
    flow: OauthFlow,
    interaction: &dyn AuthInteraction,
) -> Result<Credential, AuthError> {
    match flow {
        OauthFlow::OpenaiCodex => openai_codex::login(interaction).await.map(Credential::OAuth),
        OauthFlow::GithubCopilot => github_copilot::login(interaction).await.map(Credential::OAuth),
        // OpenRouter hands back a durable key, not a token pair, so it is stored as an api key.
        OauthFlow::OpenRouter => openrouter::login(interaction).await,
    }
}

/// Exchange a refresh token for a fresh access token.
pub async fn refresh(
    flow: OauthFlow,
    credential: &OAuthCredential,
) -> Result<OAuthCredential, AuthError> {
    match flow {
        OauthFlow::OpenaiCodex => openai_codex::refresh(credential).await,
        OauthFlow::GithubCopilot => github_copilot::refresh(credential).await,
        OauthFlow::OpenRouter => Err(AuthError::Protocol(
            "OpenRouter credentials are API keys and don't refresh".to_string(),
        )),
    }
}

// === PKCE, state and the loopback callback ===

pub use luxe_core::oauth_loopback::{pkce, random_state, Pkce};

/// What the provider sent back to the loopback listener.
#[derive(Debug, Clone)]
pub struct Callback {
    /// The authorization code.
    pub code: String,
    /// The `state` value the provider echoed, when it sent one.
    pub state: Option<String>,
}

/// A one-shot localhost listener for an OAuth redirect, over [`luxe_core::oauth_loopback`].
///
/// Dropping it closes the socket, which is how a cancelled login stops listening.
pub struct CallbackServer {
    redirect_uri: String,
    receiver: tokio::sync::oneshot::Receiver<Result<Callback, String>>,
    task: tokio::task::JoinHandle<()>,
}

impl CallbackServer {
    /// Listen on `localhost:port` (`0` for any free port) for one redirect on `path`.
    ///
    /// `localhost`, not `127.0.0.1`: providers register the literal string, and OpenAI's registered
    /// Codex redirect is `http://localhost:1455/auth/callback`. With `expected_state`, a redirect
    /// carrying any other state is someone else's and is ignored; the code is never exchanged.
    pub async fn bind(
        port: u16,
        path: &str,
        expected_state: Option<String>,
    ) -> Result<Self, LoopbackError> {
        let callback = LoopbackCallback::bind(&format!("http://localhost:{port}{path}")).await?;
        let redirect_uri = callback.redirect_uri().to_string();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let Ok(arrival) = callback.wait(expected_state.as_deref()).await else { return };
            let result = match (arrival.error().map(str::to_string), arrival.param("code")) {
                (Some(message), _) => Err(message),
                (None, Some(code)) => {
                    let state = arrival.param("state").map(str::to_string);
                    Ok(Callback { code: code.to_string(), state })
                }
                (None, None) => Err("the redirect carried no authorization code".to_string()),
            };
            match &result {
                Ok(_) => {
                    let message = "You can close this tab and return to the terminal.";
                    arrival.answer(200, "Signed in to luxe", message).await;
                }
                Err(message) => {
                    let message = format!("Authorization failed: {message}");
                    arrival.answer(400, "luxe couldn't finish the sign-in", &message).await;
                }
            }
            let _ = sender.send(result);
        });
        Ok(Self { redirect_uri, receiver, task })
    }

    /// The redirect URI to send the provider.
    pub fn redirect_uri(&self) -> String {
        self.redirect_uri.clone()
    }

    /// Wait for the redirect. `None` means the listener stopped without one.
    pub async fn wait(&mut self) -> Option<Result<Callback, String>> {
        (&mut self.receiver).await.ok()
    }
}

impl Drop for CallbackServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Parse whatever the user pasted when the browser couldn't reach the loopback.
///
/// Accepts the full redirect URL, a bare code, `code#state` (some providers render it that way), or
/// a raw query string. Returning the state lets the caller reject a mismatch.
pub fn parse_pasted_code(input: &str) -> Option<Callback> {
    let value = input.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(url) = reqwest::Url::parse(value) {
        if url.scheme().starts_with("http") {
            let mut code = None;
            let mut state = None;
            for (key, item) in url.query_pairs() {
                match key.as_ref() {
                    "code" => code = Some(item.into_owned()),
                    "state" => state = Some(item.into_owned()),
                    _ => {}
                }
            }
            return code.map(|code| Callback { code, state });
        }
    }
    if value.contains("code=") {
        let query = value.split_once('?').map(|(_, query)| query).unwrap_or(value);
        let mut code = None;
        let mut state = None;
        for pair in query.split('&') {
            match pair.split_once('=') {
                Some(("code", value)) => code = Some(value.to_string()),
                Some(("state", value)) => state = Some(value.to_string()),
                _ => {}
            }
        }
        return code.map(|code| Callback { code, state });
    }
    match value.split_once('#') {
        Some((code, state)) if !code.is_empty() => {
            Some(Callback { code: code.to_string(), state: Some(state.to_string()) })
        }
        _ => Some(Callback { code: value.to_string(), state: None }),
    }
}

/// Race the loopback listener against a paste prompt, so a login works whether or not the browser
/// can reach this machine.
///
/// Whichever arrives first wins; the loser's future is dropped. A front-end treats the next
/// [`luxe_core::auth::AuthEvent`] as superseding the prompt it was showing, which is why the caller
/// emits a progress event straight after this returns.
pub async fn await_code(
    server: &mut CallbackServer,
    interaction: &dyn AuthInteraction,
    expected_state: Option<&str>,
) -> Result<Callback, AuthError> {
    let prompt = AuthPrompt::text(
        "Finish in the browser, or paste the redirect URL / code here:",
        Some(server.redirect_uri()),
    );
    let callback = tokio::select! {
        received = server.wait() => match received {
            Some(Ok(callback)) => callback,
            Some(Err(message)) => return Err(AuthError::Rejected(message)),
            None => return Err(AuthError::Timeout(
                "the browser redirect never arrived".to_string(),
            )),
        },
        pasted = interaction.prompt(prompt) => {
            let answer = pasted?;
            parse_pasted_code(secrecy::ExposeSecret::expose_secret(&answer)).ok_or_else(|| {
                AuthError::Input("that doesn't contain an authorization code".to_string())
            })?
        }
    };
    if let (Some(expected), Some(actual)) = (expected_state, callback.state.as_deref()) {
        if expected != actual {
            return Err(AuthError::Rejected(
                "OAuth state mismatch — start the sign-in again".to_string(),
            ));
        }
    }
    Ok(callback)
}

// === Device code polling ===

/// One poll's outcome.
#[derive(Debug)]
pub enum Poll<T> {
    /// Authorized.
    Done(T),
    /// Still waiting on the user.
    Pending,
    /// The server asked us to back off; carries a new interval when it supplied one.
    SlowDown(Option<u64>),
    /// Terminal failure.
    Failed(String),
}

/// How a device-code flow is paced.
#[derive(Debug, Clone, Copy)]
pub struct DevicePacing {
    /// Seconds between polls, as the server asked.
    pub interval_secs: u64,
    /// Seconds until the user code expires.
    pub expires_in_secs: u64,
    /// Whether to wait one interval before the first poll (GitHub rejects an immediate one).
    pub wait_first: bool,
}

/// RFC 8628 minimum: a server that omits `interval` means five seconds.
const DEFAULT_INTERVAL_SECS: u64 = 5;

/// RFC 8628 §3.5: `slow_down` means add five seconds.
const SLOW_DOWN_STEP_SECS: u64 = 5;

/// Poll until the user authorizes, the code expires, or the server refuses.
///
/// `poll` is called on each attempt. Honors a server-supplied `slow_down` interval in preference to
/// our own bookkeeping — a VM with a drifting clock otherwise polls early forever and never
/// finishes.
pub async fn poll_device_code<T, F, Fut>(pacing: DevicePacing, mut poll: F) -> Result<T, AuthError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Poll<T>>,
{
    let deadline = Instant::now() + Duration::from_secs(pacing.expires_in_secs.max(1));
    let mut interval = Duration::from_secs(pacing.interval_secs.max(1).max(DEFAULT_INTERVAL_SECS));
    let mut slowed = false;

    if pacing.wait_first {
        tokio::time::sleep(interval.min(deadline.saturating_duration_since(Instant::now()))).await;
    }

    while Instant::now() < deadline {
        match poll().await {
            Poll::Done(value) => return Ok(value),
            Poll::Failed(message) => return Err(AuthError::Rejected(message)),
            Poll::SlowDown(server_interval) => {
                slowed = true;
                interval = match server_interval {
                    Some(secs) if secs > 0 => Duration::from_secs(secs),
                    _ => interval + Duration::from_secs(SLOW_DOWN_STEP_SECS),
                };
            }
            Poll::Pending => {}
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        tokio::time::sleep(interval.min(remaining)).await;
    }

    Err(AuthError::Timeout(if slowed {
        "the sign-in timed out after the server asked us to slow down — if this machine's clock is \
         off (common in VMs and WSL), sync it and try again"
            .to_string()
    } else {
        "the sign-in code expired before it was authorized".to_string()
    }))
}

// === HTTP helpers ===

/// POST a JSON body and parse a JSON response, with the status folded into the error.
pub async fn post_json(url: &str, body: Value) -> Result<Value, AuthError> {
    let response = crate::http::shared_client()
        .post(url)
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|error| AuthError::Network(format!("{url}: {error}")))?;
    read_json(url, response).await
}

/// POST a form body and parse a JSON response.
pub async fn post_form(url: &str, form: &[(&str, &str)]) -> Result<Value, AuthError> {
    let response = crate::http::shared_client()
        .post(url)
        .header("Accept", "application/json")
        .form(form)
        .send()
        .await
        .map_err(|error| AuthError::Network(format!("{url}: {error}")))?;
    read_json(url, response).await
}

/// Read a JSON body, treating a non-2xx as an error that names the provider's own message.
pub async fn read_json(url: &str, response: reqwest::Response) -> Result<Value, AuthError> {
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| AuthError::Network(format!("{url}: reading the response: {error}")))?;
    let parsed: Option<Value> = serde_json::from_str(&text).ok();
    if !status.is_success() {
        let detail = parsed
            .as_ref()
            .and_then(provider_error_message)
            .unwrap_or_else(|| truncate(&text, 300));
        return Err(AuthError::Rejected(format!("{url} returned {status}: {detail}")));
    }
    parsed.ok_or_else(|| {
        AuthError::Protocol(format!("{url} did not return JSON: {}", truncate(&text, 300)))
    })
}

/// Pull the human-readable part out of an OAuth error body.
pub fn provider_error_message(body: &Value) -> Option<String> {
    let field = |key: &str| body.get(key).and_then(Value::as_str).map(str::to_string);
    field("error_description")
        .or_else(|| body.get("error").and_then(Value::as_str).map(str::to_string))
        .or_else(|| {
            body.get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| field("message"))
}

/// The machine-readable OAuth error code, when the body has one.
pub fn provider_error_code(body: &Value) -> Option<String> {
    body.get("error")
        .and_then(|error| {
            error
                .as_str()
                .map(str::to_string)
                .or_else(|| error.get("code").and_then(Value::as_str).map(str::to_string))
        })
        .map(|code| code.trim().to_string())
}

fn truncate(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let cut: String = trimmed.chars().take(max).collect();
    format!("{cut}…")
}

/// Decode a JWT payload without verifying it.
///
/// Only ever used to read a claim the provider put there for us (the ChatGPT account id). The token
/// is opaque to luxe otherwise; we are not making a trust decision on it.
pub fn decode_jwt_payload(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(payload))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Milliseconds since the epoch for a token that expires in `expires_in` seconds, minus a safety
/// margin so a token can't die between the check and the request.
pub fn expiry_from_expires_in(expires_in_secs: u64) -> u64 {
    const MARGIN_MS: u64 = 5 * 60 * 1000;
    let lifetime_ms = expires_in_secs.saturating_mul(1000);
    super::credential::now_ms() + lifetime_ms.saturating_sub(MARGIN_MS.min(lifetime_ms / 2))
}

#[cfg(test)]
mod tests;
