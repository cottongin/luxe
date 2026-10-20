#![allow(clippy::unwrap_used)]

use super::models::parse_model_ids;
use super::*;
use local_server::LocalServer;
use luxe_core::auth::{AuthPromptKind, CredentialStatus};
use secrecy::SecretString;
use std::sync::{Arc, Mutex};
mod cleartext;
mod key_commands;
mod local_server;
mod reads;
mod verify;

/// A scripted interaction: answers in order, events recorded.
struct Scripted {
    answers: Mutex<Vec<String>>,
    events: Mutex<Vec<AuthEvent>>,
    prompts: Mutex<Vec<AuthPrompt>>,
}

impl Scripted {
    fn new(answers: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(answers.iter().rev().map(|a| a.to_string()).collect()),
            events: Mutex::new(Vec::new()),
            prompts: Mutex::new(Vec::new()),
        })
    }

    fn events(&self) -> Vec<AuthEvent> {
        self.events.lock().unwrap().clone()
    }

    fn prompts(&self) -> Vec<AuthPrompt> {
        self.prompts.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl AuthInteraction for Scripted {
    async fn prompt(&self, prompt: AuthPrompt) -> Result<SecretString, AuthError> {
        self.prompts.lock().unwrap().push(prompt);
        match self.answers.lock().unwrap().pop() {
            Some(answer) => Ok(SecretString::from(answer)),
            None => Err(AuthError::Cancelled),
        }
    }

    fn notify(&self, event: AuthEvent) {
        self.events.lock().unwrap().push(event);
    }
}

fn service() -> (tempfile::TempDir, ProviderAuth) {
    let dir = tempfile::tempdir().unwrap();
    let service = ProviderAuth::with_paths(
        AuthStore::new(dir.path().join("auth.json")),
        ConfigFile::new(dir.path().join("models.json")),
    )
    .without_verification();
    (dir, service)
}

#[tokio::test]
async fn an_api_key_login_stores_the_key_and_sets_the_default_provider() {
    let (dir, service) = service();
    let interaction = Scripted::new(&["sk-ant-live"]);
    let outcome =
        service.login("anthropic", "api-key", interaction.clone()).await.expect("stores the key");

    assert_eq!(outcome.provider_id, "anthropic");
    assert_eq!(outcome.kind, Some(CredentialKind::ApiKey));
    match service.store().credential("anthropic").expect("stored") {
        Credential::ApiKey(api) => assert_eq!(api.key.expose(), "sk-ant-live"),
        Credential::OAuth(_) => panic!("wrong kind"),
    }
    let config = std::fs::read_to_string(dir.path().join("models.json")).expect("config written");
    assert!(config.contains("\"default_provider\": \"anthropic\""), "got {config}");
    // No provider block: the catalog already knows Anthropic's endpoint.
    assert!(!config.contains("\"providers\""), "got {config}");

    // The key prompt must be masked, and the ToS note has to reach the user.
    assert!(matches!(interaction.prompts()[0].kind, AuthPromptKind::Secret));
    assert!(interaction.events().iter().any(|event| matches!(
        event,
        AuthEvent::Info { message, .. } if message.contains("terms")
    )));
}

#[tokio::test]
async fn an_empty_or_unresolvable_key_is_refused_before_anything_is_stored() {
    let (_dir, service) = service();
    let error =
        service.login("openai", "api-key", Scripted::new(&["   "])).await.expect_err("must refuse");
    assert!(matches!(error, AuthError::Input(_)), "got {error:?}");
    assert!(service.store().credential("openai").is_none(), "nothing may be stored");

    // A command spec that produces nothing fails at login, not silently at the first turn
    let error = service
        .login("openai", "api-key", Scripted::new(&["!printf ''"]))
        .await
        .expect_err("must refuse");
    assert!(matches!(error, AuthError::Input(_)), "got {error:?}");
}

/// `echo` rather than `printf`, because login validates the spec by *running* it and the shell
/// that runs it is whatever the platform has — `cmd` on a Windows box without Git for Windows.
#[tokio::test]
async fn a_key_spec_is_stored_verbatim_so_the_secret_stays_where_the_user_put_it() {
    let (_dir, service) = service();
    service
        .login("openai", "api-key", Scripted::new(&["!echo sk-from-1password"]))
        .await
        .expect("accepts a command spec");
    match service.store().credential("openai").expect("stored") {
        Credential::ApiKey(api) => assert_eq!(api.key.expose(), "!echo sk-from-1password"),
        Credential::OAuth(_) => panic!("wrong kind"),
    }
}

#[tokio::test]
async fn an_unexpected_key_shape_warns_but_still_stores() {
    let (_dir, service) = service();
    let interaction = Scripted::new(&["not-an-anthropic-key"]);
    service.login("anthropic", "api-key", interaction.clone()).await.expect("stores anyway");
    assert!(
        interaction.events().iter().any(|event| matches!(
            event,
            AuthEvent::Info { message, .. } if message.contains("usually start with")
        )),
        "expected a shape warning: {:?}",
        interaction.events()
    );
}

#[tokio::test]
async fn cancelling_a_prompt_cancels_the_login() {
    let (_dir, service) = service();
    let error =
        service.login("openai", "api-key", Scripted::new(&[])).await.expect_err("cancelled");
    assert!(error.is_cancelled(), "got {error:?}");
}

/// Two models, as a `/models` body.
fn two_models() -> String {
    r#"{"data":[{"id":"qwen3.8-27b"},{"id":"gemma-4-12b"}]}"#.to_string()
}

/// Connecting a local server has to leave something behind. It stores no credential, so the
/// provider block is the whole record — and the next launch reads it to decide whether this
/// machine can run a turn at all.
#[tokio::test]
async fn connecting_a_local_server_records_where_it_is() {
    let server = LocalServer::start(&two_models());

    let (dir, service) = service();
    let outcome = service
        .login("lmstudio", "local", Scripted::new(&[&server.endpoint]))
        .await
        .expect("the server answered");

    assert!(outcome.notes.iter().any(|note| note.contains("2 models available")));
    assert!(service.store().credential("lmstudio").is_none(), "nothing to store, so nothing is");

    let config: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("models.json")).expect("config"),
    )
    .unwrap();
    assert_eq!(config["providers"]["lmstudio"]["base_url"], server.endpoint);
    assert_eq!(config["default_provider"], "lmstudio");
    // The model is the reader's to choose: a login that wrote one would be luxe picking favorites
    assert!(config.get("default_model").is_none(), "no model chosen for them: {config}");
    // And what it is serving, which no catalog can answer for a machine it has never seen.
    assert_eq!(
        config["providers"]["lmstudio"]["served_models"],
        serde_json::json!(["gemma-4-12b", "qwen3.8-27b"])
    );
    // Which is what makes the machine configured from here on.
    assert!(matches!(
        service.provider("lmstudio").expect("in catalog").status,
        CredentialStatus::Local(_)
    ));
    assert!(service.any_configured());
}

