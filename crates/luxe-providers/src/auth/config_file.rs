//! Config patching — the small, surgical writes onboarding makes to `models.json`.
//!
//! Credentials go in `auth.json`; *configuration* a login learns goes here. Two cases only:
//!
//! - a provider the catalog can't know about (a custom OpenAI-compatible endpoint, an Azure
//!   resource) needs its `base_url` recorded, and
//! - finishing a login should leave the user pointed at something that works, which means
//!   `default_provider` (and a model, when we know a good one).
//!
//! Every write is lossless and locked — a user's hand-maintained `models.json` is theirs, and this
//! only ever adds or replaces the specific keys named below.

use std::path::{Path, PathBuf};

use luxe_core::auth::ApiKind;
use serde_json::{Map, Value};

use super::json_file::{FileError, FileMode, JsonFile};

/// `~/.config/luxe/models.json`.
#[derive(Debug, Clone)]
pub struct ConfigFile {
    file: JsonFile,
}

impl ConfigFile {
    /// A config file at an explicit path.
    pub fn new(path: PathBuf) -> Self {
        Self { file: JsonFile::new(path, FileMode::PreserveExisting) }
    }

    /// The default `models.json` next to the credential store.
    pub fn default_path() -> Self {
        Self::new(luxe_core::user_dirs::luxe_config_file("models.json"))
    }

    /// Where this writes.
    pub fn path(&self) -> &Path {
        self.file.path()
    }

    /// Point Luxe at a provider (and optionally a model) by default.
    ///
    /// This is what makes a first login actually take effect for the *next* launch; the running
    /// session switches through `Action::SetModel`.
    pub fn set_defaults(&self, provider: &str, model: Option<&str>) -> Result<(), FileError> {
        self.file.modify(|root| {
            root.insert("default_provider".into(), Value::String(provider.to_string()));
            if let Some(model) = model.filter(|model| !model.is_empty()) {
                root.insert("default_model".into(), Value::String(model.to_string()));
            }
            Ok(())
        })
    }

    /// Record a provider block: base URL, which API it speaks, and where to find its metadata.
    ///
    /// Merges into an existing block rather than replacing it, so a user's compat tweaks survive a
    /// re-login.
    pub fn upsert_provider(
        &self,
        provider: &str,
        base_url: Option<&str>,
        api: Option<ApiKind>,
        models_dev: Option<&str>,
    ) -> Result<(), FileError> {
        self.file.modify(|root| {
            let block = provider_block(root, provider)?;
            if let Some(base_url) = base_url {
                block.insert(
                    "base_url".into(),
                    Value::String(base_url.trim_end_matches('/').into()),
                );
            }
            if let Some(models_dev) = models_dev {
                block.insert("models_dev".into(), Value::String(models_dev.to_string()));
            }
            if let Some(api) = api {
                // The field that actually carries every API, including Anthropic's: writing only
                // the compat flag below recorded an `anthropic-messages` endpoint as
                // chat/completions, and the choice made in the wizard was silently lost.
                block.insert("api".into(), Value::String(api.as_str().to_string()));
                // Kept beside it because a Luxe old enough to predate `api` reads this one, and a
                // config file outlives the release that wrote it.
                let compat = block
                    .entry("compat".to_string())
                    .or_insert_with(|| Value::Object(Map::new()))
                    .as_object_mut()
                    .ok_or_else(|| {
                        FileError::Rejected(format!(
                            "\"providers.{provider}.compat\" in models.json is not an object"
                        ))
                    })?;
                compat.insert(
                    "use_responses_api".into(),
                    Value::Bool(api == ApiKind::OpenaiResponses),
                );
            }
            Ok(())
        })
    }

    /// Record what an endpoint answered `/models` with.
    ///
    /// Written whenever the answer changes and read when the server is unreachable, so the picker
    /// offers the models this machine actually has rather than the catalog's idea of them — and
    /// still offers something with the server switched off.
    pub fn set_served_models(&self, provider: &str, models: &[String]) -> Result<(), FileError> {
        self.file.modify(|root| {
            provider_block(root, provider)?.insert(
                "served_models".into(),
                Value::Array(models.iter().cloned().map(Value::String).collect()),
            );
            Ok(())
        })
    }

    /// Record what an Azure resource has deployed for one provider: the names it serves, and the
    /// model behind each name that is not the model's own.
    ///
    /// `deployments` is replaced whole rather than merged, so a deployment that was deleted stops
    /// mapping to anything. An empty map removes the key, keeping a block that needs none tidy.
    pub fn set_deployments(
        &self,
        provider: &str,
        served: &super::deployments::Served,
    ) -> Result<(), FileError> {
        self.file.modify(|root| {
            let block = provider_block(root, provider)?;
            block.insert(
                "served_models".into(),
                Value::Array(served.models.iter().cloned().map(Value::String).collect()),
            );
            match served.deployments.is_empty() {
                true => {
                    block.remove("deployments");
                }
                false => {
                    let map = served
                        .deployments
                        .iter()
                        .map(|(name, model)| (name.clone(), Value::String(model.clone())))
                        .collect();
                    block.insert("deployments".into(), Value::Object(map));
                }
            }
            Ok(())
        })
    }

    /// Provider ids already declared in the file.
    pub fn declared_providers(&self) -> Vec<String> {
        self.read().map(|root| declared_in(&root)).unwrap_or_default()
    }

