#![allow(clippy::unwrap_used)]

use super::*;
use luxe_core::auth::ApiKind;
use std::env;

#[test]
fn resolve_value_literal() {
    assert_eq!(resolve_value("plain-key"), "plain-key");
}

#[test]
fn resolve_value_env_var() {
    env::set_var("TEST_LUXE_RESOLVE", "resolved-value");
    assert_eq!(resolve_value("$TEST_LUXE_RESOLVE"), "resolved-value");
    env::remove_var("TEST_LUXE_RESOLVE");
}

#[test]
fn resolve_value_braced_env_var() {
    env::set_var("TEST_LUXE_RESOLVE2", "braced-value");
    assert_eq!(resolve_value("${TEST_LUXE_RESOLVE2}"), "braced-value");
    env::remove_var("TEST_LUXE_RESOLVE2");
}

/// An unset `${VAR}` expands to nothing, as it does in `mcp.json`, while a bare `$NAME` that names
/// nothing stays as written.
///
/// The break this catches: the same `${VAR}` gave a literal here and an empty string in the MCP
/// config, so an unset key went to the provider as the text `${VAR}`.
#[test]
fn an_unset_braced_variable_is_empty_and_a_bare_one_stays() {
    env::set_var("TEST_LUXE_RESOLVE_ENV_FORM", "env-form");
    assert_eq!(resolve_value("$env:TEST_LUXE_RESOLVE_ENV_FORM"), "env-form", "the MCP form too");
    env::remove_var("TEST_LUXE_RESOLVE_ENV_FORM");
    assert_eq!(resolve_value("${LUXE_RESOLVE_DEFINITELY_UNSET}"), "");
    assert_eq!(resolve_value("$LUXE_RESOLVE_DEFINITELY_UNSET"), "$LUXE_RESOLVE_DEFINITELY_UNSET");
}

#[test]
fn resolve_value_command() {
    // Use a simple command that always succeeds.
    assert_eq!(resolve_value("!echo hello"), "hello");
}

#[test]
fn resolve_value_escape_double_dollar() {
    // `$$` should collapse to a literal `$`.
    assert_eq!(resolve_value("$$"), "$");
    assert_eq!(resolve_value("a$$b"), "a$b");
    // After `$$`, the remaining `$key` is still subject to env expansion.
    env::set_var("TEST_LUXE_KEY", "resolved");
    assert_eq!(resolve_value("$$TEST_LUXE_KEY"), "resolved");
    env::remove_var("TEST_LUXE_KEY");
}

#[test]
fn resolve_value_escape_dollar_bang() {
    // `$!` should escape to a literal `!` so it's not treated as a command.
    assert_eq!(resolve_value("$!echo hi"), "!echo hi");
    // `$b` is not a valid escape — only `$$` and `$!` are.
    assert_eq!(resolve_value("a$b!b"), "a$b!b");
}

/// The header a key goes out on decides whether a request is authenticated at all, and for a
/// host that re-serves another vendor's API it does not follow from the protocol.
#[test]
fn the_key_style_comes_from_the_provider_then_its_api_then_config() {
    use crate::auth::KeyStyle;

    // The catalog answers first — Foundry's passthrough speaks Anthropic and authenticates the
    // Azure way, which no rule about the API could have derived.
    assert_eq!(resolve_key_style("foundry-anthropic", None), KeyStyle::FoundryKey);
    assert_eq!(resolve_key_style("foundry-openai", None), KeyStyle::Bearer);
    assert_eq!(resolve_key_style("anthropic", None), KeyStyle::AnthropicKey);
    assert_eq!(resolve_key_style("openai", None), KeyStyle::Bearer);

    // A provider only the user knows falls back to what its API implies.
    let declared = ProviderConfig { api: Some("anthropic-messages".into()), ..Default::default() };
    assert_eq!(resolve_key_style("my-proxy", Some(&declared)), KeyStyle::AnthropicKey);
    assert_eq!(resolve_key_style("my-proxy", None), KeyStyle::Bearer);

    // And an explicit `auth` wins over both — the line that makes an Entra ID token reach a
    // Foundry endpoint that would otherwise get a key header.
    let entra = ProviderConfig { auth: Some("bearer".into()), ..Default::default() };
    assert_eq!(resolve_key_style("foundry-anthropic", Some(&entra)), KeyStyle::Bearer);
    // An unparseable value falls through rather than failing the launch, as `api` does.
    let nonsense = ProviderConfig { auth: Some("semaphore".into()), ..Default::default() };
    assert_eq!(resolve_key_style("foundry-anthropic", Some(&nonsense)), KeyStyle::FoundryKey);
}

