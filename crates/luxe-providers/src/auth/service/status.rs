//! Status — how each provider's credential resolves, and the wizard rows built from it.
//!
//! A listing asks the same question of every catalog entry, so the files that answer it are read
//! once into a [`Declared`] and handed to each row instead of re-parsed per provider.

use std::collections::{BTreeMap, BTreeSet};

use luxe_core::auth::{AuthMethod, CredentialStatus, ProviderOption};

use super::{ProviderAuth, ProviderDescriptor};
use crate::auth::config_file::declared_in;
use crate::auth::credential::Credential;
use crate::config::{models_config_from, ModelsConfig};

/// What the credential store and `models.json` say right now.
///
/// Both parse as [`ModelsConfig`], which is how the legacy nested `auth.json` shape
/// (`providers.<id>.api_key`) keeps working.
pub(super) struct Declared {
    credentials: BTreeMap<String, Credential>,
    store: Option<ModelsConfig>,
    config: Option<ModelsConfig>,
    config_providers: Vec<String>,
}

impl ProviderAuth {
    /// Read the credential store and `models.json`, once.
    pub(super) fn declared(&self) -> Declared {
        let stored = self.store.read();
        let configured = self.config.read();
        Declared {
            credentials: stored
                .as_ref()
                .map(|root| self.store.credentials_in(root))
                .unwrap_or_default(),
            config_providers: configured.as_ref().map(declared_in).unwrap_or_default(),
            store: stored.and_then(typed),
            config: configured.and_then(typed),
        }
    }

    /// Every provider the wizard lists: the catalog's, then whatever the user configured that it
    /// has never heard of.
    ///
    /// A custom endpoint, a proxy or a hand-written provider block must show up, or a working
    /// setup would look unconfigured — including to the first-run check, which would then reopen
    /// the wizard on every launch.
    pub(super) fn options(&self) -> Vec<ProviderOption> {
        let declared = self.declared();
        let mut options: Vec<ProviderOption> = crate::auth::provider_descriptors()
            .iter()
            .map(|descriptor| self.option(&declared, descriptor))
            .collect();
        let others: BTreeSet<&String> =
            declared.credentials.keys().chain(&declared.config_providers).collect();
        for id in others {
            if options.iter().any(|option| &option.id == id) {
                continue;
            }
            let status = self.status_for(&declared, id, &[]);
            if !status.is_configured() {
                continue;
            }
            options.push(ProviderOption {
                name: id.clone(),
                id: id.clone(),
                methods: vec![AuthMethod::api_key(None)],
                status,
                note: None,
                console_url: None,
                keyless: false,
                // It was configured by hand, so its endpoint is already settled.
                needs_endpoint: false,
            });
        }
        options
    }

    /// How a provider's credential currently resolves.
    ///
    /// Same precedence the runtime applies: a stored credential, then an ambient environment
    /// variable, then a key written by hand into a provider block. The last one matters more than it
    /// looks — someone whose `models.json` already works must not be shown a first-run wizard.
    ///
    /// A keyless provider has none of those and is still configured once it has been connected:
    /// its block in `models.json` is the record, which is why [`Self::login_local`] writes one
    /// even for the default endpoint.
    fn status(&self, declared: &Declared, descriptor: &ProviderDescriptor) -> CredentialStatus {
        match self.status_for(declared, &descriptor.id, descriptor.env) {
            CredentialStatus::Missing if descriptor.keyless => {
                match declared_endpoint(declared, descriptor) {
                    Some(endpoint) => CredentialStatus::Local(endpoint),
                    None => CredentialStatus::Missing,
                }
            }
            resolved => resolved,
        }
    }

    /// The credential half of [`Self::status`], for a provider the catalog has never heard of.
    fn status_for(&self, declared: &Declared, id: &str, env: &[&str]) -> CredentialStatus {
        if let Some(credential) = declared.credentials.get(id) {
            return CredentialStatus::Stored(credential.info());
        }
        for name in env {
            if std::env::var(name).is_ok_and(|value| !value.trim().is_empty()) {
                return CredentialStatus::Env((*name).to_string());
            }
        }
        match self.declared_key(declared, id) {
            Some(file) => CredentialStatus::Config(file),
            None => CredentialStatus::Missing,
        }
    }

    /// The config file declaring an API key for `id`, if one does.
    fn declared_key(&self, declared: &Declared, id: &str) -> Option<String> {
        let files = [
            (self.store.path(), declared.store.as_ref()),
            (self.config.path(), declared.config.as_ref()),
        ];
        files.into_iter().find_map(|(path, config)| {
            let provider = config?.providers.get(id)?;
            let has_key = provider.api_key.as_ref().is_some_and(|key| !key.is_empty());
            has_key.then(|| {
                path.file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string())
            })
        })
    }

    /// The wizard row for one provider.
    fn option(&self, declared: &Declared, descriptor: &ProviderDescriptor) -> ProviderOption {
        ProviderOption {
            id: descriptor.id.to_string(),
            name: descriptor.name.to_string(),
            methods: descriptor.methods(),
            status: self.status(declared, descriptor),
            note: descriptor.note.map(str::to_string),
            console_url: descriptor.console_url.map(str::to_string),
            keyless: descriptor.keyless,
            needs_endpoint: descriptor.needs_base_url,
        }
    }
}

/// The endpoint a connected local server was recorded at, if it was.
///
/// The block having been written is the fact; the URL in it is only what to show. A block that
/// omits one is still a connection, so it falls back to the descriptor's default rather than
/// reporting the provider as unconfigured.
fn declared_endpoint(declared: &Declared, descriptor: &ProviderDescriptor) -> Option<String> {
    let block = declared.config.as_ref()?.providers.get(&*descriptor.id)?;
    Some(
        block
            .base_url
            .clone()
            .or_else(|| descriptor.base_url.map(str::to_string))
            .unwrap_or_else(|| "this machine".to_string()),
    )
}

fn typed(root: serde_json::Map<String, serde_json::Value>) -> Option<ModelsConfig> {
    models_config_from(serde_json::Value::Object(root)).ok()
}