/// A local list is a fact about this machine a moment ago, so it is re-asked rather than
/// remembered: loading a model changes what the picker should offer, and only the endpoint
/// knows. A server that is switched off leaves the recorded list standing rather than
/// emptying the picker.
#[tokio::test]
async fn a_local_servers_list_is_re_asked_and_survives_the_server_going_away() {
    let loaded = r#"{"data":[{"id":"qwen3.8-27b"},{"id":"gemma-4-12b"},{"id":"gpt-oss-20b"}]}"#;
    let mut server = LocalServer::start(&two_models());
    let (dir, service) = service();
    let models = dir.path().join("models.json");
    std::fs::write(
        &models,
        format!(r#"{{"providers": {{"lmstudio": {{"base_url": "{}"}}}}}}"#, server.endpoint),
    )
    .expect("the config");

    let served = |path: &std::path::Path| -> Vec<String> {
        crate::config::load_models_config(path)
            .expect("loads")
            .providers
            .get("lmstudio")
            .and_then(|block| block.served_models.clone())
            .unwrap_or_default()
    };

    assert!(service.refresh_local_models().await, "the first answer is news");
    assert_eq!(served(&models), vec!["gemma-4-12b".to_string(), "qwen3.8-27b".to_string()]);

    // A model was loaded since, so the recorded list moves with it.
    server.serves(loaded);
    assert!(service.refresh_local_models().await, "a different answer is news too");
    assert_eq!(
        served(&models),
        vec!["gemma-4-12b".to_string(), "gpt-oss-20b".to_string(), "qwen3.8-27b".to_string()]
    );

    // A server that is up with nothing loaded is not a server with no models: LM Studio
    // loads on demand, so an empty answer must not erase what this machine is known to have.
    server.serves(r#"{"data":[]}"#);
    assert!(!service.refresh_local_models().await, "an empty answer is not news");
    assert_eq!(served(&models).len(), 3, "and it does not wipe the list");

    // The server has gone. Nothing changes, and nothing is lost.
    server.stop();
    assert!(!service.refresh_local_models().await, "an unreachable server is not news");
    assert_eq!(served(&models).len(), 3, "the recorded list stands in");
}

/// A probe races `LOCAL_PROBE_TIMEOUT`, so a client that gives up leaves an accepted
/// connection that never became a request. What the server answers must not move because of
/// one: this failed on a loaded Windows runner and nowhere else, because the mock used to hand
/// out one scripted reply per accept and a dead connection ate the next test's answer.
#[tokio::test]
async fn a_connection_that_never_asks_does_not_shift_the_answer() {
    let server = LocalServer::start(&two_models());
    let (dir, service) = service();
    let models = dir.path().join("models.json");
    std::fs::write(
        &models,
        format!(r#"{{"providers": {{"lmstudio": {{"base_url": "{}"}}}}}}"#, server.endpoint),
    )
    .expect("the config");

    let address = server.endpoint.trim_start_matches("http://").trim_end_matches("/v1").to_string();
    for _ in 0..3 {
        drop(std::net::TcpStream::connect(&address).expect("the mock is listening"));
    }

    assert!(service.refresh_local_models().await, "the real request still gets the answer");
    let served = crate::config::load_models_config(&models)
        .expect("loads")
        .providers
        .get("lmstudio")
        .and_then(|block| block.served_models.clone())
        .unwrap_or_default();
    assert_eq!(served, vec!["gemma-4-12b".to_string(), "qwen3.8-27b".to_string()]);
}

/// A Foundry resource's deployment listing, as the resource spells it: the models it runs are
/// not the names it runs them under, and some of them cannot hold a conversation.
const FOUNDRY_LISTING: &str = r#"{"data": [
        {"id": "gpt-5.6-sol", "model": "gpt-5.6-sol", "status": "succeeded", "object": "deployment"},
        {"id": "gpt-realtime", "model": "gpt-realtime", "status": "succeeded", "object": "deployment"},
        {"id": "claude-opus-5", "model": "claude-opus-5", "status": "succeeded", "object": "deployment"},
        {"id": "Kimi-K2-Thinking", "model": "Kimi-K2.5", "status": "succeeded", "object": "deployment"}
    ], "object": "list"}"#;

/// What a provider's block records after discovery.
fn recorded(path: &std::path::Path, provider: &str) -> (Vec<String>, Vec<(String, String)>) {
    let config = crate::config::load_models_config(path).expect("loads");
    let block = config.providers.get(provider).cloned().unwrap_or_default();
    (block.served_models.unwrap_or_default(), block.deployments.into_iter().collect())
}

/// The picker offers what the resource has deployed rather than what Azure sells, which was
/// the defect: `claude-opus-5-5` offered beside a resource that only has `claude-opus-5`.
#[tokio::test]
async fn a_foundry_resource_is_asked_what_it_has_deployed() {
    let mut server = LocalServer::start(FOUNDRY_LISTING);
    let root = server.endpoint.trim_end_matches("/v1").to_string();
    let (dir, service) = service();
    let models = dir.path().join("models.json");
    std::fs::write(
        &models,
        serde_json::json!({ "providers": {
            "foundry-openai": { "base_url": format!("{root}/openai/v1"), "api_key": "fk" },
            "foundry-anthropic": { "base_url": format!("{root}/anthropic/v1") },
            "foundry-completions": { "base_url": format!("{root}/openai/v1") },
        }})
        .to_string(),
    )
    .expect("the config");

    assert!(service.refresh_deployments().await, "the first answer is news");
    assert_eq!(recorded(&models, "foundry-openai").0, ["gpt-5.6-sol"], "no realtime model");
    assert_eq!(recorded(&models, "foundry-anthropic").0, ["claude-opus-5"]);
    assert_eq!(
        recorded(&models, "foundry-completions"),
        (
            vec!["Kimi-K2-Thinking".to_string()],
            vec![("Kimi-K2-Thinking".to_string(), "Kimi-K2.5".to_string())]
        ),
        "routed by the model it runs, which is also what its metadata is looked up under"
    );
    assert!(!service.refresh_deployments().await, "the same answer is not news");

    // The route going away, or the resource going quiet, leaves the record standing
    server.serves(r#"{"error": {"code": "404"}}"#);
    assert!(!service.refresh_deployments().await);
    server.stop();
    assert!(!service.refresh_deployments().await);
    assert_eq!(recorded(&models, "foundry-anthropic").0, ["claude-opus-5"]);
}

/// `discover_deployments: false` is the switch the listing request answers to, and a list
/// written by hand under it is never overwritten.
#[tokio::test]
async fn a_resource_with_discovery_off_is_never_asked() {
    let server = LocalServer::start(FOUNDRY_LISTING);
    let root = server.endpoint.trim_end_matches("/v1").to_string();
    let (dir, service) = service();
    let models = dir.path().join("models.json");
    std::fs::write(
        &models,
        serde_json::json!({ "providers": {
            "foundry-openai": { "base_url": format!("{root}/openai/v1"), "api_key": "fk" },
            "foundry-anthropic": {
                "base_url": format!("{root}/anthropic/v1"),
                "discover_deployments": false,
                "served_models": ["sonnet-prod"],
            },
        }})
        .to_string(),
    )
    .expect("the config");

    assert!(!service.refresh_deployments().await);
    assert_eq!(recorded(&models, "foundry-anthropic").0, ["sonnet-prod"]);
    assert!(recorded(&models, "foundry-openai").0.is_empty(), "one switch covers the resource");
}

/// Logging in reads the deployments with the key just typed and records them for every surface.
#[tokio::test]
async fn a_foundry_login_records_the_deployments_it_found() {
    let server = LocalServer::start(FOUNDRY_LISTING);
    let root = server.endpoint.trim_end_matches("/v1").to_string();
    let dir = tempfile::tempdir().unwrap();
    let service = ProviderAuth::with_paths(
        AuthStore::new(dir.path().join("auth.json")),
        ConfigFile::new(dir.path().join("models.json")),
    );
    let outcome = service
        .login("foundry-anthropic", "api-key", Scripted::new(&[&root, "fk-live"]))
        .await
        .expect("configured");

    assert!(
        outcome.notes.iter().any(|note| note.contains("3 chat deployments")),
        "{:?}",
        outcome.notes
    );
    let models = dir.path().join("models.json");
    assert_eq!(recorded(&models, "foundry-anthropic").0, ["claude-opus-5"]);
    assert_eq!(recorded(&models, "foundry-openai").0, ["gpt-5.6-sol"]);
}

#[tokio::test]
async fn a_custom_endpoint_asks_what_it_is_and_records_a_provider_block() {
    let (dir, service) = service();
    let interaction =
        Scripted::new(&["my-proxy", "https://proxy.internal/v1", "openai-responses", "sk-x"]);
    let outcome = service.login("custom", "api-key", interaction).await.expect("configured");

    assert_eq!(outcome.provider_id, "my-proxy");
    assert!(service.store().credential("my-proxy").is_some());
    let config: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("models.json")).expect("config"),
    )
    .unwrap();
    assert_eq!(config["providers"]["my-proxy"]["base_url"], "https://proxy.internal/v1");
    assert_eq!(config["providers"]["my-proxy"]["compat"]["use_responses_api"], true);
    assert_eq!(config["default_provider"], "my-proxy");
}

