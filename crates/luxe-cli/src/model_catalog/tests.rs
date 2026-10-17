use super::*;
use luxe_providers::config::{ModelConfig, ProviderConfig};

fn config_with(models: &[(&str, ModelConfig)], providers: &[&str]) -> ModelsConfig {
    ModelsConfig {
        models: models.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        providers: providers.iter().map(|p| (p.to_string(), ProviderConfig::default())).collect(),
        ..Default::default()
    }
}

/// Billing has to survive the whole resolution chain, or the ledger books a plan turn as spend.
#[test]
fn subscription_providers_resolve_as_plan_billed() {
    use luxe_core::provider::Billing;
    let config = ModelsConfig::default();

    let codex = resolve_model(&config, "openai-codex", "gpt-5.3-codex").0;
    assert_eq!(codex.billing, Billing::Subscription);
    assert!(codex.context_window > 0, "codex borrows OpenAI's metadata: {codex:?}");
    assert!(codex.cost.is_known(), "the rates are known — they're just not a charge");

    let copilot = resolve_model(&config, "github-copilot", "claude-sonnet-5").0;
    assert_eq!(copilot.billing, Billing::Subscription);

    // An API key is real money.
    assert_eq!(resolve_model(&config, "openai", "gpt-5.6").0.billing, Billing::PerToken);
    assert_eq!(resolve_model(&config, "anthropic", "claude-sonnet-5").0.billing, Billing::PerToken);
    // OpenRouter mints a key that spends credits per token.
    assert_eq!(
        resolve_model(&config, "openrouter", "anthropic/claude-sonnet-5").0.billing,
        Billing::PerToken
    );
}

/// A Copilot plan decides which models the account may use, and the login recorded that list;
/// the picker must respect it rather than offering the whole catalog.
#[test]
fn a_credential_that_enumerates_its_models_restricts_the_picker() {
    let allowed = vec!["gpt-5.6-sol".to_string(), "claude-sonnet-5".to_string()];
    assert!(offers_model("github-copilot", "gpt-5.6-sol", &allowed));
    assert!(offers_model("github-copilot", "claude-sonnet-5", &allowed));
    assert!(!offers_model("github-copilot", "gemini-3-flash-preview", &allowed));
}

/// Verified against the live backend: `gpt-5.3-codex` is an API model that the ChatGPT-account
/// Codex endpoint rejects outright, and `gpt-5.6-sol` is one it serves. A heuristic on the id
/// ("contains codex") got this exactly backwards, so the catalog names the served set.
#[test]
fn codex_offers_what_a_chatgpt_account_can_actually_run() {
    assert!(offers_model("openai-codex", "gpt-5.6-sol", &[]));
    assert!(offers_model("codex", "gpt-5.4", &[]), "aliases resolve too");
    assert!(offers_model("openai-codex", "gpt-5.3-codex-spark", &[]));
    assert!(
        !offers_model("openai-codex", "gpt-5.3-codex", &[]),
        "the API-only Codex model 400s on a ChatGPT account"
    );
    // The plain OpenAI provider serves its whole catalog.
    assert!(offers_model("openai", "gpt-5.3-codex", &[]));
    assert!(offers_model("openai", "gpt-5.6", &[]));
}

#[test]
fn a_provider_that_says_nothing_about_its_models_offers_all_of_them() {
    assert!(offers_model("anthropic", "claude-sonnet-5", &[]));
    assert!(offers_model("some-proxy", "whatever-1", &[]));
}

#[test]
fn a_model_released_after_this_build_still_resolves_from_the_snapshot() {
    // The whole point of the embedded snapshot: no config entry, no cache, no network.
    let (info, from) = resolve_model(&ModelsConfig::default(), "anthropic", "claude-opus-5");
    assert_eq!(info.context_window, 1_000_000);
    assert_eq!(info.max_tokens, 128_000);
    assert!(info.reasoning);
    assert!(info.cost.is_known());
    assert!(matches!(from.context_window, catalog::Layer::Snapshot | catalog::Layer::Cache));
}

#[test]
fn config_still_wins_over_the_catalog() {
    let config = config_with(
        &[(
            "anthropic/claude-opus-5",
            ModelConfig {
                name: Some("Opus".into()),
                context_window: Some(200_000),
                ..Default::default()
            },
        )],
        &[],
    );
    let (info, from) = resolve_model(&config, "anthropic", "claude-opus-5");
    assert_eq!(info.context_window, 200_000);
    assert_eq!(from.context_window, catalog::Layer::Config);
    assert_eq!(info.display(), "Opus");
    // Untouched fields still come from the catalog.
    assert_eq!(info.max_tokens, 128_000);
}

