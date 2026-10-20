use super::*;
use crate::config::{CompatFlags, CostConfig};

/// The real models.dev entry for Claude Opus 5, trimmed to what luxe reads.
const OPUS_5: &str = r#"{"anthropic":{"models":{"claude-opus-5":{
        "name":"Claude Opus 5","reasoning":true,"tool_call":true,
        "reasoning_options":[{"type":"effort","values":["low","medium","high","xhigh","max"]}],
        "modalities":{"input":["text","image","pdf"],"output":["text"]},
        "limit":{"context":1000000,"output":128000},
        "cost":{"input":5.0,"output":25.0,"cache_read":0.5,"cache_write":6.25}}}}}"#;

fn catalog(json: &str) -> Catalog {
    super::super::schema::parse(json).expect("fixture parses")
}

/// A compat block as a user wrote it — the keys, and nothing serde would have defaulted.
fn written(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match value {
        serde_json::Value::Object(map) => map,
        _ => unreachable!("fixture is an object"),
    }
}

#[test]
fn unknown_model_falls_back_to_provider_aware_defaults() {
    let (info, from) = resolve("anthropic", "claude-opus-5", &Sources::default());
    assert_eq!(info.context_window, DEFAULT_CONTEXT_WINDOW);
    assert_eq!(info.max_tokens, ANTHROPIC_FALLBACK_MAX_TOKENS);
    assert!(info.reasoning, "anthropic models reason by default");
    assert_eq!(from.context_window, Layer::Default);

    // Non-Anthropic omits max_tokens rather than forcing a value on a strict endpoint.
    let (other, _) = resolve("openai_compat", "some-local-model", &Sources::default());
    assert_eq!(other.max_tokens, 0);
    assert!(!other.reasoning);
}

/// The floor follows the *API*, not the brand. A Foundry or proxy deployment of Anthropic
/// Messages requires `max_tokens` just as much, and a custom deployment name is exactly the
/// case where no catalog layer supplies one.
#[test]
fn the_anthropic_floor_follows_the_wire_api_rather_than_the_provider_name() {
    let (foundry, _) = resolve("foundry-anthropic", "sonnet5-prod", &Sources::default());
    assert_eq!(foundry.max_tokens, ANTHROPIC_FALLBACK_MAX_TOKENS);
    assert!(foundry.reasoning);

    // A provider the catalog has never heard of, declared by hand.
    let declared = ProviderConfig { api: Some("anthropic-messages".into()), ..Default::default() };
    let (proxy, _) = resolve(
        "my-claude-proxy",
        "whatever",
        &Sources { provider: Some(&declared), ..Sources::default() },
    );
    assert_eq!(proxy.max_tokens, ANTHROPIC_FALLBACK_MAX_TOKENS);

    // And the OpenAI-shaped half of the same resource still omits it.
    let (openai, _) = resolve("foundry-openai", "my-deployment", &Sources::default());
    assert_eq!(openai.max_tokens, 0);
}

