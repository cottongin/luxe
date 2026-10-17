use super::table::foundry::{
    FOUNDRY_ANTHROPIC_PREFIXES, FOUNDRY_COMPLETIONS_PREFIXES, FOUNDRY_NON_RESPONSES_PREFIXES,
};
use super::*;

#[test]
fn ids_are_unique_and_lowercase() {
    let mut ids: Vec<&str> = CATALOG.iter().map(|descriptor| &*descriptor.id).collect();
    let count = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), count, "duplicate provider id in the catalog");
    for descriptor in CATALOG {
        assert_eq!(
            descriptor.id.to_ascii_lowercase(),
            descriptor.id,
            "{} must be lowercase",
            descriptor.id
        );
        assert!(!descriptor.name.is_empty());
    }
}

/// A provider nobody can log in to is a dead row in the wizard.
#[test]
fn every_provider_offers_at_least_one_method() {
    for descriptor in CATALOG {
        assert!(!descriptor.methods().is_empty(), "{} offers no login method", descriptor.id);
    }
}

/// The whole point of the catalog is that its defaults are right. A base URL without the
/// version segment 404s at request time, which is a miserable thing to debug.
#[test]
fn base_urls_are_absolute_and_unslashed() {
    for descriptor in CATALOG {
        let Some(url) = descriptor.base_url else {
            assert!(
                descriptor.needs_base_url,
                "{} has no base URL and doesn't ask for one",
                descriptor.id
            );
            continue;
        };
        assert!(
            url.starts_with("http://") || url.starts_with("https://"),
            "{} base URL is not absolute: {url}",
            descriptor.id
        );
        assert!(!url.ends_with('/'), "{} base URL has a trailing slash: {url}", descriptor.id);
    }
}

/// A renamed models.dev provider would silently degrade every model to fallback metadata.
///
/// Resolution goes through [`models_dev_provider`], the same mapping lookup uses, because most
/// providers carry no explicit `models_dev` override — they map verbatim or through the alias
/// table. Reading the field directly skipped all of them, which is how `google` and `deepseek`
/// kept preference lists upstream had already retired.
#[test]
fn models_dev_ids_exist_in_the_embedded_snapshot() {
    let snapshot = crate::catalog::snapshot();
    for descriptor in CATALOG {
        if descriptor.keyless || descriptor.base_url.is_none() {
            continue; // a local server or a BYO endpoint has no upstream vendor to check
        }
        let id = crate::catalog::alias::models_dev_provider(&descriptor.id, None);
        let count = snapshot.provider_models(id).count();
        assert!(
            count > 0,
            "{}: models.dev id '{id}' has no models in the embedded snapshot — fix the id or \
                 rerun scripts/update-models-snapshot.sh",
            descriptor.id
        );
    }
}

/// A preference list of ids nothing recognizes is worse than no list: it would hand a new user a
/// model their account can't serve. At least one entry must still exist upstream.
///
/// The leading id is the one a new account actually gets, so it is held to the stricter bar: a
/// dead first choice is a silent demotion to the runner-up on every fresh install.
#[test]
fn preferred_models_still_exist_in_the_embedded_snapshot() {
    let snapshot = crate::catalog::snapshot();
    for descriptor in CATALOG {
        if descriptor.preferred_models.is_empty() {
            continue;
        }
        let models_dev = crate::catalog::alias::models_dev_provider(&descriptor.id, None);
        let known = |id: &str| {
            snapshot.model(models_dev, id).is_some()
                || crate::catalog::alias::normalize_model_id(id)
                    .is_some_and(|base| snapshot.model(models_dev, base).is_some())
        };
        let live = descriptor.preferred_models.iter().filter(|id| known(id)).count();
        assert!(
            live > 0,
            "{}: none of its preferred models {:?} exist under models.dev '{models_dev}' \
                 — refresh the list or the snapshot",
            descriptor.id,
            descriptor.preferred_models
        );
        let first = descriptor.preferred_models[0];
        assert!(
            known(first),
            "{}: its first-choice model '{first}' is gone from models.dev '{models_dev}', so \
                 every new account silently falls through to the next entry — promote a live id",
            descriptor.id
        );
    }
}

