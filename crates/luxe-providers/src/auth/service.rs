//! The login engine — [`ProviderAuth`], luxe's [`AuthService`].
//!
//! One implementation drives every surface: `luxe auth login`, the TUI's `/login` wizard, and the
//! credential-less first-run path all call the same three methods. The front-end only supplies an
//! [`AuthInteraction`] (how to ask a question and show progress); everything about *what* to ask,
//! which endpoint to call, and what to persist lives here.
//!
//! A login always ends in one of three states: a credential in the store, a typed
//! [`AuthError`], or [`AuthError::Cancelled`] because the user backed out. Nothing is written
//! before the credential is known-good.
//!
//! The login flows live here; `status` resolves what each provider has, `models` talks to an
//! endpoint's model list, `refresh` re-asks local servers and Azure resources in the background,
//! and `custom` turns the wizard's custom entry into a provider.

use std::sync::Arc;

use luxe_core::auth::{
    AuthError, AuthEvent, AuthInteraction, AuthLink, AuthPrompt, AuthService, CredentialKind,
    LoginOutcome, ProviderOption,
};
use secrecy::ExposeSecret;

use super::catalog::{canonical_id, provider_descriptor, ProviderDescriptor};
use super::config_file::ConfigFile;
use super::credential::Credential;
use super::deployments;
use super::source::RequestAuth;
use super::store::AuthStore;
use crate::config::{resolve_key, ApiKey};
use models::{describe_models, normalize_base_url, verify_credential, warn_if_sent_in_the_clear};
use refresh::discover_with_key;

mod custom;
mod models;
mod refresh;
mod status;

/// The key a user typed, as stored and as it resolves right now.
struct Key {
    /// Verbatim — a `$ENV` or `!command` spec stays one, which is the point of it.
    spec: String,
    resolved: String,
}

/// What verification learned: what to tell the user, and what the resource has deployed when it
/// said.
#[derive(Default)]
struct Checked<'a> {
    notes: Vec<String>,
    discovered: Option<Vec<(&'a str, deployments::Served)>>,
}

/// luxe's [`AuthService`]: catalog + credential store + the OAuth flows.
#[derive(Debug, Clone)]
pub struct ProviderAuth {
    store: AuthStore,
    config: ConfigFile,
    /// Whether to make a verification request after storing an API key.
    verify: bool,
}

impl Default for ProviderAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderAuth {
    /// Use the default config locations.
    pub fn new() -> Self {
        Self { store: AuthStore::default_path(), config: ConfigFile::default_path(), verify: true }
    }

    /// Point at explicit files (tests, alternate profiles).
    pub fn with_paths(store: AuthStore, config: ConfigFile) -> Self {
        Self { store, config, verify: true }
    }

    /// Skip the post-login verification request (`--no-verify`, air-gapped setups).
    pub fn without_verification(mut self) -> Self {
        self.verify = false;
        self
    }

    /// The credential store behind this service.
    pub fn store(&self) -> &AuthStore {
        &self.store
    }

    /// Store a credential, reporting a storage failure as such rather than as a login failure.
    fn persist(&self, provider_id: &str, credential: &Credential) -> Result<(), AuthError> {
        self.store
            .put(provider_id, credential)
            .map_err(|error| AuthError::Storage(error.to_string()))
    }

    /// Ask for a key, verify it, store it.
    ///
    /// `resource_root` is the endpoint a provider that needs one was given, already reduced to the
    /// root each of its APIs hangs off — for a lone provider that is simply its base URL.
    async fn login_api_key(
        &self,
        descriptor: &ProviderDescriptor,
        resource_root: Option<String>,
        interaction: &dyn AuthInteraction,
    ) -> Result<LoginOutcome, AuthError> {
        // Before the key is asked for, so a user who would rather not can still back out
        if let Some(root) = resource_root.as_deref() {
            warn_if_sent_in_the_clear(root, interaction);
        }
        let key = ask_for_key(descriptor, interaction).await?;
        let mut checked = self
            .verify_or_discover(descriptor, resource_root.as_deref(), &key, interaction)
            .await?;
        let siblings = self.persist_with_siblings(
            descriptor,
            &Credential::api_key(&key.spec),
            resource_root.as_deref(),
            &checked,
        )?;
        checked.notes.extend(siblings);
        Ok(LoginOutcome {
            provider_id: descriptor.id.to_string(),
            provider_name: descriptor.name.to_string(),
            kind: Some(CredentialKind::ApiKey),
            account: None,
            notes: checked.notes,
            advice: descriptor.after_login.map(str::to_string).into_iter().collect(),
        })
    }