/// One resource, one key, three wire APIs. Whichever surface is named, all three end up
/// reachable — each with its own path, its own API and the shared metadata catalog.
#[tokio::test]
async fn a_foundry_login_configures_every_surface_from_one_resource() {
    const SURFACES: [(&str, &str, &str); 3] = [
        ("foundry-openai", "openai-responses", "/openai/v1"),
        ("foundry-anthropic", "anthropic-messages", "/anthropic/v1"),
        ("foundry-completions", "openai-completions", "/openai/v1"),
    ];

    for (named, _, _) in SURFACES {
        let (dir, service) = service();
        let outcome = service
            .login(named, "api-key", Scripted::new(&["acme-foundry", "fk-live"]))
            .await
            .expect("configures the resource");
        assert_eq!(outcome.provider_id, named);

        let config =
            crate::config::load_models_config(&dir.path().join("models.json")).expect("loads");
        for (id, api, path) in SURFACES {
            match service.store().credential(id).expect("every surface holds the key") {
                Credential::ApiKey(key) => assert_eq!(key.key.expose(), "fk-live"),
                Credential::OAuth(_) => panic!("wrong kind"),
            }
            let block = config.providers.get(id).expect("block");
            assert_eq!(
                block.base_url.as_deref(),
                Some(format!("https://acme-foundry.services.ai.azure.com{path}").as_str()),
                "{id}"
            );
            assert_eq!(block.api.as_deref(), Some(api), "{id}");
            assert_eq!(block.models_dev.as_deref(), Some("azure"), "{id}");
        }

        // Only the surface the user asked for becomes the default; the siblings are reported,
        // not silently substituted.
        let raw = std::fs::read_to_string(dir.path().join("models.json")).expect("config");
        let root: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(root["default_provider"], named);
        assert_eq!(
            outcome.notes.iter().filter(|note| note.contains("same key")).count(),
            2,
            "both siblings have to be visible in the outcome: {:?}",
            outcome.notes
        );
    }
}

