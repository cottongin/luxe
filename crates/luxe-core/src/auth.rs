//! Auth — the UI-neutral model of provider credentials and the login flow.
//!
//! Signing in to a provider is network work (`luxe-providers`) driven by a front-end (`luxe-tui`,
//! `luxe-cli`) that may not depend on it. This module is the seam: the types a front-end renders and
//! the two traits that connect it to a flow engine. No HTTP, no files, no wire detail.
//!
//! A login runs as [`AuthService::login`], which reports progress through
//! [`AuthInteraction::notify`] and asks for input through [`AuthInteraction::prompt`]. Both
//! front-ends implement that one trait: the CLI against the terminal, the TUI against its wizard
//! overlay.

use std::sync::Arc;

use secrecy::SecretString;

pub use crate::provider::ApiKind;

/// What kind of credential is stored for a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    /// A long-lived API key.
    ApiKey,
    /// An OAuth token pair that expires and refreshes.
    OAuth,
}

impl CredentialKind {
    /// Stable identifier (the `type` tag in `auth.json`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::OAuth => "oauth",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ApiKey => "API key",
            Self::OAuth => "subscription",
        }
    }
}

/// How a provider's credential is resolved right now.
///
/// This is *display* truth for the wizard and `luxe auth status` — the same precedence the runtime
/// applies (stored credential first, then ambient environment).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CredentialStatus {
    #[default]
    Missing,
    /// Resolved from an environment variable (named), with nothing stored.
    Env(String),
    /// Declared by hand in a config file's provider block (`models.json`, or the legacy nested
    /// `auth.json` shape). Carries the file it came from.
    Config(String),
    /// A local server that was connected, which needs no credential. Carries its endpoint.
    ///
    /// Its own state rather than an absence: "nothing to store" and "nothing set up" are the same
    /// file contents and opposite answers.
    Local(String),
    /// A credential stored in the auth file.
    Stored(StoredCredentialInfo),
}

impl CredentialStatus {
    /// Whether a turn could authenticate against this provider right now.
    pub fn is_configured(&self) -> bool {
        !matches!(self, Self::Missing)
    }

    /// Whether the auth file holds a credential, not just an environment variable.
    pub fn is_stored(&self) -> bool {
        matches!(self, Self::Stored(_))
    }
}

/// Non-secret metadata about a stored credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCredentialInfo {
    pub kind: CredentialKind,
    /// Unix-millisecond expiry, for OAuth credentials.
    pub expires_at_ms: Option<u64>,
    /// Account label to disambiguate whose credential this is (login, account id, org).
    pub account: Option<String>,
}

impl StoredCredentialInfo {
    /// Whether an OAuth credential is past (or within a minute of) its expiry.
    ///
    /// Expiry is not failure — the runtime refreshes on the next request — so status output says
    /// "expired, refreshes on next use" rather than treating it as broken.
    pub fn is_expired(&self, now_ms: u64) -> bool {
        self.expires_at_ms.is_some_and(|expires| now_ms + 60_000 >= expires)
    }
}

/// Whether a login method takes a key the user already has, or mints one interactively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethodKind {
    ApiKey,
    /// Sign in to an account/subscription (OAuth).
    Subscription,
}

/// One way to log in to a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthMethod {
    /// Stable id (`api-key`, `oauth`) — what `luxe auth login --method` accepts.
    pub id: String,
    /// Row label, e.g. `Sign in with ChatGPT (Plus/Pro)`.
    pub label: String,
    pub kind: AuthMethodKind,
    /// Optional second line: what the user needs, or what it will cost them.
    pub detail: Option<String>,
}

impl AuthMethod {
    pub fn api_key(detail: Option<String>) -> Self {
        Self {
            id: "api-key".to_string(),
            label: "Use an API key".to_string(),
            kind: AuthMethodKind::ApiKey,
            detail,
        }
    }

    pub fn subscription(label: impl Into<String>, detail: Option<String>) -> Self {
        Self {
            id: "oauth".to_string(),
            label: label.into(),
            kind: AuthMethodKind::Subscription,
            detail,
        }
    }
}

