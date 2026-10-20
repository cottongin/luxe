//! provider_glyph — which mark the provider serving a model wears.
//!
//! The provider's id is written beside the mark, so the mark is shine, the same job
//! [`super::language_glyph`] does for a code fence. Only the providers with a mark in the Nerd Font
//! set get one; the rest share [`Icon::Provider`], because a made-up shape for a brand is worse
//! than a plain one.
//!
//! A gateway wears the gateway's mark, not the model maker's: `foundry-anthropic` is served by
//! Microsoft, and the dock row is about who serves the model. The model's own name says who made it.

use crate::glyphs::{brands, Icon};

/// The mark for `provider`, as the dock draws it now.
///
/// OpenAI, Anthropic and Copilot have marks only the newest Nerd Fonts carry, so where no font on
/// the machine has them they fall back to a mark every v3 font does.
pub(crate) fn provider_icon(provider: &str) -> Icon {
    let icon = provider_brand(&provider.to_ascii_lowercase());
    match brands() {
        true => icon,
        false => older_font_mark(icon),
    }
}

/// The brand mark for a lowercased provider id, whatever the font has.
fn provider_brand(provider: &str) -> Icon {
    match provider {
        "openai" => Icon::ProviderOpenAi,
        "anthropic" => Icon::ProviderAnthropic,
        "github-copilot" | "copilot" => Icon::ProviderCopilot,
        "google" | "gemini" | "google-gemini" | "google-vertex" | "vertex" => Icon::ProviderGoogle,
        "bedrock" | "amazon-bedrock" | "aws" => Icon::ProviderAws,
        id if id == "azure" || id.starts_with("azure-") || id.starts_with("foundry") => {
            Icon::ProviderAzure
        }
        _ => Icon::Provider,
    }
}

/// What a recent brand mark becomes in a font from before it: Copilot keeps GitHub's mark, the
/// rest the generic one.
fn older_font_mark(icon: Icon) -> Icon {
    match icon {
        Icon::ProviderCopilot => Icon::GitHub,
        Icon::ProviderOpenAi | Icon::ProviderAnthropic => Icon::Provider,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glyphs::RECENT_BRANDS;

    #[test]
    fn known_providers_wear_their_own_brand() {
        assert_eq!(provider_brand("openai"), Icon::ProviderOpenAi);
        assert_eq!(provider_brand("anthropic"), Icon::ProviderAnthropic);
        assert_eq!(provider_brand("github-copilot"), Icon::ProviderCopilot);
        assert_eq!(provider_brand("google"), Icon::ProviderGoogle);
        assert_eq!(provider_brand("bedrock"), Icon::ProviderAws);
    }

    /// A gateway is who serves the model, so `foundry-anthropic` is Azure's, not Anthropic's.
    #[test]
    fn a_gateway_wears_the_gateways_mark() {
        for id in ["foundry-anthropic", "foundry-openai", "foundry", "azure", "azure-foundry"] {
            assert_eq!(provider_brand(id), Icon::ProviderAzure, "{id}");
        }
        assert_eq!(provider_brand("azurely"), Icon::Provider, "a prefix, not a substring");
    }

    #[test]
    fn a_provider_with_no_mark_gets_the_generic_one() {
        for id in ["deepseek", "zai", "openrouter", "ollama", "mistral", ""] {
            assert_eq!(provider_brand(id), Icon::Provider, "{id}");
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
    }
}