/// Pasting the target URI off a deployment's Details tab is what people actually do, and the
/// half they copied it from must not decide which endpoints get recorded.
#[tokio::test]
async fn a_pasted_foundry_target_uri_still_yields_both_endpoints() {
    let (dir, service) = service();
    service
        .login(
            "foundry-openai",
            "api-key",
            Scripted::new(&[
                "https://acme-foundry.services.ai.azure.com/anthropic/v1/messages",
                "fk-live",
            ]),
        )
        .await
        .expect("configures the resource");

    let config = crate::config::load_models_config(&dir.path().join("models.json")).expect("loads");
    assert_eq!(
        config.providers["foundry-openai"].base_url.as_deref(),
        Some("https://acme-foundry.services.ai.azure.com/openai/v1")
    );
    assert_eq!(
        config.providers["foundry-anthropic"].base_url.as_deref(),
        Some("https://acme-foundry.services.ai.azure.com/anthropic/v1")
    );
    assert_eq!(
        config.providers["foundry-completions"].base_url.as_deref(),
        Some("https://acme-foundry.services.ai.azure.com/openai/v1")
    );
}

/// `/login foundry` has to reach something, and the alias table is the only place that can say
/// which half a bare name means.
#[test]
fn the_foundry_aliases_land_on_a_real_half() {
    let (_dir, service) = service();
    for (typed, expected) in [
        ("foundry", "foundry-openai"),
        ("microsoft-foundry", "foundry-openai"),
        ("azure-foundry", "foundry-openai"),
        ("foundry-claude", "foundry-anthropic"),
        ("foundry-partners", "foundry-completions"),
    ] {
        assert_eq!(
            service.provider(typed).map(|option| option.id).as_deref(),
            Some(expected),
            "typed {typed}"
        );
    }
}