/// A deployment is whatever somebody named it. Its metadata belongs to the model it runs, in
/// whatever case Azure reports that model, and the wire id stays the deployment's name.
#[test]
fn a_deployment_borrows_the_metadata_of_the_model_it_runs() {
    let cache = catalog(
        r#"{"azure":{"models":{"kimi-k2.5":{"name":"Kimi K2.5","limit":{"context":262144,"output":32768}}}}}"#,
    );
    let provider = ProviderConfig {
        deployments: [("Kimi-K2-Thinking".to_string(), "Kimi-K2.5".to_string())].into(),
        ..Default::default()
    };
    let (info, from) = resolve(
        "foundry-completions",
        "Kimi-K2-Thinking",
        &Sources { cache: Some(&cache), provider: Some(&provider), ..<_>::default() },
    );
    assert_eq!(info.r#ref.id, "Kimi-K2-Thinking", "the wire id is never rewritten");
    assert_eq!(info.context_window, 262_144);
    assert_eq!(from.context_window, Layer::Cache);

    // A deployment named for its model, in Azure's capitals, needs no map at all
    let (named, _) = resolve(
        "foundry-completions",
        "KIMI-K2.5",
        &Sources { cache: Some(&cache), ..<_>::default() },
    );
    assert_eq!(named.context_window, 262_144);
}

#[test]
fn catalog_supplies_everything_config_omits() {
    let cache = catalog(OPUS_5);
    let (info, from) =
        resolve("anthropic", "claude-opus-5", &Sources { cache: Some(&cache), ..<_>::default() });

    assert_eq!(info.context_window, 1_000_000);
    assert_eq!(info.max_tokens, 128_000);
    assert_eq!(info.display_name.as_deref(), Some("Claude Opus 5"));
    assert!(info.reasoning);
    assert_eq!(info.cost.input, 5.0);
    assert_eq!(info.cost.cache_read, 0.5);
    assert_eq!(info.input, [Modality::Text, Modality::Image]);
    assert_eq!(info.thinking_level_map.get("xhigh"), Some(&Some("xhigh".into())));
    assert_eq!(info.thinking_level_map.get("ultra"), Some(&Some("max".into())));
    assert_eq!(from.context_window, Layer::Cache);

    // The whole point: the levels the model really supports are now reachable.
    use luxe_core::provider::ThinkingLevel;
    assert_eq!(info.effective_thinking_level(ThinkingLevel::Ultra), ThinkingLevel::Ultra);
    assert_eq!(info.effective_thinking_level(ThinkingLevel::XHigh), ThinkingLevel::XHigh);
}

/// Copied from models.dev, the newer list and the older single key both.
#[test]
fn long_prompt_tiers_come_through_from_the_catalog() {
    let cache = catalog(
        r#"{"anthropic":{"models":{
                "claude-haiku-5-5":{"cost":{"input":0.1,"output":0.5,"cache_read":0.01,
                    "cache_write":0.125,"tiers":[{"input":0.5,"output":2.5,"cache_read":0.05,
                    "cache_write":0.625,"tier":{"type":"context","size":100000}}]}},
                "claude-sonnet-4-5":{"cost":{"input":3,"output":15,"cache_read":0.3,
                    "cache_write":3.75,"context_over_200k":{"input":6,"output":22.5,
                    "cache_read":0.6,"cache_write":7.5}}},
                "claude-odd":{"cost":{"input":1,"output":5,"tiers":[{"input":2,"output":9,
                    "tier":{"type":"requests","size":10}}]}}}}}"#,
    );
    let sources = Sources { cache: Some(&cache), ..<_>::default() };
    let (haiku, _) = resolve("anthropic", "claude-haiku-5-5", &sources);
    assert_eq!(
        haiku.cost.tiers,
        [luxe_core::CostTier {
            above_tokens: 100_000,
            input: 0.5,
            output: 2.5,
            cache_read: 0.05,
            cache_write: 0.625
        }]
    );
    let (sonnet, _) = resolve("anthropic", "claude-sonnet-4-5", &sources);
    assert_eq!(sonnet.cost.tiers.len(), 1);
    assert_eq!(sonnet.cost.tiers[0].above_tokens, 200_000);
    assert_eq!(sonnet.cost.tiers[0].output, 22.5);
    let (odd, _) = resolve("anthropic", "claude-odd", &sources);
    assert!(odd.cost.tiers.is_empty(), "a tier on something other than context is not guessed at");
}

#[test]
fn cache_overrides_snapshot_and_config_overrides_both() {
    let snapshot = catalog(
        r#"{"anthropic":{"models":{"claude-opus-5":{"name":"Old Name",
                "limit":{"context":200000,"output":32000},"cost":{"input":9.0,"output":9.0}}}}}"#,
    );
    let cache = catalog(OPUS_5);
    let model = ModelConfig { context_window: Some(500_000), ..Default::default() };

    let (info, from) = resolve(
        "anthropic",
        "claude-opus-5",
        &Sources {
            snapshot: Some(&snapshot),
            cache: Some(&cache),
            model: Some(&model),
            provider: None,
        },
    );

    // Config wins where it speaks …
    assert_eq!(info.context_window, 500_000);
    assert_eq!(from.context_window, Layer::Config);
    // … the fresher cache wins over the snapshot elsewhere …
    assert_eq!(info.max_tokens, 128_000);
    assert_eq!(info.display_name.as_deref(), Some("Claude Opus 5"));
    assert_eq!(from.max_tokens, Layer::Cache);
    // … and a field only the snapshot knows still survives when the cache is missing it.
    let (snap_only, snap_from) = resolve(
        "anthropic",
        "claude-opus-5",
        &Sources { snapshot: Some(&snapshot), ..<_>::default() },
    );
    assert_eq!(snap_only.max_tokens, 32_000);
    assert_eq!(snap_from.max_tokens, Layer::Snapshot);
}

