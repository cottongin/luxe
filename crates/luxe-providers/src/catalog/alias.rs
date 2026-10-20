//! Id mapping between luxe's names and models.dev's.
//!
//! luxe provider names are user-chosen (`lmstudio`, `dashscope`, `openai_compat`) and model ids
//! are whatever the endpoint expects, so neither is guaranteed to be a models.dev key. Both
//! mappings here affect **metadata lookup only** — the wire id a provider receives is never
//! rewritten.

use crate::config::ProviderConfig;

/// luxe provider names that don't match their models.dev id.
///
/// Deliberately tiny: almost every name maps verbatim, the provider catalog carries the rest, and a
/// user whose provider needs a different id sets `"models_dev"` on it.
const ALIASES: &[(&str, &str)] = &[
    // luxe's default provider name; an OpenAI-compatible endpoint serving `gpt-*` really is
    // serving OpenAI's models, and a local server's ids simply won't match anything.
    ("openai_compat", "openai"),
    ("dashscope", "alibaba"),
];

/// The models.dev provider id to look a luxe provider up under.
///
/// Config wins, then the provider catalog, then the alias table, then the name verbatim. Never
/// `None`: a name that matches nothing just finds no models, which is the right answer for a local
/// server.
///
/// The catalog step is what makes a subscription provider work at all: `openai-codex` is a luxe
/// provider, not a models.dev one, and without the mapping its models resolve to no metadata (a
/// zero context window, no pricing) and never appear in the picker.
pub fn models_dev_provider<'a>(provider: &'a str, config: Option<&'a ProviderConfig>) -> &'a str {
    if let Some(id) = config.and_then(|c| c.models_dev.as_deref()).filter(|id| !id.is_empty()) {
        return id;
    }
    if let Some(id) = crate::auth::provider_descriptor(provider).and_then(|d| d.models_dev) {
        return id;
    }
    ALIASES.iter().find_map(|(name, id)| (*name == provider).then_some(*id)).unwrap_or(provider)
}

/// The base model id behind a pinned variant, or `None` when the id is already a base id.
///
/// models.dev carries both `claude-opus-4-5` and `claude-opus-4-5-20251101`, but it can't carry
/// every dated snapshot the moment it ships. A pinned or `-latest` alias inherits its family's
/// metadata, which beats falling back to generic defaults.
pub fn normalize_model_id(id: &str) -> Option<&str> {
    let base = id.strip_suffix("-latest").or_else(|| {
        let (base, suffix) = id.rsplit_once('-')?;
        (suffix.len() == 8 && suffix.bytes().all(|b| b.is_ascii_digit())).then_some(base)
    })?;
    (!base.is_empty()).then_some(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_id_prefers_config_then_alias_then_verbatim() {
        let configured =
            ProviderConfig { models_dev: Some("alibaba".into()), ..Default::default() };
        assert_eq!(models_dev_provider("my-proxy", Some(&configured)), "alibaba");
        assert_eq!(models_dev_provider("openai_compat", None), "openai");
        assert_eq!(models_dev_provider("dashscope", None), "alibaba");
        assert_eq!(models_dev_provider("anthropic", None), "anthropic");
        assert_eq!(models_dev_provider("lmstudio", None), "lmstudio");
        // A subscription provider borrows its metadata from the API-key provider serving the same
        // models; without this, a Codex login resolves to no models and a zero context window.
        assert_eq!(models_dev_provider("openai-codex", None), "openai");
        assert_eq!(models_dev_provider("github-copilot", None), "github-copilot");
        assert_eq!(models_dev_provider("fireworks", None), "fireworks-ai");
        // An empty override is treated as unset rather than as "look up the empty provider".
        let blank = ProviderConfig { models_dev: Some(String::new()), ..Default::default() };
        assert_eq!(models_dev_provider("anthropic", Some(&blank)), "anthropic");
    }

    #[test]
    fn model_id_normalization_strips_dates_and_latest() {
        assert_eq!(normalize_model_id("claude-opus-4-5-20251101"), Some("claude-opus-4-5"));
        assert_eq!(normalize_model_id("claude-3-5-sonnet-latest"), Some("claude-3-5-sonnet"));
        assert_eq!(normalize_model_id("gpt-4o"), None);
        assert_eq!(normalize_model_id("claude-opus-5"), None);
        // Not a date: wrong length, or non-digits.
        assert_eq!(normalize_model_id("model-2026"), None);
        assert_eq!(normalize_model_id("model-2026072a"), None);
        assert_eq!(normalize_model_id("-20251101"), None);
    }
}
