//! Provider catalog — the curated set of providers a user can configure, and how each is reached.
//!
//! Curated rather than derived: models.dev knows every provider's models but not the wire quirks
//! Luxe needs (which API a provider speaks, whether it takes `developer` roles, how it spells a
//! thinking toggle), and a *wrong* default is worse than an absent one. Anything not listed here
//! still works — `custom` covers any OpenAI-compatible endpoint, and a hand-written `models.json`
//! provider block always wins over these defaults.
//!
//! The catalog is the single source of truth for: which adapter a provider id routes to, its default
//! base URL, its environment variables, its compat flags, and which login methods it offers.

use std::borrow::Cow;

use luxe_core::auth::{ApiKind, AuthMethod, AuthMethodKind};

use self::table::{flags::anthropic_flags, BASE, CATALOG};
use super::source::KeyStyle;
use crate::config::CompatFlags;

mod table;

/// Which OAuth flow a subscription login runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OauthFlow {
    /// ChatGPT Plus/Pro via the Codex client (PKCE loopback, or OpenAI's device-code flow).
    OpenaiCodex,
    /// GitHub Copilot (RFC 8628 device flow, then a Copilot token exchange).
    GithubCopilot,
    /// OpenRouter's published PKCE flow, which mints a user-owned API key.
    OpenRouter,
}

/// Which of its metadata catalog's models a provider actually serves.
///
/// A catalog id is not always one-to-one with an endpoint. Two of them are not: a subscription
/// backend serves a different product than the API behind the same brand, and one Microsoft Foundry
/// resource is two providers over a single models.dev catalog, split by who published the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServedModels {
    /// Everything the metadata catalog lists.
    #[default]
    Catalog,
    /// Exactly these ids.
    ///
    /// The ChatGPT-account Codex endpoint refuses `gpt-5.3-codex` ("not supported when using Codex
    /// with a ChatGPT account") while serving models the API catalog lists separately.
    Only(&'static [&'static str]),
    /// The catalog's models whose id starts with one of these.
    Prefixed(&'static [&'static str]),
    /// The catalog's models whose id starts with none of these — the other half of a
    /// [`ServedModels::Prefixed`] split.
    ///
    /// A prefix rather than a list because the rule is the *publisher*: Foundry reaches Anthropic's
    /// models at `/anthropic/v1` and everyone else's at `/openai/v1`, so a list would go stale the
    /// next time Anthropic ships and put a brand-new model on the wrong path.
    Unprefixed(&'static [&'static str]),
}

impl ServedModels {
    /// Whether this provider serves `id`, ignoring case.
    ///
    /// Case-blind because a model id is not always a catalog id: Azure reports a deployment of
    /// DeepSeek as `DeepSeek-V4-Flash`, and a case-sensitive `deepseek-` would have sent it down the
    /// OpenAI path.
    pub fn serves(self, id: &str) -> bool {
        let id = id.to_ascii_lowercase();
        match self {
            Self::Catalog => true,
            Self::Only(ids) => ids.iter().any(|known| known.eq_ignore_ascii_case(&id)),
            Self::Prefixed(prefixes) => prefixes.iter().any(|prefix| id.starts_with(prefix)),
            Self::Unprefixed(prefixes) => !prefixes.iter().any(|prefix| id.starts_with(prefix)),
        }
    }
}

/// Where a provider lives under a resource the user names.
///
/// For a host that serves several APIs off one resource. What a user has to hand is the resource
/// name, or a target URI copied off a deployment's Details tab — never the base URLs, whose paths
/// they would have to know and which differ per API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceEndpoint {
    /// Host form for a bare resource name, with `{resource}` standing in for it.
    pub host: &'static str,
    /// Path this provider serves under, appended to the resource root.
    pub path: &'static str,
}