    /// The whole file, parsed once, for a caller that reads it more than one way.
    pub(crate) fn read(&self) -> Option<Map<String, Value>> {
        self.file.read()
    }
}

/// The provider ids a parsed `models.json` has blocks for.
pub(crate) fn declared_in(root: &Map<String, Value>) -> Vec<String> {
    let providers = root.get("providers").and_then(Value::as_object);
    providers.map(|providers| providers.keys().cloned().collect()).unwrap_or_default()
}

/// `providers.<id>` in a parsed `models.json`, created when it is missing.
fn provider_block<'a>(
    root: &'a mut Map<String, Value>,
    provider: &str,
) -> Result<&'a mut Map<String, Value>, FileError> {
    root.entry("providers".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| {
            FileError::Rejected("\"providers\" in models.json is not an object".to_string())
        })?
        .entry(provider.to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| {
            FileError::Rejected(format!("\"providers.{provider}\" in models.json is not an object"))
        })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn config() -> (tempfile::TempDir, ConfigFile) {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigFile::new(dir.path().join("models.json"));
        (dir, config)
    }

    #[test]
    fn defaults_are_written_without_disturbing_anything_else() {
        let (_dir, config) = config();
        std::fs::write(
            config.path(),
            r#"{"models": {"a/b": {"context_window": 1}}, "default_model": "old"}"#,
        )
        .unwrap();
        config.set_defaults("openai", Some("gpt-5.4")).unwrap();
        let root = serde_json::from_str::<Value>(&std::fs::read_to_string(config.path()).unwrap())
            .unwrap();
        assert_eq!(root["default_provider"], "openai");
        assert_eq!(root["default_model"], "gpt-5.4");
        assert_eq!(root["models"]["a/b"]["context_window"], 1, "unrelated config survives");
    }

    #[test]
    fn a_provider_block_merges_rather_than_replaces() {
        let (_dir, config) = config();
        std::fs::write(
            config.path(),
            r#"{"providers": {"my-proxy": {"compat": {"supports_developer_role": true}}}}"#,
        )
        .unwrap();
        config
            .upsert_provider(
                "my-proxy",
                Some("https://proxy.example/v1/"),
                Some(ApiKind::OpenaiResponses),
                Some("openai"),
            )
            .unwrap();

        let root = serde_json::from_str::<Value>(&std::fs::read_to_string(config.path()).unwrap())
            .unwrap();
        let block = &root["providers"]["my-proxy"];
        assert_eq!(block["base_url"], "https://proxy.example/v1", "trailing slash trimmed");
        assert_eq!(block["models_dev"], "openai");
        assert_eq!(block["api"], "openai-responses");
        assert_eq!(block["compat"]["use_responses_api"], true);
        assert_eq!(
            block["compat"]["supports_developer_role"], true,
            "the user's own compat flag survives"
        );
        assert_eq!(config.declared_providers(), vec!["my-proxy".to_string()]);
    }

    #[test]
    fn a_completions_api_provider_records_that_explicitly() {
        let (_dir, config) = config();
        config
            .upsert_provider(
                "local",
                Some("http://127.0.0.1:8000/v1"),
                Some(ApiKind::OpenaiCompletions),
                None,
            )
            .unwrap();
        let root = serde_json::from_str::<Value>(&std::fs::read_to_string(config.path()).unwrap())
            .unwrap();
        assert_eq!(root["providers"]["local"]["api"], "openai-completions");
        assert_eq!(root["providers"]["local"]["compat"]["use_responses_api"], false);
        assert!(root["providers"]["local"].get("models_dev").is_none());
    }

    /// The bug this catches shipped: an Anthropic-speaking endpoint has no `use_responses_api`
    /// spelling, so recording only the compat flag read back as chat/completions on the next
    /// launch and every request went to the wrong path.
    #[test]
    fn an_anthropic_endpoint_is_recorded_as_one() {
        let (_dir, config) = config();
        config
            .upsert_provider(
                "foundry-anthropic",
                Some("https://r.services.ai.azure.com/anthropic/v1"),
                Some(ApiKind::AnthropicMessages),
                Some("azure"),
            )
            .unwrap();
        let raw = std::fs::read_to_string(config.path()).unwrap();
        let parsed = crate::config::load_models_config(config.path()).expect("loads");
        let block = parsed.providers.get("foundry-anthropic").expect("block");
        assert_eq!(block.api.as_deref(), Some("anthropic-messages"), "got {raw}");
        assert_eq!(
            crate::config::resolve_api("foundry-anthropic", Some(block)),
            ApiKind::AnthropicMessages
        );
    }

    #[test]
    fn a_providers_key_of_the_wrong_shape_is_reported_not_overwritten() {
        let (_dir, config) = config();
        std::fs::write(config.path(), r#"{"providers": "oops"}"#).unwrap();
        let error =
            config.upsert_provider("x", Some("http://x/v1"), None, None).expect_err("must refuse");
        assert!(format!("{error}").contains("not an object"), "got: {error}");
        assert_eq!(std::fs::read_to_string(config.path()).unwrap(), r#"{"providers": "oops"}"#);
    }

    #[test]
    fn declared_providers_is_empty_when_there_is_no_file() {
        let (_dir, config) = config();
        assert!(config.declared_providers().is_empty());
    }
}
