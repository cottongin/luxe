//! Auth fixtures — auth doubles with and without providers, their options, and the login wizard
//! seeded on a step, all without credentials or network access.

use crate::app::App;
use luxe_core::auth::{AuthMethod, CredentialStatus, ProviderOption};

fn provider_option(
    id: &str,
    name: &str,
    methods: Vec<AuthMethod>,
    status: CredentialStatus,
    note: Option<&str>,
) -> ProviderOption {
    ProviderOption {
        id: id.to_string(),
        name: name.to_string(),
        methods,
        status,
        note: note.map(str::to_string),
        console_url: None,
        keyless: id == "lmstudio",
        needs_endpoint: id == "custom",
    }
}

/// A canned [`luxe_core::auth::AuthService`] for screenshot review.
///
/// The wizard's provider list is real product content (names, notes, which providers offer a
/// subscription), so a fixture has to supply something representative — but it must never touch the
/// network or a credential file.
#[derive(Debug, Default)]
pub struct FixtureAuth;

#[async_trait::async_trait]
impl luxe_core::auth::AuthService for FixtureAuth {
    fn providers(&self) -> Vec<luxe_core::auth::ProviderOption> {
        use luxe_core::auth::{AuthMethod, CredentialKind, CredentialStatus, StoredCredentialInfo};
        use provider_option as option;
        vec![
            option(
                "openai",
                "OpenAI",
                vec![AuthMethod::api_key(Some("sk-… — or set OPENAI_API_KEY".into()))],
                CredentialStatus::Stored(StoredCredentialInfo {
                    kind: CredentialKind::ApiKey,
                    expires_at_ms: None,
                    account: None,
                }),
                None,
            ),
            option(
                "openai-codex",
                "OpenAI Codex (ChatGPT Plus/Pro)",
                vec![AuthMethod::subscription(
                    "Sign in with ChatGPT",
                    Some("Needs a ChatGPT Plus or Pro plan".into()),
                )],
                CredentialStatus::Missing,
                None,
            ),
            option(
                "anthropic",
                "Anthropic",
                vec![AuthMethod::api_key(Some("sk-ant-… — or set ANTHROPIC_API_KEY".into()))],
                CredentialStatus::Env("ANTHROPIC_API_KEY".into()),
                Some(
                    "API key only. Anthropic's terms forbid using a Claude Pro/Max subscription \
                     from a third-party harness, so Luxe does not offer that sign-in.",
                ),
            ),
            option(
                "github-copilot",
                "GitHub Copilot",
                vec![AuthMethod::subscription(
                    "Sign in with GitHub",
                    Some("Needs a Copilot subscription; supports GitHub Enterprise".into()),
                )],
                CredentialStatus::Missing,
                None,
            ),
            option(
                "openrouter",
                "OpenRouter",
                vec![
                    AuthMethod::subscription(
                        "Sign in with OpenRouter",
                        Some("Creates an API key you own, billed from OpenRouter credits".into()),
                    ),
                    AuthMethod::api_key(Some("sk-or-… — or set OPENROUTER_API_KEY".into())),
                ],
                CredentialStatus::Missing,
                None,
            ),
            option(
                "lmstudio",
                "LM Studio (local)",
                vec![AuthMethod {
                    id: "local".into(),
                    label: "Connect to the local server".into(),
                    kind: luxe_core::auth::AuthMethodKind::ApiKey,
                    detail: Some("expects http://127.0.0.1:1234/v1".into()),
                }],
                CredentialStatus::Missing,
                None,
            ),
            option(
                "custom",
                "Custom OpenAI-compatible endpoint",
                vec![AuthMethod::api_key(None)],
                CredentialStatus::Missing,
                Some("vLLM, llama.cpp, a proxy, or anything else that speaks /chat/completions"),
            ),
        ]
    }

    async fn login(
        &self,
        provider_id: &str,
        _method_id: &str,
        _interaction: std::sync::Arc<dyn luxe_core::auth::AuthInteraction>,
    ) -> Result<luxe_core::auth::LoginOutcome, luxe_core::auth::AuthError> {
        Err(luxe_core::auth::AuthError::Network(format!(
            "the fixture never talks to {provider_id}"
        )))
    }

    fn logout(&self, _provider_id: &str) -> Result<bool, luxe_core::auth::AuthError> {
        Ok(false)
    }

    fn store_path(&self) -> std::path::PathBuf {
        std::path::PathBuf::from("~/.config/luxe/auth.json")
    }
}

/// [`FixtureAuth`] with nothing configured — a first launch, before anything has been set up.
///
/// A separate service rather than a flag on the other one, because every remaining fixture wants
/// the configured list: the status badges are half of what the wizard's provider rows are for.
#[derive(Debug, Default)]
pub struct UnconfiguredAuth;