/// A provider a user can configure, as the login wizard and `luxe auth` see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOption {
    /// Provider id — the `auth.json` key and the `provider` half of a `provider/model` ref.
    pub id: String,
    pub name: String,
    /// Available login methods, in the order they should be offered.
    pub methods: Vec<AuthMethod>,
    pub status: CredentialStatus,
    /// A caveat worth reading before choosing, e.g. that only the API-key path is allowed.
    pub note: Option<String>,
    /// Where to get an API key, shown beside the key prompt
    pub console_url: Option<String>,
    /// Whether the provider needs no credential at all (a local server).
    pub keyless: bool,
    /// Whether logging in has to ask for the endpoint (a custom server, Azure).
    ///
    /// A property rather than an id, so a surface offering "local or custom" does not hard-code
    /// `custom` and go stale the day the catalog grows another custom-endpoint provider.
    pub needs_endpoint: bool,
}

impl ProviderOption {
    /// The single method to run when the user didn't pick one, if that's unambiguous.
    pub fn default_method(&self) -> Option<&AuthMethod> {
        match self.methods.len() {
            1 => self.methods.first(),
            _ => None,
        }
    }

    pub fn method(&self, id: &str) -> Option<&AuthMethod> {
        self.methods.iter().find(|method| method.id == id)
    }

    /// Text to search when filtering the provider list.
    pub fn search_key(&self) -> String {
        format!("{} {}", self.name, self.id)
    }
}

/// Which providers a login surface is offering, and what it is offering them *as*.
///
/// The wizard asks provider first and method second, which is the wrong order for someone who does
/// not yet know which providers offer a subscription. A scope inverts it: the reader picks the
/// method, and the list they then see is short, single-method, and starts its flow on Enter.
///
/// UI-neutral on purpose: the rule is about credentials, so the catalog can be tested against it
/// and `luxe auth` can reuse it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoginScope {
    /// Every configurable provider, with every method it offers.
    #[default]
    Any,
    /// Providers whose subscription Luxe is allowed to sign in to.
    Subscription,
    /// Providers that take a key you already have.
    ApiKey,
    /// A server on this machine, or an endpoint you supply.
    LocalOrCustom,
}

impl LoginScope {
    /// Stable identifier, for a CLI flag or a log field; it is a promise, so it does not change.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Subscription => "subscription",
            Self::ApiKey => "api-key",
            Self::LocalOrCustom => "local-or-custom",
        }
    }

    /// Whether this scope offers `method`.
    fn offers(self, method: &AuthMethod) -> bool {
        match self {
            Self::Any => true,
            Self::Subscription => method.kind == AuthMethodKind::Subscription,
            // A local server's method is an `ApiKey` that asks for nothing, so both key-shaped
            // scopes accept it and [`Self::covers`] decides which list it lands in.
            Self::ApiKey | Self::LocalOrCustom => method.kind == AuthMethodKind::ApiKey,
        }
    }

    /// Whether `option` belongs in this scope at all.
    ///
    /// The two key-shaped scopes overlap deliberately: a custom endpoint takes a key *and* is where
    /// someone looks for "anything else that speaks OpenAI". Each list is a lens, not a partition.
    fn covers(self, option: &ProviderOption) -> bool {
        match self {
            Self::Any | Self::Subscription | Self::ApiKey => true,
            Self::LocalOrCustom => option.keyless || option.needs_endpoint,
        }
    }

    /// `option` reduced to the methods this scope offers, or `None` when it offers none.
    pub fn narrow(self, option: ProviderOption) -> Option<ProviderOption> {
        if !self.covers(&option) {
            return None;
        }
        let methods: Vec<AuthMethod> =
            option.methods.into_iter().filter(|method| self.offers(method)).collect();
        (!methods.is_empty()).then_some(ProviderOption { methods, ..option })
    }

    /// Every provider this scope offers, each narrowed to the methods it is about.
    pub fn filter(self, options: Vec<ProviderOption>) -> Vec<ProviderOption> {
        options.into_iter().filter_map(|option| self.narrow(option)).collect()
    }
}

/// A labeled link a flow wants the user to see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthLink {
    pub url: String,
    /// Shown instead of the raw URL
    pub label: Option<String>,
}