#[test]
fn config_can_disable_reasoning_and_override_cost() {
    let cache = catalog(OPUS_5);
    let model = ModelConfig {
        reasoning: Some(false),
        cost: Some(CostConfig { input: 1.0, output: 2.0, ..Default::default() }),
        ..Default::default()
    };
    let (info, from) = resolve(
        "anthropic",
        "claude-opus-5",
        &Sources { cache: Some(&cache), model: Some(&model), ..<_>::default() },
    );
    // An explicit `false` survives even though the catalog gave a thinking map.
    assert!(!info.reasoning);
    assert_eq!(from.reasoning, Layer::Config);
    assert_eq!(info.cost.input, 1.0);
    assert_eq!(from.cost, Layer::Config);
}

/// A published sampling recipe is the point of the passthrough, so both layers have to reach
/// the resolved model — the provider's baseline for a local server hosting several builds, and
/// the model's own keys over the top of it.
#[test]
fn sampling_params_merge_per_key_with_the_model_on_top() {
    let map = |json: serde_json::Value| match json {
        serde_json::Value::Object(map) => map,
        _ => unreachable!("fixture is an object"),
    };
    let provider = ProviderConfig {
        sampling_params: map(serde_json::json!({ "min_p": 0.0, "top_k": 40 })),
        ..Default::default()
    };
    let model = ModelConfig {
        sampling_params: map(serde_json::json!({ "top_k": 20, "temperature": 1.0 })),
        ..Default::default()
    };

    let (info, from) = resolve(
        "lmstudio",
        "qwen3.8-27b",
        &Sources { provider: Some(&provider), model: Some(&model), ..<_>::default() },
    );
    assert_eq!(info.sampling_params["min_p"], 0.0, "the provider baseline survives");
    assert_eq!(info.sampling_params["top_k"], 20, "the model wins the key it names");
    assert_eq!(info.sampling_params["temperature"], 1.0);
    assert_eq!(from.sampling_params, Layer::Config);

    // Nothing configured means nothing sent — never an invented default.
    let (bare, bare_from) = resolve("lmstudio", "qwen3.8-27b", &Sources::default());
    assert!(bare.sampling_params.is_empty());
    assert_eq!(bare_from.sampling_params, Layer::Default);
}

#[test]
fn a_config_thinking_map_replaces_the_catalog_one() {
    let cache = catalog(OPUS_5);
    let model = ModelConfig {
        thinking_level_map: HashMap::from([("high".into(), Some("on".into()))]),
        ..Default::default()
    };
    let (info, from) = resolve(
        "anthropic",
        "claude-opus-5",
        &Sources { cache: Some(&cache), model: Some(&model), ..<_>::default() },
    );
    assert_eq!(info.thinking_level_map.len(), 1);
    assert_eq!(info.thinking_level_map.get("high"), Some(&Some("on".into())));
    assert_eq!(from.thinking_level_map, Layer::Config);
    // Implied by the map, since the user didn't say otherwise.
    assert!(info.reasoning);
}