/// Its three callers must agree: the adapter is built from this, the auth header is picked from
/// it, and model resolution asks it whether Anthropic's required `max_tokens` applies.
#[test]
fn the_wire_api_comes_from_config_then_the_catalog_then_the_legacy_flag() {
    use luxe_core::auth::ApiKind;

    assert_eq!(resolve_api("foundry-anthropic", None), ApiKind::AnthropicMessages);
    assert_eq!(resolve_api("foundry-openai", None), ApiKind::OpenaiResponses);
    assert_eq!(resolve_api("groq", None), ApiKind::OpenaiCompletions);
    assert_eq!(resolve_api("some-proxy", None), ApiKind::OpenaiCompletions);

    let pinned = ProviderConfig { api: Some("anthropic-messages".into()), ..Default::default() };
    assert_eq!(resolve_api("foundry-openai", Some(&pinned)), ApiKind::AnthropicMessages);

    // The pre-catalog way of asking for `/responses` still works for a config that set it.
    let legacy = ProviderConfig {
        compat: CompatFlags { use_responses_api: true, ..Default::default() },
        ..Default::default()
    };
    assert_eq!(resolve_api("my-proxy", Some(&legacy)), ApiKind::OpenaiResponses);
}

#[test]
fn compat_flags_default() {
    let flags = CompatFlags::default();
    assert!(!flags.supports_developer_role);
    assert!(!flags.supports_reasoning_effort);
    // Note: max_tokens_field has a default function, so this test verifies the default.
    // The actual default is set by the `default` attribute in serde, not in Rust's Default impl.
    // For this test, we just verify the struct compiles and other fields are correct.
    assert!(flags.thinking_format.is_none());
    assert!(flags.cache_control_format.is_none());
    assert!(!flags.requires_tool_result_name);
    // Usage-in-streaming is on by default so token/cost/context accounting works OOTB.
    assert!(flags.supports_usage_in_streaming);
    assert_eq!(flags.max_tokens_field, "max_tokens");
}

