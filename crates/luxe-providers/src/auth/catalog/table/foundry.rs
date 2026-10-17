//! Microsoft Foundry — the model-id prefixes and endpoints that split one resource into providers.

use crate::auth::catalog::ResourceEndpoint;

/// Model-id prefixes that Microsoft Foundry reaches through its Anthropic passthrough.
///
/// The split between Luxe's two Foundry providers, stated once. Every Anthropic-published model in
/// the models.dev `azure` catalog is named `claude-*`, and a test holds that true.
pub(in crate::auth::catalog) const FOUNDRY_ANTHROPIC_PREFIXES: &[&str] = &["claude-"];

/// Model-id prefixes that Microsoft Foundry serves on `/openai/v1/chat/completions`.
///
/// A Foundry resource can host models nobody at OpenAI or Anthropic published — Cohere, DeepSeek,
/// Grok, Kimi, Llama, Mistral, Phi — and those speak the older chat/completions dialect rather than
/// the Responses API sold beside them at the same path.
pub(in crate::auth::catalog) const FOUNDRY_COMPLETIONS_PREFIXES: &[&str] = &[
    "codestral-",
    "cohere-",
    "deepseek-",
    "grok-",
    "kimi-",
    "llama-",
    "ministral-",
    "mistral-",
    "phi-",
];

/// Everything a Foundry resource does **not** reach through `/openai/v1/responses`.
///
/// The union of the other two surfaces' prefixes, spelled out because stable Rust cannot
/// concatenate const slices — a test holds it equal to that union, so the three rules cannot drift
/// into a model served twice or not at all.
pub(in crate::auth::catalog) const FOUNDRY_NON_RESPONSES_PREFIXES: &[&str] = &[
    "claude-",
    "codestral-",
    "cohere-",
    "deepseek-",
    "grok-",
    "kimi-",
    "llama-",
    "ministral-",
    "mistral-",
    "phi-",
];

/// A Foundry resource's OpenAI-shaped surface.
pub(in crate::auth::catalog) const FOUNDRY_OPENAI_ENDPOINT: ResourceEndpoint =
    ResourceEndpoint { host: "https://{resource}.services.ai.azure.com", path: "/openai/v1" };

/// A Foundry resource's Anthropic passthrough.
pub(in crate::auth::catalog) const FOUNDRY_ANTHROPIC_ENDPOINT: ResourceEndpoint =
    ResourceEndpoint { host: "https://{resource}.services.ai.azure.com", path: "/anthropic/v1" };