#[test]
fn an_unknown_local_model_keeps_its_configured_shape() {
    let config = config_with(
        &[(
            "lmstudio/some-local-build",
            ModelConfig {
                name: Some("local".into()),
                context_window: Some(32_768),
                ..Default::default()
            },
        )],
        &["lmstudio"],
    );
    let (info, _) = resolve_model(&config, "lmstudio", "some-local-build");
    assert_eq!(info.context_window, 32_768);
    assert_eq!(info.max_tokens, 0, "non-Anthropic omits max_tokens unless asked");
    assert!(!info.cost.is_known());
}

/// What a local server serves is a fact about this machine, and models.dev cannot know it.
///
/// The first live run of this: LM Studio reported three models it had, and the picker offered
/// three entirely different ids that models.dev publishes under `lmstudio` — none of which
/// this machine had. The login's own choice was not among them either, so adopting it fell
/// through to "any model for that provider" and switched the session to something the server
/// would have refused.
#[test]
fn a_local_servers_own_list_replaces_the_catalogs_idea_of_it() {
    use crate::engine_setup::test_env::{isolated_env, ENV_LOCK};

    let _guard = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = isolated_env();
    invalidate_cached_catalog();
    let luxe = dir.path().join("luxe");
    std::fs::create_dir_all(&luxe).expect("the config dir");
    std::fs::write(
        luxe.join("models.json"),
        r#"{
              "providers": {
                "lmstudio": {
                  "base_url": "http://127.0.0.1:1234/v1",
                  "models_dev": "lmstudio",
                  "served_models": ["qwen3.8-27b", "qwen3.6-27b-fable-fusion"]
                }
              },
              "default_provider": "lmstudio"
            }"#,
    )
    .expect("the config");

    let settings = AgentSettings::resolve(std::path::PathBuf::from("."), Default::default());
    let listed: Vec<String> =
        build_menu(&settings).catalog.iter().map(ModelEntry::qualified_id).collect();
    assert_eq!(
        listed,
        vec!["lmstudio/qwen3.6-27b-fable-fusion".to_string(), "lmstudio/qwen3.8-27b".to_string()],
        "only what the endpoint said it serves"
    );
    drop(dir);
}

/// The menu is what the providers you configured can answer for — the reason a model comes
/// after a provider rather than before it.
///
/// A fresh machine used to open the picker on OpenAI's whole catalog: the fallback provider
/// name aliases to `openai` for metadata, and it was treated as routable whether or not
/// anything could authenticate it. Thirty-eight models, none of which would answer.
#[test]
fn an_unconfigured_machine_has_no_models_to_choose_between() {
    use crate::engine_setup::test_env::{isolated_env, ENV_LOCK};

    let _guard = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = isolated_env();
    invalidate_cached_catalog();

    let empty = AgentSettings::resolve(std::path::PathBuf::from("."), Default::default());
    let got = build_menu(&empty).catalog;
    assert!(
        got.is_empty(),
        "nothing is configured, so there is nothing that could answer: {:?}",
        got.iter().map(ModelEntry::qualified_id).take(8).collect::<Vec<_>>()
    );

    // A key in the environment is a configured provider, and its models are the ones that
    // appear — this is the path that must not be broken by gating the rest.
    std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-not-a-real-key");
    let with_key = AgentSettings::resolve(std::path::PathBuf::from("."), Default::default());
    let catalog = build_menu(&with_key).catalog;
    std::env::remove_var("ANTHROPIC_API_KEY");
    assert!(!catalog.is_empty(), "an ambient key is a provider Luxe can route to");
    assert!(
        catalog.iter().all(|entry| entry.info.r#ref.provider == "anthropic"),
        "and only that provider's models: {:?}",
        catalog.iter().map(ModelEntry::qualified_id).collect::<Vec<_>>()
    );

    // So is a key handed to this launch, for whichever provider it was handed for.
    let flagged = AgentSettings::resolve(
        std::path::PathBuf::from("."),
        crate::engine_setup::CliOverrides {
            provider: Some("openai".into()),
            api_key: Some("sk-not-a-real-key".into()),
            ..Default::default()
        },
    );
    let catalog = build_menu(&flagged).catalog;
    assert!(
        catalog.iter().any(|entry| entry.info.r#ref.provider == "openai"),
        "a launch that carries the credential can reach its provider"
    );

    // And so is naming a server that needs no credential: there is nothing else to say.
    let local = AgentSettings::resolve(
        std::path::PathBuf::from("."),
        crate::engine_setup::CliOverrides {
            provider: Some("lmstudio".into()),
            ..Default::default()
        },
    );
    assert!(
        build_menu(&local).catalog.iter().any(|entry| entry.info.r#ref.provider == "lmstudio"),
        "`--provider lmstudio` has said everything it can say about reaching it"
    );
    drop(dir);
}
