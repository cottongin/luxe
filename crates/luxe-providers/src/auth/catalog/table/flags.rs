//! Compat defaults — the wire quirks each catalog entry starts from.
//!
//! One constructor per distinct set, so an entry names its flavour rather than spelling the flags
//! out. A hand-written `models.json` provider block overrides whatever these say.

use crate::config::CompatFlags;

pub(in crate::auth::catalog) fn plain() -> CompatFlags {
    CompatFlags::default()
}

pub(in crate::auth::catalog) fn openai_flags() -> CompatFlags {
    CompatFlags {
        supports_developer_role: true,
        supports_reasoning_effort: true,
        use_responses_api: true,
        max_tokens_field: "max_completion_tokens".to_string(),
        ..CompatFlags::default()
    }
}

pub(in crate::auth::catalog) fn codex_flags() -> CompatFlags {
    CompatFlags { supports_reasoning_effort: true, ..CompatFlags::default() }
}

pub(in crate::auth::catalog) fn anthropic_flags() -> CompatFlags {
    CompatFlags { cache_control_format: Some("anthropic".to_string()), ..CompatFlags::default() }
}

pub(in crate::auth::catalog) fn thinking(format: &str) -> CompatFlags {
    CompatFlags { thinking_format: Some(format.to_string()), ..CompatFlags::default() }
}

pub(in crate::auth::catalog) fn openrouter_flags() -> CompatFlags {
    CompatFlags {
        supports_reasoning_effort: true,
        // OpenRouter does not cache Claude implicitly, so an agentic session re-reads its whole
        // transcript at full price on every turn unless asked otherwise. One field asks.
        cache_control_format: Some("openrouter".to_string()),
        openrouter_headers: true,
        ..thinking("openrouter")
    }
}

pub(in crate::auth::catalog) fn deepseek_flags() -> CompatFlags {
    CompatFlags { supports_reasoning_effort: true, ..thinking("deepseek") }
}

pub(in crate::auth::catalog) fn together_flags() -> CompatFlags {
    CompatFlags { supports_reasoning_effort: true, ..thinking("together") }
}

pub(in crate::auth::catalog) fn qwen_flags() -> CompatFlags {
    thinking("qwen")
}

pub(in crate::auth::catalog) fn local_flags() -> CompatFlags {
    CompatFlags {
        supports_developer_role: true,
        thinking_format: Some("chat_template".to_string()),
        ..CompatFlags::default()
    }
}

pub(in crate::auth::catalog) fn copilot_flags() -> CompatFlags {
    // Copilot's endpoint accepts `stream_options: {include_usage: true}` and returns usage, so the
    // default (on) stands: turning it off was a guess that would have silently zeroed the token
    // count on every Copilot turn. It does need per-request editor headers, which the adapter adds.
    // The cache format is for Claude, which Copilot serves on `/v1/messages` and which caches only
    // where it is asked to; the chat and Responses adapters have no use for it. Effort reaches
    // the chat-completions models (Gemini, Kimi) as `reasoning_effort`, held to the values each
    // model lists by `copilot::adopt_efforts`.
    CompatFlags {
        copilot_headers: true,
        supports_reasoning_effort: true,
        cache_control_format: Some("anthropic".to_string()),
        ..CompatFlags::default()
    }
}
