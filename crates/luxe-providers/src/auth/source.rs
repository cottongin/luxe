//! Per-request auth — what an adapter puts on the wire for *this* call.
//!
//! An adapter cannot hold a key handed to it at construction: an OAuth access token outlives that
//! assumption by about an hour, so auth is resolved per request: [`AuthSource::request_auth`]
//! returns the headers (and, for GitHub Copilot, the base URL) for one call, refreshing an expiring
//! token on the way if it has to.
//!
//! Two implementations. [`StaticAuth`] is a fixed key in a provider-appropriate header.
//! [`StoreAuth`] reads the credential store, under a key named for the run and over the provider's
//! variables and configured key, so a `/login` mid-session takes effect on the next turn and a
//! rotating token stays fresh without rebuilding the engine.

use std::sync::Arc;

use luxe_core::auth::ApiKind;
use luxe_core::provider::ProviderError;
use tokio::sync::Mutex;

use super::catalog::{provider_descriptor, OauthFlow, ProviderDescriptor};
use super::credential::{now_ms, Credential, OAuthCredential};
use super::store::AuthStore;
use crate::config::{resolve_value_async, ApiKey};

mod key_style;
pub use key_style::KeyStyle;

/// Refresh a token this far ahead of its expiry.
///
/// Long enough that a turn started at the boundary can't have its token die mid-stream; short
/// enough that we aren't refreshing constantly.
const REFRESH_MARGIN_MS: u64 = 5 * 60 * 1000;

/// `key`, once it is remembered for redaction: every credential a request carries passes one of
/// [`RequestAuth`]'s key constructors, so this is where a shapeless one becomes known.
fn remembered(key: &ApiKey) -> &ApiKey {
    luxe_core::redact::process_secrets().add(key.expose());
    key
}

/// The auth a single request should carry.
///
/// Header values are [`ApiKey`]-wrapped whether or not they're secret, so a `Debug` of this struct
/// can never print a token — the cost is that a boring value reads as `[REDACTED]` too.
#[derive(Debug, Clone, Default)]
pub struct RequestAuth {
    /// Headers to set, in order.
    pub headers: Vec<(String, ApiKey)>,
    /// Base URL this credential must be used against, when the provider hands out a
    /// per-account endpoint (Copilot's `proxy-ep`). Overrides the adapter's configured base URL.
    pub base_url: Option<String>,
}

impl RequestAuth {
    /// No auth at all — a local server that doesn't check.
    pub fn none() -> Self {
        Self::default()
    }

    /// `Authorization: Bearer <key>`.
    pub fn bearer(key: &ApiKey) -> Self {
        let key = remembered(key);
        Self {
            headers: vec![(
                "Authorization".to_string(),
                ApiKey::new(format!("Bearer {}", key.expose())),
            )],
            base_url: None,
        }
    }

    /// Anthropic's `x-api-key` (it rejects `Authorization: Bearer`).
    pub fn anthropic_key(key: &ApiKey) -> Self {
        let key = remembered(key);
        Self { headers: vec![("x-api-key".to_string(), key.clone())], base_url: None }
    }

    /// Azure's `api-key`, which every Microsoft Foundry surface accepts.
    pub fn azure_key(key: &ApiKey) -> Self {
        let key = remembered(key);
        Self { headers: vec![("api-key".to_string(), key.clone())], base_url: None }
    }

    /// Both `x-api-key` and `api-key`, for Foundry's Anthropic passthrough.
    ///
    /// The endpoint authenticates on `x-api-key` and its own portal samples show `api-key`, so
    /// Anthropic's `AnthropicFoundry` client sends the pair and so does luxe. One header would
    /// work today against a 401 that costs a user an evening.
    pub fn foundry_key(key: &ApiKey) -> Self {
        let key = remembered(key);
        Self {
            headers: vec![
                ("x-api-key".to_string(), key.clone()),
                ("api-key".to_string(), key.clone()),
            ],
            base_url: None,
        }
    }

    /// Add a non-secret header.
    pub fn with_header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_string(), ApiKey::new(value.into())));
        self
    }

    /// Pin the base URL for this call.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// Apply every header to a request builder.
    pub fn apply(&self, mut request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        for (name, value) in &self.headers {
            request = request.header(name.as_str(), value.expose());
        }
        request
    }

    /// Whether anything was resolved (a keyless local server resolves to nothing).
    pub fn is_empty(&self) -> bool {
        self.headers.is_empty() && self.base_url.is_none()
    }
}