/// One curated provider.
#[derive(Debug, Clone)]
pub struct ProviderDescriptor {
    /// Provider id — the `auth.json` key and the `provider` half of a `provider/model` ref.
    pub id: Cow<'static, str>,
    /// Display name.
    pub name: Cow<'static, str>,
    /// Which wire API this provider speaks.
    pub api: ApiKind,
    /// Default endpoint, including the version segment. `None` when the user must supply one.
    pub base_url: Option<&'static str>,
    /// How this provider's base URL is derived from a resource, when it isn't typed whole.
    pub endpoint: Option<ResourceEndpoint>,
    /// Environment variables consulted for an ambient key, most specific first.
    pub env: &'static [&'static str],
    /// How this provider wants its API key presented. `None` derives it from [`Self::api`].
    ///
    /// Only a host that re-serves another vendor's API needs to say: Foundry's passthrough speaks
    /// Anthropic Messages and authenticates the Azure way.
    pub key_style: Option<KeyStyle>,
    /// models.dev provider id for model metadata, when it has one.
    pub models_dev: Option<&'static str>,
    /// Whether an API key can be stored for this provider.
    pub api_key_login: bool,
    /// The subscription flow this provider offers, and how to label it.
    pub oauth: Option<(OauthFlow, &'static str)>,
    /// Where to get an API key.
    pub console_url: Option<&'static str>,
    /// Shape hint shown next to the key prompt (`sk-ant-…`).
    pub key_hint: Option<&'static str>,
    /// A caveat the user should read *before* choosing this provider (a terms restriction, a
    /// billing surprise). Shown when the flow starts.
    pub note: Option<&'static str>,
    /// Advice that only makes sense once the credential exists — shown with the outcome.
    pub after_login: Option<&'static str>,
    /// Whether this provider needs no credential at all (a local server).
    pub keyless: bool,
    /// Whether logging in must ask for a base URL (a custom endpoint, Azure).
    pub needs_base_url: bool,
    /// Whether `{base_url}/models` answers.
    ///
    /// Foundry's Anthropic passthrough has no such endpoint — Anthropic's own Foundry client
    /// removes it — so a login must not read the 404 as a broken key.
    pub lists_models: bool,
    /// Whether this is an Azure resource that can say what it has deployed.
    ///
    /// The `azure` catalog lists every model Azure offers, which is not what a resource serves:
    /// that is whatever somebody deployed, under whatever name they chose. Discovery asks the
    /// resource ([`super::deployments`]) and records the answer as `served_models`.
    pub lists_deployments: bool,
    /// The other providers the same credential configures.
    ///
    /// One Foundry resource, one key, three wire APIs: logging in to any of them configures all,
    /// because asking someone to paste the same key three times is a papercut, not a design. Every
    /// member names all the others, and a test holds the group closed.
    pub paired_with: &'static [&'static str],
    /// Compat defaults; a `models.json` provider block overrides them.
    pub compat_defaults: fn() -> CompatFlags,
    /// Which of its metadata catalog's models this provider serves.
    pub served_models: ServedModels,
    /// Whether tokens are billed to a subscription rather than per token.
    ///
    /// Separate from having an OAuth method: OpenRouter's flow mints a key that bills real credits
    /// per token, so it is *not* subscription-metered.
    pub subscription_billed: bool,
    /// Models worth defaulting to, best first.
    ///
    /// A capable workhorse rather than the flagship: nobody who never chose a model should be handed
    /// the most expensive one. Only ever used when the account can actually reach the id, so a stale
    /// entry costs nothing but a fallback.
    pub preferred_models: &'static [&'static str],
}

impl ProviderDescriptor {
    /// A provider the user named at login: an endpoint with nothing curated about it.
    pub(crate) fn custom(id: String, api: ApiKind) -> Self {
        Self { id: Cow::Owned(id.clone()), name: Cow::Owned(id), api, needs_base_url: true, ..BASE }
    }

    /// The login methods to offer, in order.
    pub fn methods(&self) -> Vec<AuthMethod> {
        let mut methods = Vec::new();
        if let Some((_, label)) = self.oauth {
            methods.push(AuthMethod::subscription(label, self.oauth_detail()));
        }
        if self.api_key_login {
            methods.push(AuthMethod::api_key(self.key_detail()));
        }
        if self.keyless {
            methods.push(AuthMethod {
                id: "local".to_string(),
                label: "Connect to the local server".to_string(),
                kind: AuthMethodKind::ApiKey,
                detail: self.base_url.map(|url| format!("expects {url}")),
            });
        }
        methods
    }

    /// The compat flags to start from for this provider.
    pub fn compat(&self) -> CompatFlags {
        (self.compat_defaults)()
    }

    /// How this provider's API key goes on the wire.
    pub fn key_style(&self) -> KeyStyle {
        self.key_style.unwrap_or_else(|| KeyStyle::for_api(self.api))
    }

    /// This provider's base URL under a resource root.
    pub fn endpoint_for(&self, root: &str) -> String {
        let root = root.trim_end_matches('/');
        match self.endpoint {
            Some(endpoint) if !root.ends_with(endpoint.path) => format!("{root}{}", endpoint.path),
            _ => root.to_string(),
        }
    }

    /// What the user typed, reduced to the resource root every half hangs off.
    ///
    /// Accepts all three things someone actually has: `my-resource`, the resource root, or a full
    /// target URI — including the *other* half's, since one key configures both and nobody should
    /// have to notice which one they pasted.
    pub fn resource_root(&self, typed: &str) -> String {
        let typed = typed.trim().trim_end_matches('/');
        let Some(endpoint) = self.endpoint else { return typed.to_string() };
        if !typed.is_empty() && !typed.contains(['.', '/', ':']) {
            return endpoint.host.replace("{resource}", typed);
        }
        let candidate = match typed.contains("://") {
            true => typed.to_string(),
            false => format!("https://{typed}"),
        };
        // Match on the parsed path, never on the whole string: a resource actually named
        // `openai-eastus` puts `/openai` inside its own hostname.
        let Ok(mut url) = reqwest::Url::parse(&candidate) else { return typed.to_string() };
        if self.resource_paths().into_iter().any(|path| url.path().starts_with(path)) {
            url.set_path("");
        }
        url.to_string().trim_end_matches('/').to_string()
    }