/// A model's compat keys override its provider's *one key at a time*.
///
/// The trap this closes is the one `layer_catalog_compat` closed a level up, and it bites harder
/// here: a model block is normally about a context window, so a `thinking_format` written beside
/// one must not also assert every flag serde would default. If it did, this entry would switch
/// off streamed usage (defaults on) and Anthropic prompt caching (a `null` format) for one model
/// while its sibling on the same endpoint kept both.
#[test]
fn a_models_compat_keys_merge_over_its_providers_rather_than_replacing_them() {
    let provider = ProviderConfig {
        compat: CompatFlags {
            thinking_format: Some("openai".into()),
            supports_reasoning_effort: true,
            supports_developer_role: true,
            cache_control_format: Some("anthropic".into()),
            max_tokens_field: "max_completion_tokens".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let model = ModelConfig {
        compat: written(serde_json::json!({
            "thinking_format": "chat_template",
            "supports_developer_role": false
        })),
        ..Default::default()
    };

    let (info, from) = resolve(
        "lmstudio",
        "qwen3.6-27b-fable-fusion",
        &Sources { provider: Some(&provider), model: Some(&model), ..<_>::default() },
    );
    let compat = info.compat.expect("the model resolved its own flags");
    // What it wrote …
    assert_eq!(compat.thinking_format.as_deref(), Some("chat_template"));
    assert!(!compat.supports_developer_role, "this template raises on a `developer` role");
    // … and everything it did not, still the provider's.
    assert!(compat.supports_reasoning_effort);
    assert_eq!(compat.cache_control_format.as_deref(), Some("anthropic"));
    assert_eq!(compat.max_tokens_field, "max_completion_tokens");
    assert!(compat.supports_usage_in_streaming, "a default nobody wrote is not an override");
    assert_eq!(from.compat, Layer::Config);

    // The sibling on the same endpoint says nothing, so it has no flags of its own and the
    // adapter keeps the ones it was built with.
    let (sibling, sibling_from) = resolve(
        "lmstudio",
        "qwen3.8-27b",
        &Sources { provider: Some(&provider), ..<_>::default() },
    );
    assert_eq!(sibling.compat, None);
    assert_eq!(sibling_from.compat, Layer::Default);
}

/// With no provider block written, the base is the catalog's flags — the same ones the adapter is
/// built with.
///
/// Resolved through one function for that reason: a second implementation of "the provider's
/// compat" is how a model ends up merged over flags nothing is actually running.
#[test]
fn a_models_compat_merges_over_the_catalog_when_no_provider_block_exists() {
    let model = ModelConfig {
        compat: written(serde_json::json!({ "force_adaptive_thinking": true })),
        ..Default::default()
    };
    let (info, _) = resolve(
        "anthropic",
        "claude-opus-9-unreleased",
        &Sources { model: Some(&model), ..<_>::default() },
    );
    let compat = info.compat.expect("resolved");
    assert!(compat.force_adaptive_thinking, "what the model wrote");
    assert!(
        compat.cache_control_format.is_some(),
        "and Anthropic's catalog default for prompt caching survives"
    );
    // Forcing adaptive thinking on one model exposes `ultra` on that model, as it does when the
    // provider forces it — the two paths reach the same place.
    assert_eq!(info.thinking_level_map.get("ultra"), Some(&Some("max".into())));
}

/// A key that chooses the *transport* is refused, because an adapter is built once per provider.
///
/// Honoring it here would be a lie: the request would be shaped for `/responses` and posted to
/// chat/completions. The rest of the block still applies — one bad key costs that key.
#[test]
fn a_model_may_not_choose_the_transport() {
    let provider = ProviderConfig {
        compat: CompatFlags { use_responses_api: true, ..Default::default() },
        ..Default::default()
    };
    let model = ModelConfig {
        compat: written(serde_json::json!({
            "use_responses_api": false,
            "supports_developer_role": true
        })),
        ..Default::default()
    };
    let (info, _) = resolve(
        "openai",
        "gpt-5",
        &Sources { provider: Some(&provider), model: Some(&model), ..<_>::default() },
    );
    let compat = info.compat.expect("the rest of the block still resolved");
    assert!(compat.use_responses_api, "the provider's transport choice stands");
    assert!(compat.supports_developer_role, "and the shaping key it wrote applies");

    // A block of nothing but refused keys leaves the model with no flags at all, rather than a
    // copy of its provider's that would drift from them.
    let only_refused = ModelConfig {
        compat: written(serde_json::json!({ "use_responses_api": false })),
        ..Default::default()
    };
    let (bare, from) = resolve(
        "openai",
        "gpt-5",
        &Sources { provider: Some(&provider), model: Some(&only_refused), ..<_>::default() },
    );
    assert_eq!(bare.compat, None);
    assert_eq!(from.compat, Layer::Default);
}

/// A value of the wrong shape costs the block, never the file.
///
/// The same rule the modality list follows: `models.json` is a file on a user's disk, and a typo
/// in one model's compat must not take down the launch — it falls back to the provider's flags
/// and says so in the log.
#[test]
fn a_malformed_model_compat_block_falls_back_to_the_providers() {
    let provider = ProviderConfig {
        compat: CompatFlags { thinking_format: Some("openai".into()), ..Default::default() },
        ..Default::default()
    };
    let model = ModelConfig {
        compat: written(serde_json::json!({ "thinking_format": 42 })),
        ..Default::default()
    };
    let (info, from) = resolve(
        "lmstudio",
        "typo",
        &Sources { provider: Some(&provider), model: Some(&model), ..<_>::default() },
    );
    assert_eq!(info.compat, None, "nothing model-specific survives a value it cannot read");
    assert_eq!(from.compat, Layer::Default);
}

/// A model's starting reasoning level is config-only — no catalog has an opinion about how much
/// *your* patience is worth — and it is clamped to a level the model can actually reach, so a
/// default the map forbids resolves to the nearest one it allows instead of silently doing
/// nothing.
#[test]
fn a_config_thinking_default_resolves_and_clamps_to_a_reachable_level() {
    let reachable = ModelConfig {
        thinking: Some(ThinkingLevel::Medium),
        thinking_level_map: HashMap::from([
            ("off".into(), Some("none".into())),
            ("medium".into(), Some("medium".into())),
            ("high".into(), Some("high".into())),
        ]),
        ..Default::default()
    };
    let (info, from) =
        resolve("lmstudio", "qwen3.8-27b", &Sources { model: Some(&reachable), ..<_>::default() });
    assert_eq!(info.default_thinking, Some(ThinkingLevel::Medium));
    assert_eq!(from.default_thinking, Layer::Config);
    assert!(info.reasoning, "the map implies it, as it does for any other entry");

    // A level the same file marks unsupported clamps rather than reaching the wire.
    let forbidden = ModelConfig { thinking: Some(ThinkingLevel::XHigh), ..reachable.clone() };
    let (clamped, _) =
        resolve("lmstudio", "qwen3.8-27b", &Sources { model: Some(&forbidden), ..<_>::default() });
    assert_eq!(clamped.default_thinking, Some(ThinkingLevel::High));

    // And a model that cannot think at all cannot start anywhere but off.
    let thoughtless = ModelConfig {
        reasoning: Some(false),
        thinking: Some(ThinkingLevel::High),
        ..Default::default()
    };
    let (off, _) =
        resolve("lmstudio", "plain", &Sources { model: Some(&thoughtless), ..<_>::default() });
    assert_eq!(off.default_thinking, Some(ThinkingLevel::Off));

    // Nothing configured means nothing declared — never an invented starting level.
    let (bare, bare_from) = resolve(
        "anthropic",
        "claude-opus-5",
        &Sources { cache: Some(&catalog(OPUS_5)), ..<_>::default() },
    );
    assert_eq!(bare.default_thinking, None);
    assert_eq!(bare_from.default_thinking, Layer::Default);
}

/// The gap this field exists to close: a local endpoint resolves entirely from config, so
/// without it a vision model loaded with its projector reported `text` and nothing the user
/// could write changed that.
#[test]
fn a_config_modality_list_overrides_the_default() {
    let model =
        ModelConfig { input: Some(vec!["text".into(), "image".into()]), ..Default::default() };
    let (info, from) = resolve(
        "lmstudio",
        "some-vision-model",
        &Sources { model: Some(&model), ..<_>::default() },
    );
    assert_eq!(info.input, [Modality::Text, Modality::Image]);
    assert_eq!(from.input, Layer::Config, "and it says the answer came from the config");
}

/// The list is a closed claim about the endpoint, so removing a modality has to stick even
/// where the catalog still advertises it — the same rule the thinking map follows.
#[test]
fn a_config_modality_list_replaces_the_catalog_one() {
    let cache = catalog(
        r#"{"openai":{"models":{"gpt-5":{"modalities":{"input":["text","image"]},
               "limit":{"context":400000,"output":128000}}}}}"#,
    );
    let model = ModelConfig { input: Some(vec!["text".into()]), ..Default::default() };

    let (catalog_only, _) =
        resolve("openai", "gpt-5", &Sources { cache: Some(&cache), ..<_>::default() });
    assert_eq!(catalog_only.input, [Modality::Text, Modality::Image], "the catalog claims both");

    let (info, from) = resolve(
        "openai",
        "gpt-5",
        &Sources { cache: Some(&cache), model: Some(&model), ..<_>::default() },
    );
    assert_eq!(info.input, [Modality::Text], "and the config takes one away for good");
    assert_eq!(from.input, Layer::Config);
}