/// Resolves the auth for one provider's requests.
#[async_trait::async_trait]
pub trait AuthSource: Send + Sync + std::fmt::Debug {
    /// Auth for one request. Fails with [`ProviderError::Auth`] when there is no usable credential,
    /// carrying a message that says how to fix it.
    async fn request_auth(&self) -> Result<RequestAuth, ProviderError>;

    /// The stored sign-in behind this source, for a provider that reads account details with it
    /// (GitHub Copilot's plan usage). `None` for a key, or with nothing stored.
    async fn oauth_credential(&self) -> Option<OAuthCredential> {
        None
    }
}

/// A fixed credential, resolved once at construction.
#[derive(Debug, Clone)]
pub struct StaticAuth {
    auth: RequestAuth,
}

impl StaticAuth {
    /// Bearer-token auth (OpenAI-compatible endpoints).
    pub fn bearer(key: Option<ApiKey>) -> Self {
        Self { auth: key.map(|key| RequestAuth::bearer(&key)).unwrap_or_default() }
    }

    /// Anthropic `x-api-key` auth.
    pub fn anthropic(key: Option<ApiKey>) -> Self {
        Self { auth: key.map(|key| RequestAuth::anthropic_key(&key)).unwrap_or_default() }
    }

    /// Auth appropriate to an API kind.
    pub fn for_api(api: ApiKind, key: Option<ApiKey>) -> Self {
        Self::for_style(KeyStyle::for_api(api), key)
    }

    /// Auth in an explicit style, for a provider whose header isn't implied by its API.
    pub fn for_style(style: KeyStyle, key: Option<ApiKey>) -> Self {
        Self { auth: key.map(|key| style.headers(&key)).unwrap_or_default() }
    }

    /// Whatever headers were resolved (used by adapters that also accept a `StoreAuth`).
    pub fn into_arc(self) -> Arc<dyn AuthSource> {
        Arc::new(self)
    }
}

#[async_trait::async_trait]
impl AuthSource for StaticAuth {
    async fn request_auth(&self) -> Result<RequestAuth, ProviderError> {
        Ok(self.auth.clone())
    }
}

/// Credential-store-backed auth for one provider.
///
/// Resolution order per request: the stored credential, then an ambient environment variable, then
/// the fallback supplied by the CLI/config. A stored credential wins so that logging out of a
/// provider doesn't silently fall back to a stale env var the user forgot about — except that an
/// explicit fallback (a `--api-key` flag) is *above* everything, since it was typed for this run.
#[derive(Debug)]
pub struct StoreAuth {
    store: AuthStore,
    provider_id: String,
    /// How a resolved key goes on the wire.
    key_style: KeyStyle,
    /// OAuth flow to use when the stored credential needs refreshing.
    flow: Option<OauthFlow>,
    /// Env var names to consult when nothing is stored.
    env: Vec<String>,
    /// A key from the CLI/config, used when nothing is stored and no env var is set.
    fallback: Option<ApiKey>,
    /// A `--api-key`-style override that beats even a stored credential.
    override_key: Option<ApiKey>,
    /// How long a `!command` key may take to print.
    command_deadline: std::time::Duration,
    /// Whether this provider needs no credential.
    keyless: bool,
    /// Serializes refreshes inside this process; the store's file lock covers other processes.
    refresh_lock: Mutex<()>,
}

impl StoreAuth {
    /// Build a source for `provider_id`, taking defaults from the catalog when it knows the
    /// provider.
    pub fn new(store: AuthStore, provider_id: impl Into<String>, api: ApiKind) -> Self {
        let provider_id = provider_id.into();
        let descriptor = provider_descriptor(&provider_id);
        Self {
            store,
            key_style: descriptor
                .map(ProviderDescriptor::key_style)
                .unwrap_or_else(|| KeyStyle::for_api(api)),
            flow: descriptor.and_then(|descriptor| descriptor.oauth_flow()),
            env: descriptor
                .map(|descriptor| descriptor.env.iter().map(|name| name.to_string()).collect())
                .unwrap_or_default(),
            keyless: descriptor.is_some_and(|descriptor| descriptor.keyless),
            provider_id,
            fallback: None,
            override_key: None,
            command_deadline: luxe_core::secret_command::SECRET_COMMAND_TIMEOUT,
            refresh_lock: Mutex::new(()),
        }
    }

