//! The table — every curated provider, one entry each.
//!
//! Entries spell out only what differs from [`BASE`]; the rest of the catalog's machinery lives in
//! the parent module.

use luxe_core::auth::ApiKind;

use std::borrow::Cow::Borrowed;

use flags::*;
use foundry::*;

use super::{OauthFlow, ProviderDescriptor, ServedModels};
use crate::auth::source::KeyStyle;

pub(super) mod flags;
pub(super) mod foundry;

/// Shared defaults every entry starts from; an entry spells out only what differs.
///
/// `id`, `name` and `api` are placeholders, as every entry sets them.
pub(super) const BASE: ProviderDescriptor = ProviderDescriptor {
    id: Borrowed(""),
    name: Borrowed(""),
    api: ApiKind::OpenaiCompletions,
    base_url: None,
    endpoint: None,
    env: &[],
    key_style: None,
    models_dev: None,
    api_key_login: true,
    oauth: None,
    console_url: None,
    key_hint: None,
    note: None,
    after_login: None,
    keyless: false,
    needs_base_url: false,
    lists_models: true,
    lists_deployments: false,
    paired_with: &[],
    compat_defaults: plain,
    served_models: ServedModels::Catalog,
    subscription_billed: false,
};

/// The curated catalog.
///
/// Adding an entry: get the base URL's version segment right, name the environment variable the
/// provider's own docs use, and point `models_dev` at a real catalog id (a test enforces that).
pub(super) static CATALOG: &[ProviderDescriptor] = &[
    ProviderDescriptor {
        id: Borrowed("openai"),
        name: Borrowed("OpenAI"),
        api: ApiKind::OpenaiResponses,
        base_url: Some("https://api.openai.com/v1"),
        env: &["OPENAI_API_KEY"],
        models_dev: Some("openai"),
        console_url: Some("https://platform.openai.com/api-keys"),
        key_hint: Some("sk-…"),
        compat_defaults: openai_flags,
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("openai-codex"),
        name: Borrowed("OpenAI Codex (ChatGPT Plus/Pro)"),
        api: ApiKind::OpenaiCodex,
        base_url: Some("https://chatgpt.com/backend-api"),
        models_dev: Some("openai"),
        api_key_login: false,
        oauth: Some((OauthFlow::OpenaiCodex, "Sign in with ChatGPT")),
        note: Some(
            "Uses your ChatGPT plan's Codex quota. For pay-as-you-go API billing, pick OpenAI instead.",
        ),
        served_models: ServedModels::Only(&[
            "gpt-5.6-sol",
            "gpt-5.6-luna",
            "gpt-5.6-terra",
            "gpt-5.5",
            "gpt-5.4",
            "gpt-5.4-mini",
            "gpt-5.3-codex-spark",
        ]),
        subscription_billed: true,
        compat_defaults: codex_flags,
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("anthropic"),
        name: Borrowed("Anthropic"),
        api: ApiKind::AnthropicMessages,
        base_url: Some("https://api.anthropic.com/v1"),
        env: &["ANTHROPIC_API_KEY"],
        models_dev: Some("anthropic"),
        console_url: Some("https://console.anthropic.com/settings/keys"),
        key_hint: Some("sk-ant-…"),
        note: Some(
            "API key only. Anthropic's terms forbid using a Claude Pro/Max subscription from a \
             third-party harness, so luxe does not offer that sign-in.",
        ),
        compat_defaults: anthropic_flags,
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("github-copilot"),
        name: Borrowed("GitHub Copilot"),
        api: ApiKind::OpenaiCompletions,
        base_url: Some("https://api.individual.githubcopilot.com"),
        env: &["COPILOT_GITHUB_TOKEN"],
        models_dev: Some("github-copilot"),
        api_key_login: false,
        oauth: Some((OauthFlow::GithubCopilot, "Sign in with GitHub")),
        after_login: Some(
            "If a model reports \"not supported\", enable it once in VS Code: Copilot Chat → model \
             picker → Enable.",
        ),
        subscription_billed: true,
        compat_defaults: copilot_flags,
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("openrouter"),
        name: Borrowed("OpenRouter"),
        api: ApiKind::OpenaiCompletions,
        base_url: Some("https://openrouter.ai/api/v1"),
        env: &["OPENROUTER_API_KEY"],
        models_dev: Some("openrouter"),
        oauth: Some((OauthFlow::OpenRouter, "Sign in with OpenRouter")),
        console_url: Some("https://openrouter.ai/keys"),
        key_hint: Some("sk-or-…"),
        compat_defaults: openrouter_flags,
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("google"),
        name: Borrowed("Google Gemini"),
        api: ApiKind::OpenaiCompletions,
        base_url: Some("https://generativelanguage.googleapis.com/v1beta/openai"),
        env: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        models_dev: Some("google"),
        console_url: Some("https://aistudio.google.com/apikey"),
        key_hint: Some("AIza…"),
        note: Some(
            "API key only. Google's terms forbid using Gemini CLI / Google AI subscription sign-in \
             from a third-party harness, so luxe does not offer that sign-in.",
        ),
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("xai"),
        name: Borrowed("xAI (Grok)"),
        api: ApiKind::OpenaiCompletions,
        base_url: Some("https://api.x.ai/v1"),
        env: &["XAI_API_KEY"],
        models_dev: Some("xai"),
        console_url: Some("https://console.x.ai"),
        key_hint: Some("xai-…"),
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("deepseek"),
        name: Borrowed("DeepSeek"),
        api: ApiKind::OpenaiCompletions,
        base_url: Some("https://api.deepseek.com/v1"),
        env: &["DEEPSEEK_API_KEY"],
        models_dev: Some("deepseek"),
        console_url: Some("https://platform.deepseek.com/api_keys"),
        key_hint: Some("sk-…"),
        compat_defaults: deepseek_flags,
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("groq"),
        name: Borrowed("Groq"),
        api: ApiKind::OpenaiCompletions,
        base_url: Some("https://api.groq.com/openai/v1"),
        env: &["GROQ_API_KEY"],
        models_dev: Some("groq"),
        console_url: Some("https://console.groq.com/keys"),
        key_hint: Some("gsk_…"),
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("cerebras"),
        name: Borrowed("Cerebras"),
        api: ApiKind::OpenaiCompletions,
        base_url: Some("https://api.cerebras.ai/v1"),
        env: &["CEREBRAS_API_KEY"],
        models_dev: Some("cerebras"),
        console_url: Some("https://cloud.cerebras.ai"),
        key_hint: Some("csk-…"),
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("mistral"),
        name: Borrowed("Mistral"),
        api: ApiKind::OpenaiCompletions,
        base_url: Some("https://api.mistral.ai/v1"),
        env: &["MISTRAL_API_KEY"],
        models_dev: Some("mistral"),
        console_url: Some("https://console.mistral.ai/api-keys"),
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("dashscope"),
        name: Borrowed("Alibaba DashScope (Qwen)"),
        api: ApiKind::OpenaiCompletions,
        base_url: Some("https://dashscope-intl.aliyuncs.com/compatible-mode/v1"),
        env: &["DASHSCOPE_API_KEY"],
        models_dev: Some("alibaba"),
        console_url: Some("https://bailian.console.alibabacloud.com"),
        key_hint: Some("sk-…"),
        compat_defaults: qwen_flags,
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("zai"),
        name: Borrowed("Z.AI (GLM)"),
        api: ApiKind::OpenaiCompletions,
        base_url: Some("https://api.z.ai/api/paas/v4"),
        env: &["ZAI_API_KEY"],
        models_dev: Some("zai"),
        console_url: Some("https://z.ai/manage-apikey/apikey-list"),
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("moonshotai"),
        name: Borrowed("Moonshot AI (Kimi)"),
        api: ApiKind::OpenaiCompletions,
        base_url: Some("https://api.moonshot.ai/v1"),
        env: &["MOONSHOT_API_KEY"],
        models_dev: Some("moonshotai"),
        console_url: Some("https://platform.moonshot.ai/console/api-keys"),
        key_hint: Some("sk-…"),
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("togetherai"),
        name: Borrowed("Together AI"),
        api: ApiKind::OpenaiCompletions,
        base_url: Some("https://api.together.xyz/v1"),
        env: &["TOGETHER_API_KEY"],
        models_dev: Some("togetherai"),
        console_url: Some("https://api.together.ai/settings/api-keys"),
        compat_defaults: together_flags,
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("fireworks"),
        name: Borrowed("Fireworks AI"),
        api: ApiKind::OpenaiCompletions,
        base_url: Some("https://api.fireworks.ai/inference/v1"),
        env: &["FIREWORKS_API_KEY"],
        models_dev: Some("fireworks-ai"),
        console_url: Some("https://fireworks.ai/account/api-keys"),
        key_hint: Some("fw_…"),
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("nvidia"),
        name: Borrowed("NVIDIA NIM"),
        api: ApiKind::OpenaiCompletions,
        base_url: Some("https://integrate.api.nvidia.com/v1"),
        env: &["NVIDIA_API_KEY"],
        models_dev: Some("nvidia"),
        console_url: Some("https://build.nvidia.com"),
        key_hint: Some("nvapi-…"),
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("vercel"),
        name: Borrowed("Vercel AI Gateway"),
        api: ApiKind::OpenaiCompletions,
        base_url: Some("https://ai-gateway.vercel.sh/v1"),
        env: &["AI_GATEWAY_API_KEY"],
        models_dev: Some("vercel"),
        console_url: Some("https://vercel.com/dashboard/ai-gateway/api-keys"),
        key_hint: Some("vck_…"),
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("foundry-openai"),
        name: Borrowed("Microsoft Foundry (OpenAI models)"),
        api: ApiKind::OpenaiResponses,
        endpoint: Some(FOUNDRY_OPENAI_ENDPOINT),
        env: &["FOUNDRY_API_KEY"],
        models_dev: Some("azure"),
        console_url: Some("https://ai.azure.com"),
        note: Some(
            "Needs your Foundry resource — its name, or any endpoint off a deployment's Details \
             tab. The same key configures your Claude deployments too.",
        ),
        needs_base_url: true,
        lists_deployments: true,
        paired_with: &["foundry-anthropic", "foundry-completions"],
        served_models: ServedModels::Unprefixed(FOUNDRY_NON_RESPONSES_PREFIXES),
        compat_defaults: openai_flags,
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("foundry-anthropic"),
        name: Borrowed("Microsoft Foundry (Claude)"),
        api: ApiKind::AnthropicMessages,
        endpoint: Some(FOUNDRY_ANTHROPIC_ENDPOINT),
        env: &["FOUNDRY_API_KEY", "ANTHROPIC_FOUNDRY_API_KEY"],
        key_style: Some(KeyStyle::FoundryKey),
        models_dev: Some("azure"),
        console_url: Some("https://ai.azure.com"),
        note: Some(
            "Needs your Foundry resource — its name, or any endpoint off a deployment's Details \
             tab. The same key configures your GPT deployments too.",
        ),
        after_login: Some(
            "Claude on Foundry is Azure's product: its context limits and rates come from the \
             Azure catalog, which does not always match Anthropic's own.",
        ),
        needs_base_url: true,
        lists_models: false,
        lists_deployments: true,
        paired_with: &["foundry-openai", "foundry-completions"],
        served_models: ServedModels::Prefixed(FOUNDRY_ANTHROPIC_PREFIXES),
        compat_defaults: anthropic_flags,
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("foundry-completions"),
        name: Borrowed("Microsoft Foundry (partner models)"),
        api: ApiKind::OpenaiCompletions,
        endpoint: Some(FOUNDRY_OPENAI_ENDPOINT),
        env: &["FOUNDRY_API_KEY"],
        models_dev: Some("azure"),
        console_url: Some("https://ai.azure.com"),
        note: Some(
            "For the models on a Foundry resource that OpenAI and Anthropic did not publish — \
             DeepSeek, Grok, Llama, Mistral, Phi, Cohere. They answer on chat/completions.",
        ),
        needs_base_url: true,
        lists_deployments: true,
        paired_with: &["foundry-openai", "foundry-anthropic"],
        served_models: ServedModels::Prefixed(FOUNDRY_COMPLETIONS_PREFIXES),
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("azure"),
        name: Borrowed("Azure OpenAI (classic)"),
        api: ApiKind::OpenaiResponses,
        env: &["AZURE_OPENAI_API_KEY"],
        models_dev: Some("azure"),
        console_url: Some("https://portal.azure.com"),
        note: Some(
            "For an `openai.azure.com` resource. A `services.ai.azure.com` one is Microsoft \
             Foundry — pick that instead and luxe derives both endpoints for you.",
        ),
        needs_base_url: true,
        lists_deployments: true,
        compat_defaults: openai_flags,
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("lmstudio"),
        name: Borrowed("LM Studio (local)"),
        api: ApiKind::OpenaiCompletions,
        base_url: Some("http://127.0.0.1:1234/v1"),
        models_dev: Some("lmstudio"),
        api_key_login: false,
        console_url: Some("https://lmstudio.ai"),
        keyless: true,
        compat_defaults: local_flags,
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("ollama"),
        name: Borrowed("Ollama (local)"),
        api: ApiKind::OpenaiCompletions,
        base_url: Some("http://127.0.0.1:11434/v1"),
        api_key_login: false,
        console_url: Some("https://ollama.com"),
        keyless: true,
        compat_defaults: local_flags,
        ..BASE
    },
    ProviderDescriptor {
        id: Borrowed("custom"),
        name: Borrowed("Custom OpenAI-compatible endpoint"),
        api: ApiKind::OpenaiCompletions,
        note: Some("vLLM, llama.cpp, a proxy, or anything else that speaks /chat/completions"),
        needs_base_url: true,
        ..BASE
    },
];