/// An unrecognized value costs that one capability, never the file. Falling through to an
/// empty list instead would render as a blank row and claim the model accepts nothing at all.
#[test]
fn config_modalities_ignore_unrecognized_values_without_erroring() {
    let model = ModelConfig { input: Some(vec!["pdf".into()]), ..Default::default() };
    let (info, _) = resolve("lmstudio", "x", &Sources { model: Some(&model), ..<_>::default() });
    assert_eq!(info.input, [Modality::Text]);

    // A recognized value alongside an unknown one still lands, and case does not matter.
    let mixed = ModelConfig {
        input: Some(vec!["Image".into(), "audio".into(), "IMAGE".into()]),
        ..Default::default()
    };
    let (info, _) = resolve("lmstudio", "x", &Sources { model: Some(&mixed), ..<_>::default() });
    assert_eq!(info.input, [Modality::Image], "deduplicated, and the unknown one dropped");
}

#[test]
fn provider_alias_routes_a_custom_name_to_its_models_dev_id() {
    let cache = catalog(
        r#"{"alibaba":{"models":{"qwen3-max":{"limit":{"context":262144,"output":65536}}}}}"#,
    );
    let provider = ProviderConfig { models_dev: Some("alibaba".into()), ..Default::default() };
    let (info, _) = resolve(
        "my-qwen-proxy",
        "qwen3-max",
        &Sources { cache: Some(&cache), provider: Some(&provider), ..<_>::default() },
    );
    assert_eq!(info.context_window, 262_144);

    // Without the hint the name matches nothing, which is right for an unknown endpoint.
    let (bare, _) =
        resolve("my-qwen-proxy", "qwen3-max", &Sources { cache: Some(&cache), ..<_>::default() });
    assert_eq!(bare.context_window, DEFAULT_CONTEXT_WINDOW);
}