/// One choice in a [`AuthPromptKind::Select`] prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthChoice {
    /// The value a prompt answers with when this row is picked
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
}

/// What kind of input a step needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthPromptKind {
    /// Free text, echoed.
    Text,
    /// A secret: never echoed, never logged.
    Secret,
    /// One of a fixed set of choices; the answer is the chosen [`AuthChoice::id`].
    Select(Vec<AuthChoice>),
}

/// A request for user input during a login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthPrompt {
    pub kind: AuthPromptKind,
    pub message: String,
    pub placeholder: Option<String>,
    /// Whether an empty answer is meaningful (e.g. "blank for github.com").
    pub allow_empty: bool,
}

impl AuthPrompt {
    pub fn secret(message: impl Into<String>, placeholder: Option<String>) -> Self {
        Self {
            kind: AuthPromptKind::Secret,
            message: message.into(),
            placeholder,
            allow_empty: false,
        }
    }

    pub fn text(message: impl Into<String>, placeholder: Option<String>) -> Self {
        Self {
            kind: AuthPromptKind::Text,
            message: message.into(),
            placeholder,
            allow_empty: false,
        }
    }

    /// A text prompt where an empty answer is a valid choice.
    pub fn optional_text(message: impl Into<String>, placeholder: Option<String>) -> Self {
        Self { allow_empty: true, ..Self::text(message, placeholder) }
    }

    pub fn select(message: impl Into<String>, choices: Vec<AuthChoice>) -> Self {
        Self {
            kind: AuthPromptKind::Select(choices),
            message: message.into(),
            placeholder: None,
            allow_empty: false,
        }
    }
}

/// Progress a login reports while it runs.
///
/// A front-end shows the newest event as the current step, and anything it was waiting on is
/// superseded — including a prompt the flow dropped because a loopback callback beat the paste
/// fallback. That is the whole cancellation contract for a raced prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthEvent {
    /// Something the user should read, optionally with links.
    Info {
        message: String,
        /// Links worth surfacing (docs, consoles).
        links: Vec<AuthLink>,
    },
    /// Open this URL to authorize. The front-end may launch a browser.
    OpenUrl {
        url: String,
        /// What to do once it's open.
        instructions: Option<String>,
    },
    /// A device-code flow: enter `user_code` at `verification_uri`.
    DeviceCode {
        user_code: String,
        verification_uri: String,
        /// Server-requested poll interval, in seconds.
        interval_secs: u64,
        /// How long the code stays valid, in seconds.
        expires_in_secs: u64,
    },
    /// A step is underway (exchanging a code, verifying a key, fetching models).
    Progress { message: String },
}

/// What a completed login produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginOutcome {
    pub provider_id: String,
    pub provider_name: String,
    /// Which kind of credential landed in the store, or `None` when none was needed.
    ///
    /// A local server takes no credential, and "API key" would be a claim the file contradicts.
    pub kind: Option<CredentialKind>,
    pub account: Option<String>,
    /// A model id worth defaulting to, when the flow learned one.
    pub suggested_model: Option<String>,
    /// Factual summary lines about what now exists (e.g. "17 models available").
    pub notes: Vec<String>,
    /// Advice that only makes sense once the credential exists, shown apart from the summary.
    pub advice: Vec<String>,
}

/// Why a login didn't finish.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// The user backed out (Esc, Ctrl-C, or a closed wizard).
    #[error("login cancelled")]
    Cancelled,
    #[error("unknown provider '{0}'")]
    UnknownProvider(String),
    #[error("provider '{provider}' has no '{method}' login method")]
    UnknownMethod { provider: String, method: String },
    /// The answer to a prompt was empty or malformed.
    #[error("{0}")]
    Input(String),
    #[error("network error: {0}")]
    Network(String),
    /// The provider answered, but not with what the protocol requires.
    #[error("{0}")]
    Protocol(String),
    /// The credential was rejected when we verified it.
    #[error("{0}")]
    Rejected(String),
    /// A flow ran out of time (device code expired, browser never returned).
    #[error("{0}")]
    Timeout(String),
    #[error("credential store: {0}")]
    Storage(String),
}