    /// A key from config, used only when nothing is stored and no env var is set.
    pub fn with_fallback(mut self, key: Option<ApiKey>) -> Self {
        self.fallback = key;
        self
    }

    /// A key from the command line, which beats everything.
    pub fn with_override(mut self, key: Option<ApiKey>) -> Self {
        self.override_key = key;
        self
    }

    /// Additional environment variables to consult (a user-declared provider).
    pub fn with_env(mut self, env: Vec<String>) -> Self {
        self.env.extend(env);
        self
    }

    /// Override the header style, for a provider block that names one.
    pub fn with_key_style(mut self, style: Option<KeyStyle>) -> Self {
        if let Some(style) = style {
            self.key_style = style;
        }
        self
    }

    /// Bound a `!command` key by `deadline` instead of the default.
    pub fn with_command_deadline(mut self, deadline: std::time::Duration) -> Self {
        self.command_deadline = deadline;
        self
    }

    /// Treat a missing credential as fine (a local server).
    pub fn keyless(mut self, keyless: bool) -> Self {
        self.keyless = keyless;
        self
    }

    /// Wrap in an `Arc` for an adapter.
    pub fn into_arc(self) -> Arc<dyn AuthSource> {
        Arc::new(self)
    }

    fn env_key(&self) -> Option<ApiKey> {
        self.env
            .iter()
            .filter_map(|name| std::env::var(name).ok())
            .map(|value| value.trim().to_string())
            .find(|value| !value.is_empty())
            .map(ApiKey::new)
    }

    /// Turn a resolved credential into request headers.
    async fn derive(&self, credential: &Credential) -> Result<RequestAuth, ProviderError> {
        match credential {
            Credential::ApiKey(api) => {
                // `$ENV` / `!command` specs are expanded here, at the moment of use — never while
                // listing credentials.
                let resolved = self.resolve(&api.key).await?;
                if resolved.trim().is_empty() {
                    return Err(self.missing("its stored key resolved to nothing"));
                }
                Ok(self.api_key_auth(&ApiKey::new(resolved)))
            }
            Credential::OAuth(oauth) => self.oauth_auth(oauth),
        }
    }

    /// A key spec as its value; a `!command` runs bounded, off the runtime thread.
    async fn resolve(&self, key: &ApiKey) -> Result<String, ProviderError> {
        resolve_value_async(key.expose(), self.command_deadline)
            .await
            .map_err(|reason| self.missing(&reason))
    }

    /// An override or fallback spec as a key, `None` when absent or when it expands to nothing.
    ///
    /// Resolved on every call with nothing cached, so a short-lived token from a `!command` is
    /// fresh for each request. A local server needs no key, so a spec that will not resolve is no
    /// reason to stop there.
    async fn resolve_spec(&self, spec: Option<&ApiKey>) -> Result<Option<ApiKey>, String> {
        let Some(spec) = spec else { return Ok(None) };
        match resolve_value_async(spec.expose(), self.command_deadline).await {
            Ok(value) if !value.trim().is_empty() => Ok(Some(ApiKey::new(value))),
            Err(reason) if !self.keyless => Err(reason),
            _ => Ok(None),
        }
    }

    /// The override, resolved. Signing in would not help when it fails, since it outranks the
    /// store, so its error says which key it was rather than pointing at `luxe auth login`.
    async fn resolved_override(&self) -> Result<Option<ApiKey>, ProviderError> {
        self.resolve_spec(self.override_key.as_ref()).await.map_err(|reason| {
            ProviderError::Auth(format!(
                "the key given for '{}' on this run could not be read — {reason}",
                self.provider_id
            ))
        })
    }

    fn api_key_auth(&self, key: &ApiKey) -> RequestAuth {
        self.key_style.headers(key)
    }