    /// Prove the key works and learn what it can serve.
    ///
    /// What the resource has deployed is both the proof the key works and the only honest list of
    /// what it serves. A resource that will not say falls back to the model list, which proves the
    /// key but describes Azure rather than the resource.
    async fn verify_or_discover<'a>(
        &self,
        descriptor: &'a ProviderDescriptor,
        resource_root: Option<&str>,
        key: &Key,
        interaction: &dyn AuthInteraction,
    ) -> Result<Checked<'a>, AuthError> {
        let mut checked = Checked::default();
        let discovered = match (self.verify, resource_root) {
            (true, Some(root)) if descriptor.lists_deployments => {
                interaction.notify(AuthEvent::Progress {
                    message: "Reading the resource's deployments…".to_string(),
                });
                discover_with_key(descriptor, root, &ApiKey::new(&key.resolved)).await
            }
            _ => None,
        };
        if let Some(split) = &discovered {
            checked.notes.extend(deployments::describe(split));
        }
        // A provider with no model list can't answer for itself; where it shares a resource with
        // one that can, the sibling proves the same key.
        let checker = match descriptor.lists_models {
            true => Some(descriptor),
            false => self.halves(descriptor).into_iter().find(|sibling| sibling.lists_models),
        };
        if self.verify && discovered.is_none() {
            if let Some(checker) = checker {
                interaction.notify(AuthEvent::Progress {
                    message: "Checking the endpoint…".to_string(),
                });
                let auth = checker.key_style().headers(&ApiKey::new(&key.resolved));
                let endpoint = resource_root
                    .map(|root| checker.endpoint_for(root))
                    .or_else(|| checker.base_url.map(str::to_string));
                let models = verify_credential(checker, endpoint.as_deref(), &auth).await?;
                checked.notes.extend(describe_models(&models));
            }
        }
        checked.discovered = discovered;
        Ok(checked)
    }

    /// Write the credential and its config, and the same for the rest of a shared resource.
    ///
    /// One resource, one key, two wire APIs: both halves are configured, whichever was named. Only
    /// the half the user asked for becomes the default, so the sibling never steals it. Answers a
    /// note per sibling.
    fn persist_with_siblings(
        &self,
        descriptor: &ProviderDescriptor,
        credential: &Credential,
        resource_root: Option<&str>,
        checked: &Checked<'_>,
    ) -> Result<Vec<String>, AuthError> {
        let mut notes = Vec::new();
        for sibling in self.halves(descriptor) {
            self.persist(&sibling.id, credential)?;
            self.record_provider(sibling, resource_root)?;
            notes.push(format!("{} is configured with the same key", sibling.name));
        }
        self.persist(&descriptor.id, credential)?;
        self.record_config(descriptor, resource_root)?;
        for (id, served) in checked.discovered.iter().flatten() {
            self.config
                .set_deployments(id, served)
                .map_err(|error| AuthError::Storage(error.to_string()))?;
        }
        Ok(notes)
    }

    /// The other providers this login configures — the rest of a shared resource.
    fn halves(&self, descriptor: &ProviderDescriptor) -> Vec<&'static ProviderDescriptor> {
        descriptor.paired_with.iter().filter_map(|id| provider_descriptor(id)).collect()
    }

    /// Run a subscription flow.
    async fn login_subscription(
        &self,
        descriptor: &ProviderDescriptor,
        interaction: &dyn AuthInteraction,
    ) -> Result<LoginOutcome, AuthError> {
        let flow = descriptor.oauth_flow().ok_or_else(|| AuthError::UnknownMethod {
            provider: descriptor.id.to_string(),
            method: "oauth".to_string(),
        })?;
        let credential = super::oauth::login(flow, interaction).await?;

        let mut notes = Vec::new();
        let available =
            credential.as_oauth().map(|oauth| oauth.available_models.clone()).unwrap_or_default();
        if let Some(note) = describe_models(&available) {
            notes.push(note);
        }

        // OpenRouter's flow mints an API key; the others hand back a token pair. Either way the
        // provider is only really configured once we can list its models.
        if available.is_empty() && self.verify {
            interaction.notify(AuthEvent::Progress { message: "Checking access…".to_string() });
            let auth = match &credential {
                Credential::OAuth(oauth) => {
                    let mut auth = RequestAuth::bearer(&oauth.access);
                    if let Some(endpoint) = &oauth.endpoint {
                        auth = auth.with_base_url(endpoint.clone());
                    }
                    auth
                }
                Credential::ApiKey(api) => RequestAuth::bearer(&ApiKey::new(
                    resolve_key(api.key.expose()).await.unwrap_or_default(),
                )),
            };
            let endpoint =
                auth.base_url.clone().or_else(|| descriptor.base_url.map(str::to_string));
            if let Ok(models) = verify_credential(descriptor, endpoint.as_deref(), &auth).await {
                if let Some(note) = describe_models(&models) {
                    notes.push(note);
                }
            }
        }

        let account = credential.as_oauth().and_then(|oauth| oauth.account.clone());
        self.persist(&descriptor.id, &credential)?;
        self.record_config(descriptor, None)?;
        Ok(LoginOutcome {
            provider_id: descriptor.id.to_string(),
            provider_name: descriptor.name.to_string(),
            kind: Some(credential.kind()),
            account,
            notes,
            // Advice that only makes sense now that the credential exists.
            advice: descriptor.after_login.map(str::to_string).into_iter().collect(),
        })
    }

    /// Connect to a local server: confirm the endpoint, list what it's serving, remember it.
    async fn login_local(
        &self,
        descriptor: &ProviderDescriptor,
        interaction: &dyn AuthInteraction,
    ) -> Result<LoginOutcome, AuthError> {
        let default = descriptor.base_url.unwrap_or("http://127.0.0.1:1234/v1");
        let answer = interaction
            .prompt(AuthPrompt::optional_text(
                format!("{} endpoint (blank for the default):", descriptor.name),
                Some(default.to_string()),
            ))
            .await?;
        let typed = answer.expose_secret().trim().to_string();
        let base_url =
            if typed.is_empty() { default.to_string() } else { normalize_base_url(&typed)? };

        interaction.notify(AuthEvent::Progress { message: format!("Contacting {base_url}…") });
        let models = verify_credential(descriptor, Some(&base_url), &RequestAuth::none())
            .await
            .map_err(|error| match error {
                AuthError::Network(message) => AuthError::Network(format!(
                    "{message} — is {} running and serving {base_url}?",
                    descriptor.name
                )),
                other => other,
            })?;
        if models.is_empty() {
            return Err(AuthError::Rejected(format!(
                "{base_url} answered but is serving no models — load one in {} first",
                descriptor.name
            )));
        }

        // Recorded even when it is the default, because the block is the only durable record that
        // this server was connected at all — a keyless provider leaves nothing in `auth.json`, so
        // without it the machine reports itself unconfigured on the next launch.
        self.record_config(descriptor, Some(&base_url))?;
        // And what it serves, which the catalog cannot answer for a machine it has never seen.
        self.config
            .set_served_models(&descriptor.id, &models)
            .map_err(|error| AuthError::Storage(error.to_string()))?;
        Ok(LoginOutcome {
            provider_id: descriptor.id.to_string(),
            provider_name: descriptor.name.to_string(),
            kind: None,
            account: None,
            notes: describe_models(&models).into_iter().collect(),
            advice: descriptor.after_login.map(str::to_string).into_iter().collect(),
        })
    }

    /// Persist what the runtime needs to reach this provider next launch, and point luxe at it.
    fn record_config(
        &self,
        descriptor: &ProviderDescriptor,
        resource_root: Option<&str>,
    ) -> Result<(), AuthError> {
        self.record_provider(descriptor, resource_root)?;
        self.config
            .set_default_provider(&descriptor.id)
            .map_err(|error| AuthError::Storage(error.to_string()))
    }

    /// Record a provider's block, when the catalog can't supply the answer itself.
    ///
    /// A plain `luxe auth login openai` leaves `models.json` alone apart from the defaults.
    fn record_provider(
        &self,
        descriptor: &ProviderDescriptor,
        resource_root: Option<&str>,
    ) -> Result<(), AuthError> {
        let Some(base_url) = resource_root.map(|root| descriptor.endpoint_for(root)) else {
            return Ok(());
        };
        self.config
            .upsert_provider(
                &descriptor.id,
                Some(&base_url),
                Some(descriptor.api),
                descriptor.models_dev,
            )
            .map_err(|error| AuthError::Storage(error.to_string()))
    }
}

