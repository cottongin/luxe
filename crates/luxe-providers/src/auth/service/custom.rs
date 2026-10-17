//! The custom endpoint — a wizard entry that becomes a provider once the user names it.

use luxe_core::auth::{ApiKind, AuthError, AuthInteraction, AuthPrompt};
use secrecy::ExposeSecret;

use super::models::normalize_base_url;
use super::{ProviderAuth, ProviderDescriptor};
use crate::auth::catalog::provider_descriptor;

impl ProviderAuth {
    /// Resolve the custom-endpoint pseudo-provider into a real one the user names.
    pub(super) async fn resolve_custom(
        &self,
        interaction: &dyn AuthInteraction,
    ) -> Result<(ProviderDescriptor, String), AuthError> {
        let id_answer = interaction
            .prompt(AuthPrompt::text(
                "Name for this provider (letters, digits, - and _):",
                Some("my-endpoint".to_string()),
            ))
            .await?;
        let id = id_answer.expose_secret().trim().to_ascii_lowercase();
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(AuthError::Input(
                "use letters, digits, dashes or underscores for the provider name".to_string(),
            ));
        }
        if provider_descriptor(&id).is_some() {
            return Err(AuthError::Input(format!(
                "'{id}' is already a built-in provider — run `luxe auth login {id}` instead"
            )));
        }

        let url_answer = interaction
            .prompt(AuthPrompt::text(
                "Base URL, including the version segment:",
                Some("https://host/v1".to_string()),
            ))
            .await?;
        let base_url = normalize_base_url(url_answer.expose_secret())?;

        let api_answer = interaction
            .prompt(AuthPrompt::select(
                "Which API does it speak?",
                vec![
                    luxe_core::auth::AuthChoice {
                        id: ApiKind::OpenaiCompletions.as_str().to_string(),
                        label: "OpenAI /chat/completions".to_string(),
                        detail: Some("vLLM, llama.cpp, Ollama, most proxies".to_string()),
                    },
                    luxe_core::auth::AuthChoice {
                        id: ApiKind::OpenaiResponses.as_str().to_string(),
                        label: "OpenAI /responses".to_string(),
                        detail: Some("Azure OpenAI, newer OpenAI-compatible gateways".to_string()),
                    },
                    luxe_core::auth::AuthChoice {
                        id: ApiKind::AnthropicMessages.as_str().to_string(),
                        label: "Anthropic /messages".to_string(),
                        detail: Some("An Anthropic-compatible gateway".to_string()),
                    },
                ],
            ))
            .await?;
        let api = ApiKind::parse(api_answer.expose_secret())
            .ok_or_else(|| AuthError::Input("unknown API kind".to_string()))?;

        let descriptor = ProviderDescriptor::custom(id, api);
        Ok((descriptor, base_url))
    }
}