#[test]
fn compat_flags_usage_default_survives_partial_config() {
    // A compat block that omits the field must still default it on (field-level serde
    // default), matching the Rust Default — the reference lmstudio config relies on this.
    let flags: CompatFlags = serde_json::from_str(r#"{"supports_developer_role": true}"#).unwrap();
    assert!(flags.supports_usage_in_streaming);
    // An explicit opt-out is still honored.
    let off: CompatFlags =
        serde_json::from_str(r#"{"supports_usage_in_streaming": false}"#).unwrap();
    assert!(!off.supports_usage_in_streaming);
}

/// A provider block written for a base URL must not switch off the catalog's quirks.
#[test]
fn a_partial_provider_block_keeps_the_catalog_compat_defaults() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("models.json");
    std::fs::write(&path, r#"{"providers":{"anthropic":{"base_url":"https://proxy.example/v1"}}}"#)
        .expect("write");

    let config = load_models_config(&path).expect("loads");
    let anthropic = config.providers.get("anthropic").expect("provider block");
    assert_eq!(anthropic.base_url.as_deref(), Some("https://proxy.example/v1"));
    assert!(
        anthropic.compat.cache_control_format.is_some(),
        "prompt caching must survive a block that never mentioned it"
    );
}

#[test]
fn an_explicitly_written_compat_flag_still_wins() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("models.json");
    std::fs::write(
        &path,
        r#"{"providers":{"anthropic":{"compat":{"cache_control_format":null,"max_tokens_field":"max_completion_tokens"}}}}"#,
    )
    .expect("write");

    let config = load_models_config(&path).expect("loads");
    let anthropic = config.providers.get("anthropic").expect("provider block");
    assert!(anthropic.compat.cache_control_format.is_none(), "an explicit null disables it");
    assert_eq!(anthropic.compat.max_tokens_field, "max_completion_tokens");
}

#[test]
fn an_unknown_provider_block_is_left_alone() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("models.json");
    std::fs::write(&path, r#"{"providers":{"my-proxy":{"base_url":"http://localhost/v1"}}}"#)
        .expect("write");

    let config = load_models_config(&path).expect("loads");
    let proxy = config.providers.get("my-proxy").expect("provider block");
    assert!(proxy.compat.cache_control_format.is_none());
}

/// … unless it says which wire API it speaks, because that is the thing that decides whether
/// `cache_control` means anything. A hand-declared Claude proxy used to come out of here with
/// prompt caching off: no error, no warning, and roughly ten times the input bill.
#[test]
fn a_declared_anthropic_proxy_still_gets_prompt_caching() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("models.json");
    std::fs::write(
        &path,
        r#"{"providers":{"my-claude-proxy":{
                "api": "anthropic-messages", "base_url": "https://proxy.example/v1"
            }}}"#,
    )
    .expect("write");

    let config = load_models_config(&path).expect("loads");
    let proxy = config.providers.get("my-claude-proxy").expect("provider block");
    assert_eq!(proxy.compat.cache_control_format.as_deref(), Some("anthropic"));

    // And an explicit refusal is still a refusal.
    std::fs::write(
        &path,
        r#"{"providers":{"my-claude-proxy":{
                "api": "anthropic-messages", "compat": {"cache_control_format": null}
            }}}"#,
    )
    .expect("write");
    let config = load_models_config(&path).expect("loads");
    assert!(config.providers["my-claude-proxy"].compat.cache_control_format.is_none());
}

#[test]
fn models_config_deserialize() {
    let json = r#"{
            "models": {
                "openai/gpt-4o": {
                    "api_key": "sk-test",
                    "base_url": "https://api.openai.com/v1",
                    "context_window": 128000,
                    "max_tokens": 16000,
                    "reasoning": true,
                    "modalities": ["text", "image"],
                    "thinking_level_map": {
                        "minimal": "low",
                        "ultra": "max"
                    }
                }
            },
            "providers": {
                "openai": {
                    "api_key": "sk-provider",
                    "compat": {
                        "supports_developer_role": true,
                        "max_tokens_field": "max_completion_tokens"
                    }
                }
            }
        }"#;

    let config: ModelsConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.models.len(), 1);
    assert_eq!(config.providers.len(), 1);

    let model = config.models.get("openai/gpt-4o").unwrap();
    assert_eq!(model.api_key.as_ref().map(|k| k.expose()), Some("sk-test"));
    assert_eq!(model.context_window, Some(128000));
    assert_eq!(model.reasoning, Some(true));
    // The JSON key is `modalities`; the field mirrors `ModelInfo::input`.
    assert_eq!(model.input.as_deref(), Some(["text".to_string(), "image".to_string()].as_slice()));
    assert_eq!(
        model.thinking_level_map.get("ultra").and_then(|value| value.as_deref()),
        Some("max")
    );

    let provider = config.providers.get("openai").unwrap();
    assert!(provider.compat.supports_developer_role);
    assert_eq!(provider.compat.max_tokens_field, "max_completion_tokens");
}

