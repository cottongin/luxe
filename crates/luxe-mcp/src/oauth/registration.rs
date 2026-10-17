//! Registration — which client the login is, and registering one when it has none.

use serde_json::Value;

use super::endpoints::Endpoints;
use super::{Client, OAuthClient, OAuthError};
use crate::secret::SecretError;

impl OAuthClient {
    /// Use the configured client id, a previously-registered one from the store, or register a new
    /// client dynamically (RFC 7591).
    pub(super) async fn ensure_client(
        &self,
        endpoints: &Endpoints,
        redirect_uri: &str,
    ) -> Result<Client, OAuthError> {
        if let Some(id) = self.config.client_id.clone() {
            return Ok(Client { id, secret: self.configured_secret().await?, issued_secret: None });
        }
        if let Some(token) = self.store.get(&self.server_id) {
            if let Some(id) = token.client_id.clone() {
                let issued = token.client_secret.clone();
                return Ok(Client { id, secret: issued.clone(), issued_secret: issued });
            }
        }
        let endpoint = endpoints.registration.clone().ok_or_else(|| {
            OAuthError::Config(
                "no client_id configured and no dynamic registration endpoint — register a \
                 client with the provider and set `client_id` (and `client_secret`) in the \
                 server's auth"
                    .into(),
            )
        })?;
        self.register_client(&endpoint, redirect_uri).await
    }

    /// The configured secret, resolved ([`crate::secret::resolve_secret`]), or `None` when there
    /// is none.
    ///
    /// The one place a configured secret is read, so every flow — the code exchange, a refresh,
    /// client credentials — sends the same thing, with `${VAR}` expanded rather than the reference
    /// itself.
    pub(super) async fn configured_secret(&self) -> Result<Option<String>, SecretError> {
        let Some(secret) = &self.config.client_secret else {
            return Ok(None);
        };
        let what = format!("the client secret for '{}'", self.server_id);
        crate::secret::resolve_secret(secret.as_written(), self.secret_commands, &what)
            .await
            .map(Some)
    }

    /// Dynamic client registration (RFC 7591). Persists the issued client id/secret in the store.
    async fn register_client(
        &self,
        endpoint: &str,
        redirect_uri: &str,
    ) -> Result<Client, OAuthError> {
        let body = serde_json::json!({
            "client_name": "luxe",
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        });
        let resp = self
            .http
            .post(endpoint)
            .json(&body)
            .send()
            .await
            .map_err(|e| OAuthError::Network(format!("client registration failed: {e}")))?;
        if !resp.status().is_success() {
            return Err(OAuthError::Refused(format!(
                "client registration returned HTTP {}",
                resp.status()
            )));
        }
        let doc: Value = resp
            .json()
            .await
            .map_err(|e| OAuthError::Network(format!("bad registration response: {e}")))?;
        let client_id = doc
            .get("client_id")
            .and_then(Value::as_str)
            .ok_or_else(|| OAuthError::Network("registration response missing client_id".into()))?
            .to_string();
        let client_secret = doc.get("client_secret").and_then(Value::as_str).map(str::to_string);
        // Remember the registration so we don't re-register on every login.
        let mut token = self.store.get(&self.server_id).unwrap_or_default();
        token.client_id = Some(client_id.clone());
        token.client_secret = client_secret.clone();
        if let Err(error) = self.store.put(&self.server_id, token) {
            tracing::warn!(server = %self.server_id, %error, "mcp: could not remember the registered client");
        }
        Ok(Client { id: client_id, secret: client_secret.clone(), issued_secret: client_secret })
    }
}