#[async_trait::async_trait]
impl luxe_core::auth::AuthService for UnconfiguredAuth {
    fn providers(&self) -> Vec<luxe_core::auth::ProviderOption> {
        FixtureAuth
            .providers()
            .into_iter()
            .map(|option| luxe_core::auth::ProviderOption {
                status: luxe_core::auth::CredentialStatus::Missing,
                ..option
            })
            .collect()
    }

    async fn login(
        &self,
        provider_id: &str,
        method_id: &str,
        interaction: std::sync::Arc<dyn luxe_core::auth::AuthInteraction>,
    ) -> Result<luxe_core::auth::LoginOutcome, luxe_core::auth::AuthError> {
        FixtureAuth.login(provider_id, method_id, interaction).await
    }

    fn logout(&self, provider_id: &str) -> Result<bool, luxe_core::auth::AuthError> {
        FixtureAuth.logout(provider_id)
    }

    fn store_path(&self) -> std::path::PathBuf {
        FixtureAuth.store_path()
    }
}

/// Which login-wizard state a screenshot should capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginFixture {
    /// The provider list, as `/login` shows it.
    Providers,
    /// The subscription list, as the Configuration surface opens it.
    Subscription,
    /// The local-or-custom list, the other end of the same menu.
    LocalOrCustom,
    /// A provider that offers more than one way in.
    Methods,
    /// An API-key prompt mid-typing.
    Secret,
    /// A device-code flow waiting on the user.
    Device,
    /// A login that landed, already handed on to the model picker.
    Done,
}

impl LoginFixture {
    /// Parse the `LUXE_FIXTURE_OVERLAY` value naming a login state.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "login" => Some(Self::Providers),
            "login-subscription" => Some(Self::Subscription),
            "login-local" => Some(Self::LocalOrCustom),
            "login-method" => Some(Self::Methods),
            "login-secret" => Some(Self::Secret),
            "login-device" => Some(Self::Device),
            "login-done" => Some(Self::Done),
            _ => None,
        }
    }

    /// The list this state opens over.
    fn scope(self) -> luxe_core::auth::LoginScope {
        use luxe_core::auth::LoginScope;
        match self {
            Self::Subscription => LoginScope::Subscription,
            Self::LocalOrCustom => LoginScope::LocalOrCustom,
            _ => LoginScope::Any,
        }
    }
}

/// Seed the login wizard into `app` in the requested state.
pub fn seed_login_wizard(app: &mut App, which: LoginFixture) {
    use crate::auth_wizard::{AuthUiMsg, AuthWizardState};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use luxe_core::auth::{AuthEvent, AuthPrompt, AuthService, CredentialKind, LoginOutcome};

    let service = FixtureAuth;
    let scope = which.scope();
    let providers = scope.filter(service.providers());
    let key = |code: KeyCode| KeyEvent::new(code, KeyModifiers::NONE);

    let mut wizard = match which {
        LoginFixture::Methods => AuthWizardState::open_for(providers, "openrouter", scope),
        LoginFixture::Secret | LoginFixture::Done => {
            AuthWizardState::open_for(providers, "openai", scope)
        }
        LoginFixture::Device => AuthWizardState::open_for(providers, "github-copilot", scope),
        LoginFixture::Providers | LoginFixture::Subscription | LoginFixture::LocalOrCustom => {
            AuthWizardState::open(providers, scope)
        }
    };
    wizard.take_pending_start();

    match which {
        LoginFixture::Secret => {
            let (reply, _rx) = tokio::sync::oneshot::channel();
            wizard.apply(AuthUiMsg::Prompt {
                prompt: AuthPrompt::secret("OpenAI API key:", Some("sk-…".into())),
                reply,
            });
            for _ in 0..24 {
                wizard.on_key(&key(KeyCode::Char('x')));
            }
        }
        LoginFixture::Device => {
            wizard.apply(AuthUiMsg::Event(AuthEvent::DeviceCode {
                user_code: "WXYZ-9876".into(),
                verification_uri: "https://github.com/login/device".into(),
                interval_secs: 5,
                expires_in_secs: 900,
            }));
        }
        _ => {}
    }
    app.auth_service = Some(std::sync::Arc::new(FixtureAuth));
    app.overlays.auth_wizard = Some(wizard);

    // Success has no screen of its own: the wizard hands the outcome back and the app walks into
    // the model picker. Driven through `Msg::Auth` because that is the door the running flow uses
    // — a fixture that called the wizard directly would not notice the app ignoring the answer.
    if which == LoginFixture::Done {
        app.update(crate::app::Msg::Auth(AuthUiMsg::Finished(Ok(LoginOutcome {
            provider_id: "openai".into(),
            provider_name: "OpenAI".into(),
            kind: Some(CredentialKind::ApiKey),
            account: None,
            suggested_model: Some("gpt-5.6".into()),
            notes: vec!["47 models available (gpt-4.1, gpt-5, gpt-5-mini, …)".into()],
            advice: Vec::new(),
        }))));
    }
}