/// The catalog's own account of which dial a model exposes decides how the Anthropic adapter
/// asks for thinking, so it has to survive resolution intact — and an effort-only list is a
/// closed set, which is what stops luxe asking for a level the endpoint never advertised.
#[test]
fn declared_reasoning_dials_survive_resolution() {
    // Real 2026-07 shapes: Sonnet 5 (effort only), Sonnet 4.5 (a token budget), Opus 4.5 (both).
    let cache = catalog(
        r#"{"anthropic":{"models":{
                "claude-sonnet-5":{"reasoning":true,"reasoning_options":[{"type":"toggle"},
                    {"type":"effort","values":["low","medium","high","xhigh","max"]}]},
                "claude-sonnet-4-5":{"reasoning":true,
                    "reasoning_options":[{"min":1024,"type":"budget_tokens"}]},
                "claude-opus-4-5":{"reasoning":true,"reasoning_options":[
                    {"type":"effort","values":["low","medium","high"]},
                    {"min":1024,"type":"budget_tokens"}]}}}}"#,
    );
    let resolve_cached =
        |id: &str| resolve("anthropic", id, &Sources { cache: Some(&cache), ..<_>::default() }).0;

    let sonnet_5 = resolve_cached("claude-sonnet-5");
    assert!(sonnet_5.thinking_controls.effort_only(), "effort is the only dial");
    // Every level it names is reachable …
    assert_eq!(sonnet_5.effective_thinking_level(ThinkingLevel::XHigh), ThinkingLevel::XHigh);
    assert_eq!(sonnet_5.effective_thinking_level(ThinkingLevel::Ultra), ThinkingLevel::Ultra);
    // … and one it doesn't clamps instead of inventing an effort value.
    assert_eq!(sonnet_5.thinking_level_map.get("minimal"), Some(&None));
    assert_eq!(sonnet_5.effective_thinking_level(ThinkingLevel::Minimal), ThinkingLevel::Low);
    // Off keeps its own switch — a mapped `off` would clamp "no thinking" up into thinking.
    assert!(!sonnet_5.thinking_level_map.contains_key("off"));
    assert_eq!(sonnet_5.effective_thinking_level(ThinkingLevel::Off), ThinkingLevel::Off);

    // A token budget expresses any depth, so nothing is closed off and the levels stay open.
    let sonnet_4_5 = resolve_cached("claude-sonnet-4-5");
    assert!(!sonnet_4_5.thinking_controls.effort_only());
    assert!(sonnet_4_5.thinking_controls.budget_tokens);
    assert!(sonnet_4_5.thinking_level_map.is_empty());
    assert_eq!(sonnet_4_5.effective_thinking_level(ThinkingLevel::Medium), ThinkingLevel::Medium);

    // Both dials: still not effort-only, so the adapter keeps extended thinking.
    let opus_4_5 = resolve_cached("claude-opus-4-5");
    assert!(opus_4_5.thinking_controls.effort);
    assert!(opus_4_5.thinking_controls.budget_tokens);
    assert!(!opus_4_5.thinking_controls.effort_only());
    assert!(!opus_4_5.thinking_level_map.contains_key("minimal"), "the list only adds");
}

