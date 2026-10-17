//! OAuth 2.1 client for hosted MCP servers.
//!
//! Covers the MCP auth story: discovery (RFC 9728 protected-resource metadata, then RFC 8414
//! authorization-server metadata), dynamic client registration (RFC 7591) for servers that allow
//! it and a configured client for those that do not, resource indicators (RFC 8707), the
//! authorization-code + PKCE flow (interactive loopback *and* a headless start/complete pair), the
//! client-credentials flow, and refresh. Tokens land in the `0600`
//! [`crate::token_store::TokenStore`]; secrets are never logged.
//!
//! # Discovery
//!
//! The order is the specification's, and it matters more than it looks. A host can serve metadata
//! for more than one server: Asana's MCP host still publishes, at its root, the authorization server
//! of a beta it retired, while its current server's metadata sits at the path the specification
//! names. Reading the root first signs in to the wrong server. So the path the server itself points
//! at wins, then the path form, then the root, and the old origin lookup only when a server
//! publishes no protected-resource metadata at all. What is found is checked before it is used:
//! a protected-resource document must name the server it was fetched for, and an authorization
//! server must be `https` and say it is who it was asked for, so a hostile document cannot send a
//! token anywhere else.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use luxe_core::mcp_client::{OAuthConfig, OAuthGrant};
use luxe_core::oauth_loopback::{pkce, random_state, LoopbackCallback, Pkce};

use crate::token_store::TokenStore;

mod discovery;
mod endpoints;
mod error;
mod grants;
mod registration;

pub(crate) use endpoints::require_secure;
use endpoints::Endpoints;
pub use error::OAuthError;

/// How long a login waits for the browser to come back before giving up.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How long one discovery or token request may take. Without it a server that accepts the
/// connection and never answers holds a login open forever.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Endpoints plus the resource indicator the tokens are for.
#[derive(Debug, Clone)]
struct Resolved {
    endpoints: Endpoints,
    /// The protected resource's own identifier when it published one, else the server URL
    resource: String,
}

/// Who the client is when it asks for a token.
#[derive(Clone)]
struct Client {
    id: String,
    /// What is sent, expanded
    secret: Option<String>,
    /// A secret registration issued, which the token store keeps. A secret from the config is
    /// never copied there: it belongs in the config, or the environment variable the config names.
    issued_secret: Option<String>,
}

/// In-flight authorization-code state between `auth_start` and `auth_complete`.
struct Pending {
    verifier: String,
    state: String,
    redirect_uri: String,
    token_endpoint: String,
    resource: String,
    client: Client,
}

/// What `access_token` resolved to.
pub enum TokenState {
    /// A usable bearer token.
    Have(String),
    /// Interactive login is required (reason is user-facing).
    NeedsLogin(String),
    /// A sign-in is stored but could not be refreshed, for a reason signing in again would not
    /// fix — a secret that did not resolve, a server that could not be reached (user-facing).
    Unavailable(String),
}

/// OAuth client bound to one MCP server.
pub struct OAuthClient {
    server_id: String,
    server_url: String,
    config: OAuthConfig,
    http: reqwest::Client,
    store: TokenStore,
    pending: Mutex<Option<Pending>>,
    /// Whether a `!command` client secret may run: only for a server from the reader's own config
    secret_commands: bool,
    /// Held while a token is read and refreshed, so calls finding it expired refresh it once
    refreshing: tokio::sync::Mutex<()>,
}

