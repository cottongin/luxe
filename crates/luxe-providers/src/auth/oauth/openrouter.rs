//! OpenRouter — the one subscription-style flow a provider actually publishes for this purpose.
//!
//! OpenRouter's documented PKCE flow exists so a third-party app can mint an API key **the user
//! owns**, billed from their OpenRouter credits and revocable from their dashboard. The result is a
//! durable key, so it is stored as an api-key credential rather than a token pair — there is nothing
//! to refresh.

use luxe_core::auth::{AuthError, AuthEvent, AuthInteraction};
use serde_json::Value;

use super::super::credential::Credential;
use super::{await_code, post_json, CallbackServer};

const AUTHORIZE_URL: &str = "https://openrouter.ai/auth";
const KEYS_URL: &str = "https://openrouter.ai/api/v1/auth/keys";
const CALLBACK_PATH: &str = "/openrouter/callback";

/// Where the flow talks to OpenRouter. Split out so tests can drive the real code locally.
#[derive(Debug, Clone)]
pub struct Endpoints {
    /// Authorization page the browser opens.
    pub authorize: String,
    /// Exchanges the code for a user-owned API key.
    pub keys: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self { authorize: AUTHORIZE_URL.to_string(), keys: KEYS_URL.to_string() }
    }
}

/// Run the flow and return the minted key.
pub async fn login(interaction: &dyn AuthInteraction) -> Result<Credential, AuthError> {
    login_with(&Endpoints::default(), interaction).await
}

/// Run the flow against specific endpoints.
pub async fn login_with(
    endpoints: &Endpoints,
    interaction: &dyn AuthInteraction,
) -> Result<Credential, AuthError> {
    let pkce = super::pkce();
    // OpenRouter accepts any callback URL, so take a free port, and a path no page can guess: with
    // no `state` in this flow, the path is what keeps a forged answer from ending it.
    let path = format!("{CALLBACK_PATH}/{}", super::random_state());
    let mut server = CallbackServer::bind(0, &path, None).await?;
    let redirect_uri = server.redirect_uri();

    let mut url = reqwest::Url::parse(&endpoints.authorize)
        .map_err(|error| AuthError::Protocol(format!("bad authorize URL: {error}")))?;
    url.query_pairs_mut()
        .append_pair("callback_url", &redirect_uri)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256");

    interaction.notify(AuthEvent::OpenUrl {
        url: url.to_string(),
        instructions: Some(
            "Approve the request to create an API key. It stays yours — revoke it any time at \
             openrouter.ai/keys."
                .to_string(),
        ),
    });

    let callback = await_code(&mut server, interaction, None).await?;
    interaction.notify(AuthEvent::Progress { message: "Creating the API key…".to_string() });

    let body = post_json(
        &endpoints.keys,
        serde_json::json!({
            "code": callback.code,
            "code_verifier": pkce.verifier,
            "code_challenge_method": "S256",
        }),
    )
    .await?;

    let key = body
        .get("key")
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty())
        .ok_or_else(|| AuthError::Protocol("OpenRouter returned no API key".to_string()))?;
    Ok(Credential::api_key(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_authorize_url_matches_openrouters_documented_parameters() {
        let mut url = reqwest::Url::parse(AUTHORIZE_URL).expect("constant");
        url.query_pairs_mut()
            .append_pair("callback_url", "http://localhost:5000/openrouter/callback")
            .append_pair("code_challenge", "chal")
            .append_pair("code_challenge_method", "S256");
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(pairs["callback_url"], "http://localhost:5000/openrouter/callback");
        assert_eq!(pairs["code_challenge_method"], "S256");
        assert!(url.as_str().starts_with("https://openrouter.ai/auth?"));
    }

    /// The point of this flow is a durable, user-owned key — storing it as an OAuth token pair would
    /// make the runtime try (and fail) to refresh it.
    #[test]
    fn the_minted_credential_is_an_api_key_not_a_token_pair() {
        let credential = Credential::api_key("sk-or-v1-minted");
        assert_eq!(credential.kind(), luxe_core::auth::CredentialKind::ApiKey);
        assert!(credential.as_oauth().is_none());
    }
}