/// Every field here is optional, and a `models.json` written before one existed has to keep
/// parsing — it is a file on a user's disk, not something a release can migrate.
#[test]
fn an_entry_without_modalities_still_parses() {
    let json = r#"{"models": {"lmstudio/local": {"context_window": 32000}}}"#;
    let config: ModelsConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.models["lmstudio/local"].input, None, "absent, not empty");
    assert_eq!(config.models["lmstudio/local"].thinking, None, "and no starting level");
}

/// A model's starting reasoning level parses by its Luxe name, and an unknown one fails the
/// entry rather than being read as some neighboring level.
#[test]
fn a_model_can_declare_the_level_a_session_starts_at() {
    let json = r#"{"models": {"lmstudio/qwen3.8-27b": {"thinking": "medium"}}}"#;
    let config: ModelsConfig = serde_json::from_str(json).unwrap();
    assert_eq!(
        config.models["lmstudio/qwen3.8-27b"].thinking,
        Some(luxe_core::provider::ThinkingLevel::Medium)
    );

    // `max` is the documented alias for `ultra`, the one spelling that isn't its own name.
    let aliased = r#"{"models": {"x/y": {"thinking": "max"}}}"#;
    let config: ModelsConfig = serde_json::from_str(aliased).unwrap();
    assert_eq!(config.models["x/y"].thinking, Some(luxe_core::provider::ThinkingLevel::Ultra));

    let typo = r#"{"models": {"x/y": {"thinking": "mediumish"}}}"#;
    assert!(serde_json::from_str::<ModelsConfig>(typo).is_err(), "a typo is not a level");
}

/// A model's compat block is kept exactly as written, because only the keys a user typed may
/// override the provider's — see `ModelConfig::compat`. An unknown key parses and is dropped at
/// resolution, the same way an unknown sampling key reaches the endpoint and an unknown modality
/// costs one capability: a typo never fails the file.
#[test]
fn a_model_compat_block_parses_as_the_keys_that_were_written() {
    let json = r#"{
            "models": {
                "lmstudio/fable-fusion": { "compat": {
                    "thinking_format": "chat_template",
                    "supports_developer_role": false
                } },
                "lmstudio/qwen3.8-27b": { "context_window": 262144 }
            },
            "providers": { "lmstudio": { "compat": { "thinking_format": "openai" } } }
        }"#;
    let config: ModelsConfig = serde_json::from_str(json).unwrap();

    let written = &config.models["lmstudio/fable-fusion"].compat;
    assert_eq!(written.len(), 2, "the two keys written, and nothing serde would default");
    assert_eq!(written["thinking_format"], "chat_template");
    assert_eq!(written["supports_developer_role"], false);
    assert!(!written.contains_key("supports_usage_in_streaming"), "{written:?}");

    // A model that says nothing about compat has nothing to merge — distinct from writing the
    // defaults, which is exactly the distinction a typed struct here could not make.
    assert!(config.models["lmstudio/qwen3.8-27b"].compat.is_empty());

    // The provider's block stays typed and fully resolved: it is applied wholesale.
    assert_eq!(config.providers["lmstudio"].compat.thinking_format.as_deref(), Some("openai"));
}

/// A published sampling recipe is written as JSON and carried as JSON — no key is modelled,
/// because the servers Luxe doesn't model are exactly the reason the field exists.
#[test]
fn sampling_params_parse_as_free_form_json_at_both_layers() {
    let json = r#"{
            "models": { "lmstudio/qwen3.8-27b": { "sampling_params": {
                "temperature": 1.0, "top_k": 20, "min_p": 0.0, "repeat_penalty": 1.0
            } } },
            "providers": { "lmstudio": { "sampling_params": { "min_p": 0.05 } } }
        }"#;
    let config: ModelsConfig = serde_json::from_str(json).unwrap();

    let model = config.models.get("lmstudio/qwen3.8-27b").unwrap();
    assert_eq!(model.sampling_params["top_k"], 20);
    assert_eq!(model.sampling_params["min_p"], 0.0);
    assert_eq!(model.sampling_params["repeat_penalty"], 1.0);
    assert_eq!(config.providers.get("lmstudio").unwrap().sampling_params["min_p"], 0.05);

    // Absent is empty, so a config that never mentions sampling sends nothing.
    let bare: ModelsConfig =
        serde_json::from_str(r#"{"models":{"a/b":{}},"providers":{"a":{}}}"#).unwrap();
    assert!(bare.models.get("a/b").unwrap().sampling_params.is_empty());
    assert!(bare.providers.get("a").unwrap().sampling_params.is_empty());
}