#[async_trait::async_trait]
impl AuthService for ProviderAuth {
    fn providers(&self) -> Vec<ProviderOption> {
        self.options()
    }

    /// Accepts the aliases people type (`copilot`, `codex`, `gemini`) as well as catalog ids.
    fn provider(&self, id: &str) -> Option<ProviderOption> {
        let canonical = canonical_id(id);
        self.providers().into_iter().find(|option| option.id == canonical || option.id == id)
    }

    async fn login(
        &self,
        provider_id: &str,
        method_id: &str,
        interaction: Arc<dyn AuthInteraction>,
    ) -> Result<LoginOutcome, AuthError> {
        let id = canonical_id(provider_id);
        let descriptor = provider_descriptor(&id)
            .ok_or_else(|| AuthError::UnknownProvider(provider_id.to_string()))?;
        let interaction = interaction.as_ref();

        // The custom endpoint is a wizard entry, not a provider: it asks what to become first.
        if descriptor.id == "custom" {
            let (resolved, base_url) = self.resolve_custom(interaction).await?;
            return self.login_api_key(&resolved, Some(base_url), interaction).await;
        }

        let method =
            descriptor.methods().into_iter().find(|method| method.id == method_id).ok_or_else(
                || AuthError::UnknownMethod {
                    provider: descriptor.id.to_string(),
                    method: method_id.to_string(),
                },
            )?;

        if let Some(note) = descriptor.note {
            interaction.notify(AuthEvent::Info { message: note.to_string(), links: Vec::new() });
        }

        match method.id.as_str() {
            "oauth" => self.login_subscription(descriptor, interaction).await,
            "local" => self.login_local(descriptor, interaction).await,
            _ if descriptor.needs_base_url => {
                let prompt = match descriptor.endpoint {
                    Some(endpoint) => AuthPrompt::text(
                        format!("{} resource (name or endpoint):", descriptor.name),
                        Some(endpoint.host.replace("{resource}", "my-resource")),
                    ),
                    None => AuthPrompt::text(
                        "Base URL, including the version segment:",
                        Some("https://host/v1".to_string()),
                    ),
                };
                let answer = interaction.prompt(prompt).await?;
                let root = normalize_base_url(&descriptor.resource_root(answer.expose_secret()))?;
                self.login_api_key(descriptor, Some(root), interaction).await
            }
            _ => self.login_api_key(descriptor, None, interaction).await,
        }
    }