/// A served rule is a promise: an explicit id the metadata catalog doesn't know would show up
/// as a model with no context window, and a prefix nothing matches would empty the picker.
#[test]
fn served_rules_match_real_models_and_cover_the_preferred_ones() {
    let snapshot = crate::catalog::snapshot();
    for descriptor in CATALOG {
        if descriptor.served_models == ServedModels::Catalog {
            continue;
        }
        let models_dev = descriptor.models_dev.expect("a served rule needs metadata to hang on");
        match descriptor.served_models {
            ServedModels::Catalog => unreachable!("skipped above"),
            ServedModels::Only(ids) => {
                assert!(!ids.is_empty(), "{}: an empty list serves nothing", descriptor.id);
                for id in ids {
                    assert!(
                        snapshot.model(models_dev, id).is_some(),
                        "{}: served model '{id}' is unknown to models.dev '{models_dev}'",
                        descriptor.id
                    );
                }
            }
            ServedModels::Prefixed(prefixes) | ServedModels::Unprefixed(prefixes) => {
                assert!(
                    !prefixes.is_empty(),
                    "{}: a rule with no prefixes says nothing",
                    descriptor.id
                );
                for prefix in prefixes {
                    assert!(
                        snapshot.provider_models(models_dev).any(|(id, _)| id.starts_with(prefix)),
                        "{}: no models.dev '{models_dev}' model starts with '{prefix}'",
                        descriptor.id
                    );
                }
            }
        }
        // Defaulting to a model the backend refuses is the bug this pairing prevents.
        for preferred in descriptor.preferred_models {
            assert!(
                descriptor.served_models.serves(preferred),
                "{}: prefers '{preferred}', which it does not serve",
                descriptor.id
            );
        }
    }
}

/// The three Foundry providers share one models.dev catalog and must partition it **exactly**.
/// A model two of them serve is a coin toss over which path a request takes; a model none of
/// them serves is one nobody can reach through a picker.
#[test]
fn the_three_foundry_surfaces_partition_the_azure_catalog() {
    let snapshot = crate::catalog::snapshot();
    let surfaces: Vec<&ProviderDescriptor> =
        ["foundry-openai", "foundry-anthropic", "foundry-completions"]
            .iter()
            .map(|id| provider_descriptor(id).expect("in catalog"))
            .collect();
    for surface in &surfaces {
        assert_eq!(
            surface.models_dev,
            Some("azure"),
            "{}: one catalog, three surfaces",
            surface.id
        );
    }

    let mut counts = [0usize; 3];
    for (id, _) in snapshot.provider_models("azure") {
        let serving: Vec<&str> = surfaces
            .iter()
            .filter(|surface| surface.served_models.serves(id))
            .map(|surface| &*surface.id)
            .collect();
        assert_eq!(serving.len(), 1, "'{id}' is served by {serving:?}, not by exactly one");
        let at = surfaces.iter().position(|s| s.id == serving[0]).expect("known surface");
        counts[at] += 1;
    }
    for (surface, count) in surfaces.iter().zip(counts) {
        assert!(count >= 5, "{} serves only {count} models", surface.id);
    }

    // The split is the publisher, so a deployment name Luxe has never heard of still lands on
    // the OpenAI-shaped surface rather than nowhere.
    let served_by = |id: &str| -> Vec<&str> {
        surfaces
            .iter()
            .filter(|surface| surface.served_models.serves(id))
            .map(|surface| &*surface.id)
            .collect()
    };
    assert_eq!(served_by("my-own-deployment"), vec!["foundry-openai"]);
    assert_eq!(served_by("claude-something-new"), vec!["foundry-anthropic"]);
    assert_eq!(served_by("deepseek-v4-pro"), vec!["foundry-completions"]);
}

/// `FOUNDRY_NON_RESPONSES_PREFIXES` is a hand-written union because stable Rust cannot
/// concatenate const slices. Nothing but this stops the three rules drifting apart into a model
/// served twice, or one served nowhere.
#[test]
fn the_responses_exclusion_list_is_exactly_the_other_two_surfaces() {
    let mut union: Vec<&str> =
        FOUNDRY_ANTHROPIC_PREFIXES.iter().chain(FOUNDRY_COMPLETIONS_PREFIXES).copied().collect();
    union.sort_unstable();
    let mut excluded: Vec<&str> = FOUNDRY_NON_RESPONSES_PREFIXES.to_vec();
    excluded.sort_unstable();
    assert_eq!(excluded, union);
}

/// Each surface reaches its own path under one resource, and a root that already names the path
/// is left alone — someone who pasted a full target URI must not get
/// `/anthropic/v1/anthropic/v1`.
#[test]
fn a_foundry_resource_root_becomes_every_endpoint() {
    let root = "https://acme-foundry.services.ai.azure.com";
    let openai = provider_descriptor("foundry-openai").expect("in catalog");
    let anthropic = provider_descriptor("foundry-anthropic").expect("in catalog");
    let completions = provider_descriptor("foundry-completions").expect("in catalog");

    assert_eq!(openai.endpoint_for(root), "https://acme-foundry.services.ai.azure.com/openai/v1");
    // Two APIs on one path: the adapter decides `/responses` against `/chat/completions`.
    assert_eq!(completions.endpoint_for(root), openai.endpoint_for(root));
    assert_ne!(completions.api, openai.api);
    assert_eq!(
        anthropic.endpoint_for(root),
        "https://acme-foundry.services.ai.azure.com/anthropic/v1"
    );
    assert_eq!(
        anthropic.endpoint_for(&format!("{root}/anthropic/v1/")),
        anthropic.endpoint_for(root)
    );
    // A provider with no resource shape takes the endpoint as typed.
    assert_eq!(provider_descriptor("azure").expect("in catalog").endpoint_for(root), root);
}