#[tokio::test]
async fn a_custom_name_that_collides_with_a_builtin_is_refused() {
    let (_dir, service) = service();
    let error = service
        .login("custom", "api-key", Scripted::new(&["openai"]))
        .await
        .expect_err("must refuse");
    assert!(format!("{error}").contains("already a built-in"), "got {error}");
}

#[tokio::test]
async fn a_bad_custom_name_or_url_is_refused() {
    let (_dir, service) = service();
    for answers in [vec!["has spaces"], vec!["ok-name", "not a url at all"]] {
        let error = service
            .login("custom", "api-key", Scripted::new(&answers))
            .await
            .expect_err("must refuse");
        assert!(matches!(error, AuthError::Input(_)), "got {error:?}");
    }
}

#[tokio::test]
async fn unknown_providers_and_methods_are_named_in_the_error() {
    let (_dir, service) = service();
    let error =
        service.login("not-a-provider", "api-key", Scripted::new(&[])).await.expect_err("unknown");
    assert!(matches!(error, AuthError::UnknownProvider(id) if id == "not-a-provider"));

    // Anthropic has no subscription method, by decision.
    let error = service.login("anthropic", "oauth", Scripted::new(&[])).await.expect_err("unknown");
    assert!(matches!(error, AuthError::UnknownMethod { .. }), "got {error:?}");
}