impl OAuthClient {
    /// Build a client for `server_id` (reached at `server_url`), using the default token store.
    pub fn new(server_id: String, server_url: String, config: OAuthConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            server_id,
            server_url,
            config,
            http,
            store: TokenStore::default_path(),
            pending: Mutex::new(None),
            secret_commands: false,
            refreshing: tokio::sync::Mutex::new(()),
        }
    }

    /// Lets a `!command` client secret run, for a server declared in the reader's own config.
    pub fn with_secret_commands(mut self, allowed: bool) -> Self {
        self.secret_commands = allowed;
        self
    }

    /// Override the token store (tests).
    pub fn with_store(mut self, store: TokenStore) -> Self {
        self.store = store;
        self
    }

    /// Return a usable access token: stored-and-valid, refreshed, freshly fetched
    /// (client-credentials), or what stands in the way.
    ///
    /// A refresh that fails says why, and only a refusal from the server sends the reader back to
    /// `luxe mcp login`. A secret that did not resolve is not "needs authorization": signing in
    /// again would fail on the same missing variable.
    pub async fn access_token(&self) -> TokenState {
        // the second caller reads what the first stored, since a rotating server refuses an old
        // refresh token the second time
        let _one_at_a_time = self.refreshing.lock().await;
        // An entry with no access token is a remembered registration from a login that never
        // finished, and sending it would be a bearer header with nothing in it
        if let Some(token) = self.store.get(&self.server_id).filter(|t| !t.access_token.is_empty())
        {
            if !token.is_expired() {
                return TokenState::Have(crate::secret::remembered(&token.access_token));
            }
            if let Some(refresh) = token.refresh_token.as_deref().map(crate::secret::remembered) {
                match self.refresh(&refresh, &token).await {
                    Ok(fresh) => return TokenState::Have(crate::secret::remembered(&fresh)),
                    // Only a refusal is cured by signing in again; a secret that did not resolve,
                    // a missing client id or the network would fail the same way
                    Err(OAuthError::Refused(reason)) => {
                        tracing::warn!(server = %self.server_id, %reason, "mcp: token refresh refused");
                        return TokenState::NeedsLogin(format!(
                            "MCP server '{}' needs authorization — the refresh was refused \
                             ({reason}); run `luxe mcp login --id {}`.",
                            self.server_id, self.server_id
                        ));
                    }
                    Err(reason) => {
                        tracing::warn!(server = %self.server_id, %reason, "mcp: could not refresh the token");
                        return TokenState::Unavailable(format!(
                            "MCP server '{}' could not refresh its sign-in: {reason}",
                            self.server_id
                        ));
                    }
                }
            }
        }
        if self.config.grant_type == OAuthGrant::ClientCredentials {
            return match self.client_credentials().await {
                Ok(token) => TokenState::Have(crate::secret::remembered(&token)),
                Err(error) => TokenState::NeedsLogin(error.to_string()),
            };
        }
        TokenState::NeedsLogin(format!(
            "MCP server '{}' needs authorization — run `luxe mcp login --id {}`.",
            self.server_id, self.server_id
        ))
    }

    /// Begin an authorization-code login: resolve endpoints, register a client if needed, and
    /// return the authorize URL the user opens. `redirect_uri` is where the provider will send the
    /// code (a loopback the caller listens on, or the configured pre-registered URI).
    pub async fn auth_start(&self, redirect_uri: &str) -> Result<String, OAuthError> {
        let resolved = self.resolve().await?;
        let authorization = resolved.endpoints.authorization.clone().ok_or_else(|| {
            OAuthError::Discovery("no authorization endpoint (set it or enable discovery)".into())
        })?;
        let token = resolved
            .endpoints
            .token
            .clone()
            .ok_or_else(|| OAuthError::Discovery("no token endpoint".into()))?;
        let client = self.ensure_client(&resolved.endpoints, redirect_uri).await?;

        let Pkce { verifier, challenge } = pkce();
        let state = random_state();
        let mut url = url::Url::parse(&authorization)
            .map_err(|e| OAuthError::Discovery(format!("bad authorization endpoint: {e}")))?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("response_type", "code");
            q.append_pair("client_id", &client.id);
            q.append_pair("redirect_uri", redirect_uri);
            q.append_pair("state", &state);
            q.append_pair("code_challenge", &challenge);
            q.append_pair("code_challenge_method", "S256");
            q.append_pair("resource", &resolved.resource);
            if let Some(scope) = &self.config.scope {
                q.append_pair("scope", scope);
            }
        }
        *self.pending.lock().expect("oauth pending poisoned") = Some(Pending {
            verifier,
            state,
            redirect_uri: redirect_uri.to_string(),
            token_endpoint: token,
            resource: resolved.resource,
            client,
        });
        Ok(url.to_string())
    }

    /// Complete an authorization-code login from the provider's redirect URL (contains `code` +
    /// `state`, or `error`). Validates `state`, exchanges the code, and stores the token.
    pub async fn auth_complete(&self, redirect_url: &str) -> Result<(), OAuthError> {
        let pending =
            self.pending.lock().expect("oauth pending poisoned").take().ok_or_else(|| {
                OAuthError::Config("no login in progress — call auth_start first".into())
            })?;
        let parsed = url::Url::parse(redirect_url)
            .map_err(|e| OAuthError::Config(format!("bad redirect url: {e}")))?;
        let field =
            |name: &str| parsed.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned());
        // Checked first: a redirect that does not carry our state is not an answer to our request,
        // whatever else it says
        if field("state").as_deref() != Some(pending.state.as_str()) {
            return Err(OAuthError::StateMismatch);
        }
        if let Some(error) = field("error") {
            return Err(OAuthError::Refused(match field("error_description") {
                Some(description) => format!("the server refused: {error} — {description}"),
                None => format!("the server refused: {error}"),
            }));
        }
        let code =
            field("code").ok_or_else(|| OAuthError::Config("redirect url has no `code`".into()))?;
        let token = self.exchange_code(&code, &pending).await?;
        Ok(self.store.put(&self.server_id, token)?)
    }

    /// Interactive loopback login for the CLI: print the authorize URL, wait for the browser to
    /// come back, exchange, and store the token.
    pub async fn interactive_login(&self) -> Result<(), OAuthError> {
        let server = self.server_id.clone();
        self.login_with(
            |url, redirect| {
                println!("\nOpen this URL to authorize '{server}':\n\n  {url}\n");
                println!("Waiting for the browser redirect on {redirect} …");
            },
            LOGIN_TIMEOUT,
        )
        .await?;
        println!("Authorized '{}'. Token stored.", self.server_id);
        Ok(())
    }

    /// A loopback login that hands the authorize URL to `show` rather than printing it.
    ///
    /// Args: `(show, timeout)` — `show` gets `(authorize URL, redirect URI)`. The page the browser
    /// lands on is written after the exchange, so it says whether signing in worked rather than
    /// promising it did.
    pub async fn login_with(
        &self,
        show: impl FnOnce(&str, &str),
        timeout: Duration,
    ) -> Result<(), OAuthError> {
        let redirect = self.config.redirect_uri.as_deref().unwrap_or("http://127.0.0.1:0/callback");
        let callback = LoopbackCallback::bind(redirect)
            .await
            .map_err(|e| OAuthError::Callback(e.to_string()))?;
        let url = self.auth_start(callback.redirect_uri()).await?;
        show(&url, callback.redirect_uri());
        let state =
            self.pending.lock().expect("oauth pending poisoned").as_ref().map(|p| p.state.clone());
        let arrival = tokio::time::timeout(timeout, callback.wait(state.as_deref()))
            .await
            .map_err(|_| {
                OAuthError::Callback(format!(
                    "no answer from the browser within {} seconds — run the login again",
                    timeout.as_secs()
                ))
            })?
            .map_err(|e| OAuthError::Callback(e.to_string()))?;
        let outcome = self.auth_complete(&arrival.redirect_url()).await;
        let (status, title, message) = landing(&self.server_id, &outcome);
        arrival.answer(status, title, &message).await;
        outcome
    }
}

/// What the browser tab says once the exchange has run, so a refusal does not read as success:
/// `(status, title, message)`, for [`luxe_core::oauth_loopback::Arrival::answer`] to escape.
fn landing(server: &str, outcome: &Result<(), OAuthError>) -> (u16, &'static str, String) {
    match outcome {
        Ok(()) => {
            (200, "Signed in", format!("Luxe signed in to {server}. You can close this tab."))
        }
        Err(error) => (
            400,
            "Luxe couldn't finish the sign-in",
            format!("Luxe could not sign in to {server}: {error}"),
        ),
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests;