/// The three things a user actually has — the resource name, the root, and a target URI copied
/// off either deployment — all have to reduce to the same root, or half the pair 404s.
#[test]
fn every_way_of_naming_a_foundry_resource_reduces_to_the_same_root() {
    let root = "https://acme-foundry.services.ai.azure.com";
    let anthropic = provider_descriptor("foundry-anthropic").expect("in catalog");
    let openai = provider_descriptor("foundry-openai").expect("in catalog");

    for typed in [
        "acme-foundry",
        root,
        "acme-foundry.services.ai.azure.com",
        "https://acme-foundry.services.ai.azure.com/",
        "https://acme-foundry.services.ai.azure.com/anthropic/v1/messages",
        "https://acme-foundry.services.ai.azure.com/openai/v1/responses",
        "  https://acme-foundry.services.ai.azure.com/anthropic  ",
    ] {
        assert_eq!(anthropic.resource_root(typed), root, "typed {typed:?}");
        assert_eq!(openai.resource_root(typed), root, "typed {typed:?}");
    }

    // A resource whose own name contains a path marker must not be truncated to its scheme.
    let awkward = "https://openai-eastus.services.ai.azure.com";
    assert_eq!(anthropic.resource_root(awkward), awkward);
    // A gateway that fronts the resource under a path of its own keeps it.
    assert_eq!(
        openai.endpoint_for(&openai.resource_root("https://gw.corp.example/foundry")),
        "https://gw.corp.example/foundry/openai/v1"
    );
}

/// The group is the whole reason several ids exist, and it has to be closed: logging in to any
/// member configures the others, so a one-way link leaves part of a resource unreachable.
#[test]
fn paired_providers_form_a_closed_group() {
    for descriptor in CATALOG {
        if descriptor.paired_with.is_empty() {
            continue;
        }
        let group = |d: &'static ProviderDescriptor| {
            let mut ids: Vec<&str> =
                std::iter::once(&*d.id).chain(d.paired_with.iter().copied()).collect();
            ids.sort_unstable();
            ids
        };
        let mine = group(descriptor);
        for id in descriptor.paired_with {
            let sibling = provider_descriptor(id).expect("a pair names a real provider");
            assert_eq!(
                group(sibling),
                mine,
                "{} and {} disagree about who shares their resource",
                descriptor.id,
                sibling.id
            );
            assert_ne!(
                sibling.api, descriptor.api,
                "{} and {} would be the same adapter",
                descriptor.id, sibling.id
            );
            assert!(
                sibling.endpoint.is_some(),
                "{}: a shared resource means every member derives its own endpoint",
                sibling.id
            );
        }
        assert!(
            descriptor.endpoint.is_some(),
            "{}: a shared resource means every member derives its own endpoint",
            descriptor.id
        );
        assert!(
            std::iter::once(descriptor)
                .chain(descriptor.paired_with.iter().filter_map(|id| provider_descriptor(id)))
                .any(|member| member.lists_models),
            "{}: no member of the group can verify a credential",
            descriptor.id
        );
    }
}

/// Foundry's Anthropic passthrough speaks Anthropic Messages and authenticates the Azure way,
/// which is the entire reason the key style is not derived from the API.
#[test]
fn foundry_sends_both_key_headers_where_the_passthrough_wants_them() {
    let anthropic = provider_descriptor("foundry-anthropic").expect("in catalog");
    assert_eq!(anthropic.api, ApiKind::AnthropicMessages);
    assert_eq!(anthropic.key_style(), KeyStyle::FoundryKey);
    // The OpenAI-shaped half takes a bearer token, exactly as Microsoft's own samples send it.
    assert_eq!(
        provider_descriptor("foundry-openai").expect("in catalog").key_style(),
        KeyStyle::Bearer
    );
    // Everyone else still derives from their API.
    assert_eq!(
        provider_descriptor("anthropic").expect("in catalog").key_style(),
        KeyStyle::AnthropicKey
    );
    assert_eq!(provider_descriptor("openai").expect("in catalog").key_style(), KeyStyle::Bearer);
}

#[test]
fn aliases_resolve_to_real_providers() {
    for (alias, target) in ALIASES {
        assert!(
            CATALOG.iter().any(|descriptor| descriptor.id == *target),
            "alias {alias} points at unknown provider {target}"
        );
        assert_eq!(provider_descriptor(alias).map(|d| &*d.id), Some(*target));
    }
    assert_eq!(canonical_id("  CoPilot "), "github-copilot");
    assert_eq!(canonical_id("openai_compat"), "openai");
    assert_eq!(canonical_id("unknown-thing"), "unknown-thing");
    assert!(provider_descriptor("unknown-thing").is_none());
}