impl AuthError {
    /// Whether this is the user's own choice rather than a failure to report.
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

/// Where a login gets its input and sends its progress.
///
/// Implemented once per front-end. `prompt` resolves with the user's answer — a
/// [`AuthPromptKind::Select`] answer is the chosen [`AuthChoice::id`] — and returns
/// [`AuthError::Cancelled`] when the user backs out. Every answer arrives as a [`SecretString`]
/// because some of them are keys, and a uniform type means no call site can leak one by accident.
#[async_trait::async_trait]
pub trait AuthInteraction: Send + Sync {
    /// Ask the user for one piece of input.
    async fn prompt(&self, prompt: AuthPrompt) -> Result<SecretString, AuthError>;

    /// Report progress. Never blocks; a front-end that can't display it drops it.
    fn notify(&self, event: AuthEvent);
}

/// The login engine, as a front-end sees it.
///
/// Implemented in `luxe-providers` (which owns the network and the credential file) and injected
/// into the front-ends by `luxe-cli`, so neither UI crate needs a provider dependency.
#[async_trait::async_trait]
pub trait AuthService: Send + Sync {
    /// Every configurable provider, with its current credential status.
    fn providers(&self) -> Vec<ProviderOption>;

    fn provider(&self, id: &str) -> Option<ProviderOption> {
        self.providers().into_iter().find(|option| option.id == id)
    }

    /// Only the providers with a credential in the auth file — the set `/logout` can act on.
    fn stored(&self) -> Vec<ProviderOption> {
        self.providers().into_iter().filter(|option| option.status.is_stored()).collect()
    }

    /// Whether *any* provider is configured, from the file or the environment. Drives the
    /// credential-less first-run path.
    fn any_configured(&self) -> bool {
        self.providers().iter().any(|option| option.status.is_configured())
    }

    /// Run a login flow to completion, persisting the credential on success.
    async fn login(
        &self,
        provider_id: &str,
        method_id: &str,
        interaction: Arc<dyn AuthInteraction>,
    ) -> Result<LoginOutcome, AuthError>;

    /// Remove a stored credential, returning whether there was one; environment variables are left alone.
    fn logout(&self, provider_id: &str) -> Result<bool, AuthError>;