    /// The leading path segment each provider on this resource serves under (`/anthropic`,
    /// `/openai`).
    fn resource_paths(&self) -> Vec<&'static str> {
        std::iter::once(self)
            .chain(self.paired_with.iter().filter_map(|id| provider_descriptor(id)))
            .filter_map(|descriptor| descriptor.endpoint)
            .map(|endpoint| leading_segment(endpoint.path))
            .collect()
    }

    /// Which OAuth flow this provider's subscription login runs.
    pub fn oauth_flow(&self) -> Option<OauthFlow> {
        self.oauth.map(|(flow, _)| flow)
    }

    fn oauth_detail(&self) -> Option<String> {
        match self.oauth_flow()? {
            OauthFlow::OpenaiCodex => Some("Needs a ChatGPT Plus or Pro plan".to_string()),
            OauthFlow::GithubCopilot => {
                Some("Needs a Copilot subscription; supports GitHub Enterprise".to_string())
            }
            OauthFlow::OpenRouter => {
                Some("Creates an API key you own, billed from OpenRouter credits".to_string())
            }
        }
    }

    fn key_detail(&self) -> Option<String> {
        match (self.key_hint, self.env.first()) {
            (Some(hint), Some(env)) => Some(format!("{hint} — or set {env}")),
            (Some(hint), None) => Some(hint.to_string()),
            (None, Some(env)) => Some(format!("or set {env}")),
            (None, None) => None,
        }
    }
}

/// Alternate spellings accepted on the command line and in config.
///
/// `openai_compat` is Luxe's historical default provider name; the rest are the names people
/// actually type.
const ALIASES: &[(&str, &str)] = &[
    ("openai_compat", "openai"),
    ("openai-compat", "openai"),
    ("foundry", "foundry-openai"),
    ("microsoft-foundry", "foundry-openai"),
    ("azure-foundry", "foundry-openai"),
    ("ai-foundry", "foundry-openai"),
    ("foundry-claude", "foundry-anthropic"),
    ("foundry-partners", "foundry-completions"),
    ("codex", "openai-codex"),
    ("chatgpt", "openai-codex"),
    ("copilot", "github-copilot"),
    ("github_copilot", "github-copilot"),
    ("gemini", "google"),
    ("google-gemini", "google"),
    ("grok", "xai"),
    ("kimi", "moonshotai"),
    ("moonshot", "moonshotai"),
    ("qwen", "dashscope"),
    ("together", "togetherai"),
    ("z-ai", "zai"),
    ("glm", "zai"),
];

/// `/anthropic/v1` -> `/anthropic`.
fn leading_segment(path: &'static str) -> &'static str {
    match path.get(1..).and_then(|rest| rest.find('/')) {
        Some(at) => &path[..=at],
        None => path,
    }
}

/// Resolve a user-typed provider name to a catalog id.
pub fn canonical_id(raw: &str) -> String {
    let name = raw.trim().to_ascii_lowercase();
    ALIASES
        .iter()
        .find_map(|(alias, id)| (*alias == name).then_some(*id))
        .unwrap_or(name.as_str())
        .to_string()
}

/// Every curated provider, in the order the wizard lists them: the ones most people want first,
/// then the rest alphabetically, then local servers and the custom escape hatch.
pub fn provider_descriptors() -> &'static [ProviderDescriptor] {
    CATALOG
}

/// One provider by id, accepting the aliases in [`canonical_id`].
pub fn provider_descriptor(id: &str) -> Option<&'static ProviderDescriptor> {
    let canonical = canonical_id(id);
    CATALOG.iter().find(|descriptor| descriptor.id == canonical)
}

/// The API-key environment variable for a provider, if it has one.
pub fn provider_env_vars(id: &str) -> &'static [&'static str] {
    provider_descriptor(id).map(|descriptor| descriptor.env).unwrap_or(&[])
}

/// The compat floor a declared wire API implies, for a provider the catalog has never heard of.
///
/// Keyed on the API rather than the brand, for the same reason the `max_tokens` floor is: a proxy
/// or a private deployment of Anthropic Messages caches by `cache_control` exactly as Anthropic's
/// own endpoint does. A hand-declared provider that misses it is not slightly wrong — it re-reads
/// the entire transcript at full input price on every turn, and there is no error to notice.
pub fn wire_api_compat_floor(api: &str) -> Option<CompatFlags> {
    match ApiKind::parse(api)? {
        ApiKind::AnthropicMessages => Some(anthropic_flags()),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