    fn logout(&self, provider_id: &str) -> Result<bool, AuthError> {
        let id = canonical_id(provider_id);
        self.store.remove(&id).map_err(|error| AuthError::Storage(error.to_string()))
    }

    fn store_path(&self) -> std::path::PathBuf {
        self.store.path().to_path_buf()
    }
}

/// Ask for the provider's key and make sure it is something.
///
/// A `$ENV`/`!command` spec is stored verbatim (that's the point of it) but must resolve to
/// something now, or the user has a broken setup they won't discover until their first turn.
async fn ask_for_key(
    descriptor: &ProviderDescriptor,
    interaction: &dyn AuthInteraction,
) -> Result<Key, AuthError> {
    if let Some(url) = descriptor.console_url {
        interaction.notify(AuthEvent::Info {
            message: format!("Create a key for {}, then paste it here.", descriptor.name),
            links: vec![AuthLink { url: url.to_string(), label: Some("API keys".to_string()) }],
        });
    }
    let answer = interaction
        .prompt(AuthPrompt::secret(
            format!("{} API key:", descriptor.name),
            descriptor.key_hint.map(str::to_string),
        ))
        .await?;
    let spec = answer.expose_secret().trim().to_string();
    if spec.is_empty() {
        return Err(AuthError::Input("no key entered".to_string()));
    }
    let resolved = resolve_key(&spec).await.map_err(AuthError::Input)?;
    if resolved.trim().is_empty() {
        return Err(AuthError::Input(
            "that resolved to an empty value — check the environment variable or command"
                .to_string(),
        ));
    }
    if let Some(hint) = descriptor.key_hint {
        let prefix = hint.trim_end_matches(['…', '.']);
        if !prefix.is_empty() && !resolved.starts_with(prefix) {
            // A warning, not a refusal: key formats change more often than this code does.
            interaction.notify(AuthEvent::Info {
                message: format!(
                    "Heads up: {} keys usually start with {prefix} — storing it anyway.",
                    descriptor.name
                ),
                links: Vec::new(),
            });
        }
    }
    Ok(Key { spec, resolved })
}

#[cfg(test)]
mod tests;
