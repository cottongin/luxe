//! provider_glyph — which brand mark the provider serving a model, and the model, wear.
//!
//! The id is written beside the mark, so the mark is shine, the same job
//! [`super::language_glyph`] does for a code fence. Only the brands with a mark in the Nerd Font
//! set get one; the rest share [`Icon::Provider`] or [`Icon::Model`], because a made-up shape for a
//! brand is worse than a plain one. That rules out Anthropic, DeepSeek and Mistral, which have none.
//!
//! The two rows answer different questions. The provider row is who serves the model, so a gateway
//! wears the gateway's mark: `foundry-anthropic` is Microsoft's. The model row is who made it, so
//! `claude-sonnet-5` wears Claude's mark whether Anthropic, Copilot or Foundry serves it.

use crate::glyphs::{brands, Icon, RECENT_BRANDS};

/// The mark for `provider`, as the dock draws it now.
///
/// OpenAI and Copilot have marks only the newest Nerd Fonts carry, so where no font on the machine
/// has them they fall back to a mark every v3 font does.
pub(crate) fn provider_icon(provider: &str) -> Icon {
    let icon = provider_brand(&provider.to_ascii_lowercase());
    match brands() {
        true => icon,
        false => older_font_mark(icon),
    }
}

/// The mark for the model `model`, an id like `claude-opus-5-5` or `openai/gpt-5`, as the dock
/// draws it now. A recent brand falls back to the plain model mark where no font has it.
pub(crate) fn model_icon(model: &str) -> Icon {
    let icon = model_brand(&model.to_ascii_lowercase());
    match brands() || !RECENT_BRANDS.contains(&icon) {
        true => icon,
        false => Icon::Model,
    }
}

/// The brand mark for a lowercased provider id, whatever the font has.
fn provider_brand(provider: &str) -> Icon {
    match provider {
        "openai" | "openai-codex" | "codex" | "chatgpt" => Icon::ProviderOpenAi,
        "github-copilot" | "copilot" => Icon::ProviderCopilot,
        "google" | "gemini" | "google-gemini" | "google-vertex" | "vertex" => Icon::ProviderGoogle,
        "bedrock" | "amazon-bedrock" | "aws" => Icon::ProviderAws,
        id if id == "azure" || id.starts_with("azure-") || id.starts_with("foundry") => {
            Icon::ProviderAzure
        }
        _ => Icon::Provider,
    }
}

/// The maker's mark for a lowercased model id, whatever the font has.
///
/// Judged on the last path segment, since a router names a model `anthropic/claude-sonnet-5`.
/// Claude is found anywhere in it, because Bedrock writes `us.anthropic.claude-sonnet-5-v1:0`.
fn model_brand(model: &str) -> Icon {
    let name = model.rsplit('/').next().unwrap_or(model);
    let family = |prefix: &str| name == prefix || name.starts_with(&format!("{prefix}-"));
    if name.contains("claude") {
        return Icon::ModelClaude;
    }
    if ["gpt", "chatgpt", "codex", "o1", "o3", "o4"].into_iter().any(family) {
        return Icon::ProviderOpenAi;
    }
    if family("gemini") || family("gemma") {
        return Icon::ProviderGoogle;
    }
    Icon::Model
}

/// What a recent brand mark becomes in a font from before it: Copilot keeps GitHub's mark, the
/// rest the generic one.
fn older_font_mark(icon: Icon) -> Icon {
    match icon {
        Icon::ProviderCopilot => Icon::GitHub,
        Icon::ProviderOpenAi => Icon::Provider,
        Icon::ModelClaude => Icon::Model,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_providers_wear_their_own_brand() {
        assert_eq!(provider_brand("openai"), Icon::ProviderOpenAi);
        assert_eq!(provider_brand("github-copilot"), Icon::ProviderCopilot);
        assert_eq!(provider_brand("google"), Icon::ProviderGoogle);
        assert_eq!(provider_brand("bedrock"), Icon::ProviderAws);
    }

    /// A ChatGPT subscription is OpenAI serving its own models, so it wears OpenAI's mark too.
    #[test]
    fn the_codex_subscription_wears_openais_mark() {
        for id in ["openai-codex", "codex", "chatgpt"] {
            assert_eq!(provider_brand(id), Icon::ProviderOpenAi, "{id}");
        }
    }

    /// A gateway is who serves the model, so `foundry-anthropic` is Azure's, not Anthropic's.
    #[test]
    fn a_gateway_wears_the_gateways_mark() {
        for id in ["foundry-anthropic", "foundry-openai", "foundry", "azure", "azure-foundry"] {
            assert_eq!(provider_brand(id), Icon::ProviderAzure, "{id}");
        }
        assert_eq!(provider_brand("azurely"), Icon::Provider, "a prefix, not a substring");
    }

    /// Anthropic has no mark of its own, and Claude's is its models', not the company's.
    #[test]
    fn a_provider_with_no_mark_gets_the_generic_one() {
        for id in ["anthropic", "deepseek", "zai", "openrouter", "ollama", "mistral", ""] {
            assert_eq!(provider_brand(id), Icon::Provider, "{id}");
        }
    }

    /// A model wears its maker's mark, whoever serves it and however the id is spelled.
    #[test]
    fn a_model_wears_its_makers_mark() {
        for id in ["claude-opus-5-5", "anthropic/claude-sonnet-5", "us.anthropic.claude-haiku-5"] {
            assert_eq!(model_brand(id), Icon::ModelClaude, "{id}");
        }
        for id in ["gpt-5", "openai/gpt-5-codex", "o3", "o4-mini", "codex-mini", "chatgpt-4o"] {
            assert_eq!(model_brand(id), Icon::ProviderOpenAi, "{id}");
        }
        for id in ["gemini-3-pro", "google/gemma-4"] {
            assert_eq!(model_brand(id), Icon::ProviderGoogle, "{id}");
        }
    }

    /// A family is a whole leading word: `o3` is OpenAI's, `o3d-model` and `opus` are not.
    #[test]
    fn a_model_with_no_known_maker_gets_the_plain_mark() {
        for id in ["deepseek-v4", "qwen3.8-27b", "o3d-model", "gptish", "kimi-k2.5", ""] {
            assert_eq!(model_brand(id), Icon::Model, "{id}");
        }
    }

    /// Every recent brand has a fallback that is not itself a recent brand, so an older font never
    /// draws a box for one.
    #[test]
    fn an_older_font_never_meets_a_recent_brand() {
        for icon in RECENT_BRANDS {
            assert!(!RECENT_BRANDS.contains(&older_font_mark(*icon)), "{icon:?}");
        }
        assert_eq!(older_font_mark(Icon::ProviderCopilot), Icon::GitHub);
        assert_eq!(older_font_mark(Icon::ProviderGoogle), Icon::ProviderGoogle);
        // Brands are off unless the startup probe finds them, so this is the older font
        assert_eq!(model_icon("claude-opus-5"), Icon::Model, "the model row's own plain mark");
        assert_eq!(model_icon("gemini-3"), Icon::ProviderGoogle, "every v3 font has Google's");
    }
}