    /// Absolute path of the credential file, for status output.
    fn store_path(&self) -> std::path::PathBuf;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_kind_round_trips_and_accepts_aliases() {
        for kind in [
            ApiKind::OpenaiCompletions,
            ApiKind::OpenaiResponses,
            ApiKind::AnthropicMessages,
            ApiKind::OpenaiCodex,
        ] {
            assert_eq!(ApiKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(ApiKind::parse("openai_responses"), Some(ApiKind::OpenaiResponses));
        assert_eq!(ApiKind::parse("  ANTHROPIC  "), Some(ApiKind::AnthropicMessages));
        assert_eq!(ApiKind::parse("nope"), None);
    }

    fn option(id: &str, methods: Vec<AuthMethod>) -> ProviderOption {
        ProviderOption {
            id: id.into(),
            name: id.to_uppercase(),
            methods,
            status: CredentialStatus::Missing,
            note: None,
            console_url: None,
            keyless: false,
            needs_endpoint: false,
        }
    }

    #[test]
    fn a_single_method_is_the_default_and_several_are_not() {
        let mut option = option("openai", vec![AuthMethod::api_key(None)]);
        assert_eq!(option.default_method().map(|m| m.id.as_str()), Some("api-key"));
        option.methods.push(AuthMethod::subscription("Sign in with ChatGPT", None));
        assert!(option.default_method().is_none(), "an ambiguous choice must be the user's");
        assert!(option.method("oauth").is_some());
    }

    /// The point of a scope: what comes back is single-method, so choosing a provider starts its
    /// flow instead of asking which way in.
    #[test]
    fn a_scope_narrows_a_provider_to_the_methods_it_is_about() {
        let both = option(
            "openrouter",
            vec![
                AuthMethod::subscription("Sign in with OpenRouter", None),
                AuthMethod::api_key(None),
            ],
        );

        let subscription = LoginScope::Subscription.narrow(both.clone()).expect("offers one");
        assert_eq!(subscription.methods.len(), 1);
        assert_eq!(subscription.default_method().map(|m| m.id.as_str()), Some("oauth"));

        let key = LoginScope::ApiKey.narrow(both.clone()).expect("offers one");
        assert_eq!(key.default_method().map(|m| m.id.as_str()), Some("api-key"));

        assert_eq!(LoginScope::Any.narrow(both).map(|o| o.methods.len()), Some(2));
    }

    #[test]
    fn a_provider_with_nothing_the_scope_offers_is_dropped() {
        let anthropic = option("anthropic", vec![AuthMethod::api_key(None)]);
        assert!(
            LoginScope::Subscription.narrow(anthropic).is_none(),
            "a key-only provider must not appear under subscriptions"
        );
    }

    /// Local servers and endpoints the user supplies are the same question asked twice, and a
    /// hosted provider that only takes a key is neither.
    #[test]
    fn local_or_custom_covers_keyless_servers_and_supplied_endpoints() {
        let mut local = option("lmstudio", vec![AuthMethod::api_key(None)]);
        local.keyless = true;
        let mut custom = option("custom", vec![AuthMethod::api_key(None)]);
        custom.needs_endpoint = true;
        let hosted = option("groq", vec![AuthMethod::api_key(None)]);

        let scoped = LoginScope::LocalOrCustom.filter(vec![local, custom, hosted.clone()]);
        let ids: Vec<&str> = scoped.iter().map(|option| option.id.as_str()).collect();
        assert_eq!(ids, vec!["lmstudio", "custom"]);
        // The same custom endpoint still belongs in the key list: it takes one.
        assert_eq!(LoginScope::ApiKey.filter(vec![hosted]).len(), 1);
    }

    #[test]
    fn status_distinguishes_stored_from_ambient() {
        assert!(!CredentialStatus::Missing.is_configured());
        let env = CredentialStatus::Env("OPENAI_API_KEY".into());
        assert!(env.is_configured() && !env.is_stored(), "env counts as configured, not stored");
        // A hand-written provider block counts too, or someone with a working models.json would be
        // shown a first-run wizard.
        let config = CredentialStatus::Config("models.json".into());
        assert!(config.is_configured() && !config.is_stored());
        let stored = CredentialStatus::Stored(StoredCredentialInfo {
            kind: CredentialKind::OAuth,
            expires_at_ms: Some(10_000),
            account: None,
        });
        assert!(stored.is_configured() && stored.is_stored());
    }

    #[test]
    fn expiry_uses_a_one_minute_margin() {
        let info = StoredCredentialInfo {
            kind: CredentialKind::OAuth,
            expires_at_ms: Some(600_000),
            account: None,
        };
        assert!(!info.is_expired(500_000));
        assert!(info.is_expired(560_000), "within the margin counts as expiring");
        assert!(info.is_expired(700_000));
        let key = StoredCredentialInfo {
            kind: CredentialKind::ApiKey,
            expires_at_ms: None,
            account: None,
        };
        assert!(!key.is_expired(u64::MAX), "an API key never expires");
    }

    /// The secret wrapper hides its contents from the formatter it is most likely to meet.
    ///
    /// Every credential in the product rides in a [`secrecy::SecretString`], and what keeps one out
    /// of a log line or a panic message is that crate's `Debug` — not a rule anyone here enforces.
    /// That makes it someone else's invariant, which is exactly the kind that changes under a major
    /// bump without a compiler error: the wrapper kept its name and its API across `0.8 -> 0.10`
    /// while its whole internal shape changed. A `{:?}` that starts printing keys is silent, so it
    /// is pinned here rather than trusted.
    #[test]
    fn a_secret_does_not_print_itself() {
        use secrecy::{ExposeSecret, SecretString};

        let secret = SecretString::from("sk-not-in-the-log".to_string());
        let rendered = format!("{secret:?}");

        assert!(!rendered.contains("sk-not-in-the-log"), "Debug leaked it: {rendered}");
        assert!(rendered.contains("REDACTED"), "and it says so plainly: {rendered}");
        assert_eq!(secret.expose_secret(), "sk-not-in-the-log", "still recoverable on purpose");
    }
}