    /// OAuth headers, which are provider-specific beyond the bearer token.
    fn oauth_auth(&self, oauth: &OAuthCredential) -> Result<RequestAuth, ProviderError> {
        let mut auth = RequestAuth::bearer(&oauth.access);
        match self.flow {
            Some(OauthFlow::OpenaiCodex) => {
                let account = oauth
                    .account
                    .clone()
                    .or_else(|| super::oauth::openai_codex::account_id(oauth.access.expose()))
                    .ok_or_else(|| {
                        self.missing("its ChatGPT credential carries no account id; sign in again")
                    })?;
                auth = auth
                    .with_header("chatgpt-account-id", account)
                    .with_header("OpenAI-Beta", "responses=experimental")
                    .with_header("originator", super::oauth::openai_codex::originator());
            }
            Some(OauthFlow::GithubCopilot) => {
                for (name, value) in super::oauth::github_copilot::EDITOR_HEADERS {
                    auth = auth.with_header(name, *value);
                }
                let base = oauth.endpoint.clone().unwrap_or_else(|| {
                    super::oauth::github_copilot::api_base_url(
                        oauth.access.expose(),
                        oauth.enterprise_domain.as_deref(),
                    )
                });
                auth = auth.with_base_url(base);
            }
            // OpenRouter's flow mints a plain API key, so it never lands here.
            Some(OauthFlow::OpenRouter) | None => {}
        }
        Ok(auth)
    }

    /// Refresh the stored OAuth credential if it's within the margin of expiry.
    ///
    /// Double-checked: the cheap check happens outside the lock, the authoritative one inside, so
    /// concurrent turns produce exactly one refresh and the rotated token is persisted before any of
    /// them reads it.
    async fn refreshed(&self, current: OAuthCredential) -> Result<OAuthCredential, ProviderError> {
        if current.expires_at_ms == 0 || now_ms() + REFRESH_MARGIN_MS < current.expires_at_ms {
            return Ok(current);
        }
        let Some(flow) = self.flow else { return Ok(current) };

        let _guard = self.refresh_lock.lock().await;
        // Re-read under the lock: another task may have refreshed while we waited.
        let latest = self
            .store
            .credential(&self.provider_id)
            .and_then(|credential| credential.as_oauth().cloned())
            .unwrap_or(current);
        if latest.expires_at_ms != 0 && now_ms() + REFRESH_MARGIN_MS < latest.expires_at_ms {
            return Ok(latest);
        }

        tracing::debug!(provider = %self.provider_id, "refreshing an expiring OAuth token");
        let refreshed = super::oauth::refresh(flow, &latest).await.map_err(|error| {
            ProviderError::Auth(format!(
                "{}: could not refresh the stored credential ({error}) — run `luxe auth login {}`",
                self.provider_id, self.provider_id
            ))
        })?;
        if let Err(error) = self.store.put(&self.provider_id, &Credential::OAuth(refreshed.clone()))
        {
            // A token we can't persist still works for this request; the next process pays again.
            tracing::warn!(provider = %self.provider_id, %error, "could not persist refreshed token");
        }
        Ok(refreshed)
    }

    fn missing(&self, reason: &str) -> ProviderError {
        ProviderError::Auth(format!(
            "no credential for '{provider}' — {reason}. Run `luxe auth login {provider}` (or \
             `/login {provider}` in the TUI).",
            provider = self.provider_id
        ))
    }
}

#[async_trait::async_trait]
impl AuthSource for StoreAuth {
    async fn oauth_credential(&self) -> Option<OAuthCredential> {
        // a key typed for this run outranks the store, so the store's sign-in is not this run's,
        // unless that key expands to nothing and `request_auth` falls through to the store too
        if !matches!(self.resolved_override().await, Ok(None)) {
            return None;
        }
        self.store
            .credential(&self.provider_id)
            .and_then(|credential| credential.as_oauth().cloned())
    }

    async fn request_auth(&self) -> Result<RequestAuth, ProviderError> {
        // An explicit command-line key is the user speaking about this run specifically.
        if let Some(key) = self.resolved_override().await? {
            return Ok(self.api_key_auth(&key));
        }
        if let Some(credential) = self.store.credential(&self.provider_id) {
            return match credential {
                Credential::OAuth(oauth) => {
                    let fresh = self.refreshed(oauth).await?;
                    self.derive(&Credential::OAuth(fresh)).await
                }
                api_key => self.derive(&api_key).await,
            };
        }
        if let Some(key) = self.env_key() {
            return Ok(self.api_key_auth(&key));
        }
        if let Some(key) = self
            .resolve_spec(self.fallback.as_ref())
            .await
            .map_err(|reason| self.missing(&reason))?
        {
            return Ok(self.api_key_auth(&key));
        }
        if self.keyless {
            return Ok(RequestAuth::none());
        }
        Err(self.missing("nothing is stored and no environment variable is set"))
    }
}

#[cfg(test)]
mod tests;
