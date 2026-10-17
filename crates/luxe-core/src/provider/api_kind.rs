//! API kind — which wire API a provider speaks.

/// Which wire API a provider speaks — the adapter a provider id routes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApiKind {
    /// OpenAI `POST /chat/completions` (and every service that clones it).
    OpenaiCompletions,
    /// OpenAI `POST /responses`.
    OpenaiResponses,
    /// Anthropic `POST /messages`.
    AnthropicMessages,
    /// The ChatGPT-subscription Codex backend (`/codex/responses`).
    OpenaiCodex,
}

impl ApiKind {
    /// Stable identifier used in config files and diagnostics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiCompletions => "openai-completions",
            Self::OpenaiResponses => "openai-responses",
            Self::AnthropicMessages => "anthropic-messages",
            Self::OpenaiCodex => "openai-codex",
        }
    }

    /// Parse an [`ApiKind`] from its stable identifier.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "openai-completions" | "openai-compat" | "openai" => Some(Self::OpenaiCompletions),
            "openai-responses" | "responses" => Some(Self::OpenaiResponses),
            "anthropic-messages" | "anthropic" => Some(Self::AnthropicMessages),
            "openai-codex" | "codex" => Some(Self::OpenaiCodex),
            _ => None,
        }
    }
}

impl std::fmt::Display for ApiKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