#[test]
fn status_reports_stored_then_environment_then_missing() {
    let (_dir, service) = service();
    let openai = || {
        service.providers().into_iter().find(|option| option.id == "openai").expect("in catalog")
    };
    // The host may legitimately have OPENAI_API_KEY set; only assert on the stored transition.
    let before = openai().status;
    service.store().put("openai", &Credential::api_key("sk")).unwrap();
    assert!(matches!(openai().status, CredentialStatus::Stored(_)));
    service.logout("openai").unwrap();
    assert_eq!(openai().status, before, "logging out restores the ambient status");
}

/// The Configuration surface offers three lists, and a row that opens an empty one is a dead
/// end a user meets on their first launch. Each has to be non-empty against the real catalog,
/// and single-method, so choosing a provider starts its flow rather than asking again.
#[test]
fn every_login_scope_offers_providers_that_start_on_one_choice() {
    use luxe_core::auth::LoginScope;

    let (_dir, service) = service();
    let all = service.providers();
    for scope in [LoginScope::Subscription, LoginScope::ApiKey, LoginScope::LocalOrCustom] {
        let scoped = scope.filter(all.clone());
        assert!(!scoped.is_empty(), "{} offers nothing", scope.as_str());
        for option in &scoped {
            assert!(
                option.default_method().is_some(),
                "{} / {}: a scoped row must be unambiguous, got {:?}",
                scope.as_str(),
                option.id,
                option.methods
            );
        }
    }

    let named = |scope: LoginScope| -> Vec<String> {
        scope.filter(all.clone()).into_iter().map(|option| option.id).collect()
    };
    assert_eq!(
        named(LoginScope::Subscription),
        vec!["openai-codex", "github-copilot", "openrouter"],
        "the three sanctioned subscription flows, and only those"
    );
    let local = named(LoginScope::LocalOrCustom);
    for id in ["lmstudio", "ollama", "custom"] {
        assert!(local.iter().any(|listed| listed == id), "{id} belongs in local-or-custom");
    }
    assert!(
        named(LoginScope::ApiKey).iter().any(|id| id == "anthropic"),
        "a key-only provider belongs in the key list"
    );
}

/// The regression from the first live run: a custom provider's stored credential was invisible
/// to `providers()`, so the first-run wizard reopened on every launch.
#[test]
fn a_provider_the_catalog_does_not_know_still_shows_as_configured() {
    let (_dir, service) = service();
    service.store().put("my-proxy", &Credential::api_key("sk-proxy")).unwrap();

    let listed = service.provider("my-proxy").expect("a stored credential is a provider");
    assert!(listed.status.is_stored());
    assert!(service.any_configured(), "a configured machine must not be offered onboarding");
    assert!(service.stored().iter().any(|option| option.id == "my-proxy"));
}