/// Whether reasoning can be switched off is a claim the catalog has to make out loud, because
/// the endpoints that refuse are the ones that answer 400 to a disable. Real 2026-08 shapes
/// from the models.dev `openrouter` block, one per way of learning it.
#[test]
fn an_off_switch_is_claimed_only_where_the_catalog_says_so() {
    let cache = catalog(
        r#"{"openrouter":{"models":{
                "anthropic/claude-sonnet-5":{"reasoning":true,"reasoning_options":[{"type":"toggle"},
                    {"type":"effort","values":["low","medium","high","xhigh","max"]}]},
                "openai/gpt-5.6-sol":{"reasoning":true,"reasoning_options":[
                    {"type":"effort","values":["none","low","medium","high","xhigh","max"]}]},
                "google/gemini-3.5-flash":{"reasoning":true,"reasoning_options":[
                    {"type":"effort","values":["minimal","low","medium","high"]}]},
                "moonshotai/kimi-k2-thinking":{"reasoning":true}}}}"#,
    );
    let disables = |id: &str| {
        resolve("openrouter", id, &Sources { cache: Some(&cache), ..<_>::default() })
            .0
            .thinking_controls
            .can_disable
    };

    assert!(disables("anthropic/claude-sonnet-5"), "a toggle option is the plain statement");
    assert!(disables("openai/gpt-5.6-sol"), "`none` among the efforts says the same thing");
    assert!(!disables("google/gemini-3.5-flash"), "a closed effort list without `none`");
    assert!(!disables("moonshotai/kimi-k2-thinking"), "silence is not permission");
}

#[test]
fn adaptive_thinking_compat_still_adds_ultra() {
    let provider = ProviderConfig {
        compat: CompatFlags { force_adaptive_thinking: true, ..Default::default() },
        ..Default::default()
    };
    let (info, _) =
        resolve("anthropic", "claude-x", &Sources { provider: Some(&provider), ..<_>::default() });
    assert_eq!(info.thinking_level_map.get("ultra"), Some(&Some("max".into())));
    assert!(info.reasoning);
}

#[test]
fn pinned_date_suffix_inherits_the_base_model() {
    let cache = catalog(OPUS_5);
    let (info, from) = resolve(
        "anthropic",
        "claude-opus-5-20260724",
        &Sources { cache: Some(&cache), ..<_>::default() },
    );
    // Metadata is inherited …
    assert_eq!(info.context_window, 1_000_000);
    assert_eq!(from.context_window, Layer::Cache);
    // … but the wire id is untouched.
    assert_eq!(info.r#ref.id, "claude-opus-5-20260724");
}