#[test]
fn env_lookup_covers_the_common_providers() {
    assert_eq!(provider_env_vars("anthropic"), &["ANTHROPIC_API_KEY"]);
    assert_eq!(provider_env_vars("openai_compat"), &["OPENAI_API_KEY"]);
    assert_eq!(provider_env_vars("google"), &["GEMINI_API_KEY", "GOOGLE_API_KEY"]);
    assert!(provider_env_vars("lmstudio").is_empty());
    assert!(provider_env_vars("nope").is_empty());
}

#[test]
fn subscription_providers_carry_a_flow_and_a_label() {
    let subscription: Vec<&ProviderDescriptor> =
        CATALOG.iter().filter(|d| d.oauth.is_some()).collect();
    let ids: Vec<&str> = subscription.iter().map(|d| &*d.id).collect();
    assert_eq!(ids, vec!["openai-codex", "github-copilot", "openrouter"]);
    for descriptor in subscription {
        let methods = descriptor.methods();
        assert_eq!(
            methods.first().map(|method| method.kind),
            Some(AuthMethodKind::Subscription),
            "{}: the subscription option should lead",
            descriptor.id
        );
    }
}

/// Anthropic and Google are API-key-only *by decision*, and must say why rather than looking
/// like an oversight. If someone adds a subscription flow for either, this test is the
/// conversation.
#[test]
fn providers_without_a_sanctioned_subscription_explain_themselves() {
    for id in ["anthropic", "google"] {
        let descriptor = provider_descriptor(id).expect("in catalog");
        assert!(descriptor.oauth.is_none(), "{id} must not offer subscription auth");
        let note = descriptor.note.expect("must carry a note");
        assert!(note.contains("terms"), "{id} note should say why: {note}");
    }
}

/// The mistake this guards against was mine: I turned usage-in-streaming *off* for Copilot on a
/// hunch, which would have silently zeroed the token count on every Copilot turn. Copilot
/// accepts `stream_options` and returns usage — token accounting is not optional.
#[test]
fn copilot_still_reports_usage_and_asks_for_its_editor_headers() {
    let copilot = provider_descriptor("github-copilot").expect("in catalog").compat();
    assert!(
        copilot.supports_usage_in_streaming,
        "Copilot returns usage; disabling it loses every token count"
    );
    assert!(copilot.copilot_headers, "Copilot's endpoint expects its per-request headers");
    assert_eq!(
        copilot.cache_control_format.as_deref(),
        Some("anthropic"),
        "Claude over Copilot's /v1/messages caches only where a breakpoint asks it to"
    );
}

/// Subscription metering is about *how tokens are paid for*, not about having an OAuth flow:
/// OpenRouter's flow mints a key that spends real credits per token.
#[test]
fn subscription_billing_is_marked_only_where_a_plan_pays() {
    let billed: Vec<&str> =
        CATALOG.iter().filter(|d| d.subscription_billed).map(|d| &*d.id).collect();
    assert_eq!(billed, vec!["openai-codex", "github-copilot"]);
    assert!(
        !provider_descriptor("openrouter").expect("in catalog").subscription_billed,
        "OpenRouter bills credits per token, so its costs are real spend"
    );
}

#[test]
fn compat_defaults_match_what_the_adapters_expect() {
    let openai = provider_descriptor("openai").expect("in catalog").compat();
    assert!(openai.use_responses_api, "gpt-5-class models require /responses");
    assert_eq!(openai.max_tokens_field, "max_completion_tokens");
    let anthropic = provider_descriptor("anthropic").expect("in catalog").compat();
    assert_eq!(anthropic.cache_control_format.as_deref(), Some("anthropic"));
    assert!(provider_descriptor("lmstudio").expect("in catalog").compat().supports_developer_role);
}

/// The two spellings a gateway needs and nobody else does. Caching is the expensive one:
/// OpenRouter caches OpenAI and DeepSeek implicitly but *not* Claude, so an unasked session
/// re-reads its whole transcript at full price on every turn.
#[test]
fn openrouter_asks_for_caching_and_names_itself() {
    let openrouter = provider_descriptor("openrouter").expect("in catalog").compat();
    assert_eq!(openrouter.cache_control_format.as_deref(), Some("openrouter"));
    assert!(openrouter.openrouter_headers);
    assert_eq!(openrouter.thinking_format.as_deref(), Some("openrouter"));

    let named: Vec<&str> = CATALOG
        .iter()
        .filter(|descriptor| descriptor.compat().openrouter_headers)
        .map(|descriptor| &*descriptor.id)
        .collect();
    assert_eq!(named, vec!["openrouter"], "one gateway's headers, on one gateway");
}