/// The other half of that: a key written by hand into a provider block counts as configured.
#[test]
fn a_hand_written_provider_block_counts_as_configured() {
    let dir = tempfile::tempdir().unwrap();
    let models = dir.path().join("models.json");
    std::fs::write(
        &models,
        r#"{"providers": {"anthropic": {"api_key": "!printf sk-ant-from-a-command"}}}"#,
    )
    .unwrap();
    let service = ProviderAuth::with_paths(
        AuthStore::new(dir.path().join("auth.json")),
        ConfigFile::new(models),
    );

    let anthropic = service.provider("anthropic").expect("in catalog");
    assert!(
        matches!(anthropic.status, CredentialStatus::Config(ref file) if file == "models.json"),
        "got {:?}",
        anthropic.status
    );
    assert!(service.any_configured());
    assert!(
        !anthropic.status.is_stored(),
        "a config key isn't ours to delete, so /logout must not offer it"
    );
}

/// A local server has no credential to find, so "nothing is stored" and "nothing is set up"
/// are the same file contents. Its recorded block is what tells them apart — without it a
/// machine running only LM Studio is told forever that no provider is configured, and the
/// model picker it is sent to has nothing in it.
#[test]
fn a_connected_local_server_counts_as_configured() {
    let dir = tempfile::tempdir().unwrap();
    let models = dir.path().join("models.json");
    let open = |models: &std::path::Path| {
        ProviderAuth::with_paths(
            AuthStore::new(dir.path().join("auth.json")),
            ConfigFile::new(models.to_path_buf()),
        )
    };

    // Nothing written yet: a keyless provider nobody has connected is still unconfigured.
    std::fs::write(&models, "{}").unwrap();
    let service = open(&models);
    assert_eq!(service.provider("lmstudio").expect("in catalog").status, CredentialStatus::Missing);
    assert!(!service.any_configured(), "a keyless provider is not configured by existing");

    // Connected at a chosen endpoint: the block names where.
    std::fs::write(
        &models,
        r#"{"providers": {"lmstudio": {"base_url": "http://127.0.0.1:4891/v1"}}}"#,
    )
    .unwrap();
    let service = open(&models);
    let lmstudio = service.provider("lmstudio").expect("in catalog");
    assert_eq!(lmstudio.status, CredentialStatus::Local("http://127.0.0.1:4891/v1".to_string()));
    assert!(service.any_configured(), "a machine with a local model can run turns");
    assert!(!lmstudio.status.is_stored(), "there is nothing in auth.json for /logout to take");

    // Connected at the catalog's own default: the block carries no URL, and the status falls
    // back to the one the connection used rather than reporting nothing at all.
    std::fs::write(&models, r#"{"providers": {"lmstudio": {}}}"#).unwrap();
    assert_eq!(
        open(&models).provider("lmstudio").expect("in catalog").status,
        CredentialStatus::Local("http://127.0.0.1:1234/v1".to_string())
    );

    // And the rule is about keyless providers, not about having a block: a provider that
    // takes a key is not configured by someone writing down its endpoint.
    std::fs::write(
        &models,
        r#"{"providers": {"openai": {"base_url": "https://api.openai.com/v1"}}}"#,
    )
    .unwrap();
    let service = open(&models);
    if std::env::var("OPENAI_API_KEY").is_err() {
        assert_eq!(
            service.provider("openai").expect("in catalog").status,
            CredentialStatus::Missing
        );
    }
}

/// The legacy nested `auth.json` shape is the same code path, and predates this phase.
#[test]
fn the_legacy_nested_auth_file_still_counts_as_configured() {
    let dir = tempfile::tempdir().unwrap();
    let auth = dir.path().join("auth.json");
    std::fs::write(&auth, r#"{"providers": {"openai": {"api_key": "sk-legacy"}}}"#).unwrap();
    let service = ProviderAuth::with_paths(
        AuthStore::new(auth),
        ConfigFile::new(dir.path().join("models.json")),
    );
    let openai = service.provider("openai").expect("in catalog");
    assert!(matches!(openai.status, CredentialStatus::Config(_)), "got {:?}", openai.status);
}

/// `/login copilot` has to reach GitHub Copilot; the alias table is in the catalog, so the
/// service has to apply it on lookup or every front-end would need its own copy.
#[test]
fn lookup_accepts_the_aliases_people_type() {
    let (_dir, service) = service();
    for (typed, expected) in [
        ("copilot", "github-copilot"),
        ("codex", "openai-codex"),
        ("gemini", "google"),
        ("openai_compat", "openai"),
    ] {
        assert_eq!(
            service.provider(typed).map(|option| option.id).as_deref(),
            Some(expected),
            "typed {typed}"
        );
    }
    assert!(service.provider("not-a-provider").is_none());
}

#[test]
fn logout_reports_whether_there_was_anything_to_remove() {
    let (_dir, service) = service();
    assert!(!service.logout("openai").unwrap());
    service.store().put("openai", &Credential::api_key("sk")).unwrap();
    assert!(service.logout("openai").unwrap());
    assert!(service.store().credential("openai").is_none());
    // Aliases resolve, so `/logout copilot` works.
    service.store().put("github-copilot", &Credential::api_key("t")).unwrap();
    assert!(service.logout("copilot").unwrap());
}

#[test]
fn stored_lists_only_providers_with_a_credential_in_the_file() {
    let (_dir, service) = service();
    assert!(service.stored().is_empty());
    service.store().put("openai", &Credential::api_key("sk")).unwrap();
    let stored: Vec<String> = service.stored().into_iter().map(|option| option.id).collect();
    assert_eq!(stored, vec!["openai".to_string()]);
    assert!(service.any_configured());
}

#[test]
fn model_lists_are_parsed_from_both_shapes_and_summarized() {
    let ids = parse_model_ids(
        r#"{"data":[{"id":"b"},{"id":"a"},{"id":"a"},{"no_id":1}],"object":"list"}"#,
    );
    assert_eq!(ids, vec!["a".to_string(), "b".to_string()], "sorted and deduped");
    assert!(parse_model_ids("not json").is_empty());
    assert!(parse_model_ids(r#"{"models":[]}"#).is_empty());

    // A flat `/v1/models` list mixes in the models that answer nothing. Offering one as the
    // default — which is what a local server's first entry becomes — fails on the first turn
    // with no way for the reader to know why.
    let mixed = parse_model_ids(
        r#"{"data":[{"id":"qwen3.8-27b"},{"id":"text-embedding-nomic-embed-text-v1.5"},
                        {"id":"bge-reranker-v2-m3"},{"id":"text-embedding-3-large"}]}"#,
    );
    assert_eq!(mixed, vec!["qwen3.8-27b".to_string()]);

    assert_eq!(describe_models(&[]), None);
    let one = describe_models(&["only".to_string()]).unwrap();
    assert_eq!(one, "1 model available (only)");
    let many: Vec<String> = (0..10).map(|index| format!("m{index}")).collect();
    let summary = describe_models(&many).unwrap();
    assert!(summary.starts_with("10 models available (m0, m1, m2, …"), "got {summary}");
}

#[test]
fn base_urls_are_normalized_and_nonsense_is_rejected() {
    assert_eq!(normalize_base_url(" https://x.dev/v1/ ").unwrap(), "https://x.dev/v1");
    assert_eq!(normalize_base_url("x.dev/v1").unwrap(), "https://x.dev/v1");
    assert_eq!(normalize_base_url("http://127.0.0.1:1234/v1").unwrap(), "http://127.0.0.1:1234/v1");
    assert!(normalize_base_url("").is_err());
    assert!(normalize_base_url("file:///etc/passwd").is_err());
    assert!(normalize_base_url("not a url").is_err());
}
