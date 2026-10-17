//! Grants — the token endpoint: a code exchange, client credentials and a refresh.

use serde_json::Value;

use super::endpoints::require_secure;
use super::{now_secs, Client, OAuthClient, OAuthError, Pending};
use crate::token_store::StoredToken;

impl OAuthClient {
    /// Exchange an authorization code for tokens.
    pub(super) async fn exchange_code(
        &self,
        code: &str,
        pending: &Pending,
    ) -> Result<StoredToken, OAuthError> {
        let mut form = vec![
            ("grant_type", "authorization_code".to_string()),
            ("code", code.to_string()),
            ("redirect_uri", pending.redirect_uri.clone()),
            ("client_id", pending.client.id.clone()),
            ("code_verifier", pending.verifier.clone()),
            ("resource", pending.resource.clone()),
        ];
        if let Some(secret) = &pending.client.secret {
            form.push(("client_secret", secret.clone()));
        }
        self.token_request(&pending.token_endpoint, &form, &pending.client, &pending.resource).await
    }

    /// The client-credentials grant.
    pub(super) async fn client_credentials(&self) -> Result<String, OAuthError> {
        let resolved = self.resolve().await?;
        let token_endpoint = resolved
            .endpoints
            .token
            .ok_or_else(|| OAuthError::Discovery("no token endpoint".into()))?;
        let id = self
            .config
            .client_id
            .clone()
            .ok_or_else(|| OAuthError::Config("client_credentials needs a client_id".into()))?;
        let client = Client { id, secret: self.configured_secret().await?, issued_secret: None };
        let mut form = vec![
            ("grant_type", "client_credentials".to_string()),
            ("client_id", client.id.clone()),
            ("resource", resolved.resource.clone()),
        ];
        if let Some(secret) = &client.secret {
            form.push(("client_secret", secret.clone()));
        }
        if let Some(scope) = &self.config.scope {
            form.push(("scope", scope.clone()));
        }
        let token = self.token_request(&token_endpoint, &form, &client, &resolved.resource).await?;
        self.store.put(&self.server_id, token.clone())?;
        Ok(token.access_token.clone())
    }

    /// Refresh an access token; stores and returns the new access token.
    ///
    /// Against the token endpoint and resource the token was issued for, when it recorded them:
    /// a refresh that re-ran discovery could land somewhere the token never came from.
    pub(super) async fn refresh(
        &self,
        refresh_token: &str,
        prior: &StoredToken,
    ) -> Result<String, OAuthError> {
        let (token_endpoint, resource) = match (&prior.token_endpoint, &prior.resource) {
            (Some(endpoint), Some(resource)) => {
                require_secure(endpoint, "the token endpoint")?;
                (endpoint.clone(), resource.clone())
            }
            _ => {
                let resolved = self.resolve().await?;
                let token = resolved
                    .endpoints
                    .token
                    .ok_or_else(|| OAuthError::Discovery("no token endpoint".into()))?;
                (token, resolved.resource)
            }
        };
        let id = self
            .config
            .client_id
            .clone()
            .or_else(|| prior.client_id.clone())
            .ok_or_else(|| OAuthError::Config("no client_id for refresh".into()))?;
        // A configured client's secret comes from the config; only a registered one's is stored
        let issued =
            if self.config.client_id.is_some() { None } else { prior.client_secret.clone() };
        let secret = self.configured_secret().await?.or_else(|| issued.clone());
        let client = Client { id, secret, issued_secret: issued };
        let mut form = vec![
            ("grant_type", "refresh_token".to_string()),
            ("refresh_token", refresh_token.to_string()),
            ("client_id", client.id.clone()),
            ("resource", resource.clone()),
        ];
        if let Some(secret) = &client.secret {
            form.push(("client_secret", secret.clone()));
        }
        let mut token = self.token_request(&token_endpoint, &form, &client, &resource).await?;
        // Providers may omit a new refresh token; keep the old one.
        if token.refresh_token.is_none() {
            token.refresh_token = Some(refresh_token.to_string());
        }
        let access = token.access_token.clone();
        self.store.put(&self.server_id, token)?;
        Ok(access)
    }

    /// POST a token request and parse the response into a [`StoredToken`], recording where it came
    /// from and carrying a registration's issued secret forward.
    async fn token_request(
        &self,
        endpoint: &str,
        form: &[(&str, String)],
        client: &Client,
        resource: &str,
    ) -> Result<StoredToken, OAuthError> {
        use OAuthError::{Network, Refused};
        let resp = self
            .http
            .post(endpoint)
            .form(form)
            .send()
            .await
            .map_err(|e| Network(format!("token request failed: {e}")))?;
        let status = resp.status();
        // An error status is a refusal whatever its body, so the body is read leniently for it
        let doc: Result<Value, _> = resp.json().await;
        if !status.is_success() {
            let doc = doc.unwrap_or_default();
            let err = doc.get("error").and_then(Value::as_str).unwrap_or(status.as_str());
            return Err(Refused(match doc.get("error_description").and_then(Value::as_str) {
                Some(description) => format!("token endpoint error: {err} — {description}"),
                None => format!("token endpoint error: {err}"),
            }));
        }
        let doc = doc.map_err(|e| Network(format!("bad token response: {e}")))?;
        let access_token = doc
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(|| Network("token response missing access_token".into()))?
            .to_string();
        let refresh_token = doc.get("refresh_token").and_then(Value::as_str).map(str::to_string);
        let expires_at_secs =
            doc.get("expires_in").and_then(Value::as_u64).map(|secs| now_secs() + secs);
        Ok(StoredToken {
            access_token,
            refresh_token,
            expires_at_secs,
            client_id: Some(client.id.clone()),
            client_secret: client.issued_secret.clone(),
            token_endpoint: Some(endpoint.to_string()),
            resource: Some(resource.to_string()),
        })
    }
}