#[test]
fn create_openai_compat_provider() {
    use luxe_core::Provider;

    let config = ProviderConfig {
        api_key: Some("sk-test".into()),
        base_url: Some("https://api.example.com/v1".into()),
        ..Default::default()
    };
    let provider = openai_compat_provider(config, CompatFlags::default());
    assert_eq!(provider.name(), "openai_compat");
}

/// A config written for an older Luxe keeps loading: retired flags are simply ignored, and the
/// ones that are still real are honored.
#[test]
fn compat_flags_anthropic() {
    let json = r#"{
            "supports_eager_tool_input_streaming": true,
            "supports_long_cache_retention": true,
            "force_adaptive_thinking": true,
            "cache_control_format": "anthropic"
        }"#;
    let flags: CompatFlags = serde_json::from_str(json).unwrap();
    assert!(flags.force_adaptive_thinking);
    assert_eq!(flags.cache_control_format.as_deref(), Some("anthropic"));
    assert!(flags.supports_long_cache_retention);
    assert!(!CompatFlags::default().supports_long_cache_retention, "opt-in, not a default");
}

#[test]
fn anthropic_provider_config() {
    let config = ProviderConfig {
        api_key: Some("sk-ant-test".into()),
        base_url: Some("https://api.anthropic.com/v1".into()),
        compat: CompatFlags { force_adaptive_thinking: false, ..Default::default() },
        ..Default::default()
    };
    assert_eq!(config.api_key.unwrap().expose(), "sk-ant-test");
    assert_eq!(config.base_url.unwrap(), "https://api.anthropic.com/v1");
}

/// The sync form goes through the bounded runner too: from the home folder, first line only, as
/// a password manager's entry prints the secret and then everything else it holds.
#[cfg(unix)]
#[test]
fn resolve_value_runs_a_command_like_the_request_path() {
    assert_eq!(resolve_value("!printf 'first\\nsecond'"), "first");
    let home = luxe_core::user_dirs::home_dir().unwrap();
    let home = std::fs::canonicalize(home).unwrap();
    assert_eq!(std::path::PathBuf::from(resolve_value("!pwd -P")), home);
}

/// The wire API decides the adapter, and Copilot — chat completions with its own headers — is the
/// one provider routed per model between its endpoints.
#[test]
fn each_api_kind_builds_its_own_adapter() {
    let auth = crate::auth::StaticAuth::bearer(None).into_arc();
    let model = luxe_core::provider::ModelInfo::default();
    let name_of = |api: ApiKind| {
        build_adapter(api, model.clone(), auth.clone(), None, CompatFlags::default())
            .name()
            .to_string()
    };
    assert_eq!(name_of(ApiKind::AnthropicMessages), "anthropic");
    assert_eq!(name_of(ApiKind::OpenaiCompletions), "openai_compat");
    assert_eq!(name_of(ApiKind::OpenaiResponses), "openai_responses");
    assert_eq!(name_of(ApiKind::OpenaiCodex), "openai_responses");
    let copilot = resolve_provider_compat("github-copilot", None);
    let built = build_adapter(ApiKind::OpenaiCompletions, model, auth, None, copilot);
    assert_eq!(built.name(), "github_copilot");
}
