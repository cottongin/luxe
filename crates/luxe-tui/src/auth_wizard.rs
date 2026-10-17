//! Login wizard — the panel behind `/login`, and behind every row of the Configuration surface.
//!
//! Pure state plus rendering, in the same shape as the other overlays: the CLI injects an
//! [`luxe_core::auth::AuthService`], this module walks the user through it, and the flow's questions
//! arrive as [`AuthUiMsg`]s from the task running the login.
//!
//! It opens over a [`luxe_core::auth::LoginScope`], which is the list it shows and the framing it
//! wears. `/login` opens the whole catalog; a Configuration row opens one method's worth of it,
//! already narrowed, so the provider list is short and Enter starts the flow.
//!
//! Three stages the reader walks — pick a provider, pick a method, run the flow — and a fourth the
//! wizard only reaches when something went wrong. The running stage is a transcript: every
//! [`luxe_core::auth::AuthEvent`] appends a line, and a prompt takes over the bottom of the panel
//! until it's answered. A dropped prompt (the loopback callback beating the paste fallback) is
//! cleared by the next event, which is the whole cancellation contract.
//!
//! Success has no stage of its own. A credential that landed is not something to read about, it is
//! the reason you opened this — so the wizard hands the outcome back as
//! [`WizardOutcome::Finished`] and the app walks straight into choosing a model, carrying what the
//! flow had to say as the picker's opening line.

mod interaction;
mod prompt;
mod providers;
mod render;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
pub use interaction::{AuthServiceHandle, AuthUiMsg, ChannelInteraction};
use luxe_core::auth::{
    AuthChoice, AuthError, AuthEvent, AuthPrompt, AuthPromptKind, LoginOutcome, LoginScope,
    ProviderOption,
};
use secrecy::SecretString;

/// One line in the running transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WizardLine {
    /// Informational text.
    Info(String),
    /// A step that's underway.
    Progress(String),
    /// A URL to open, with the instruction that came with it.
    Url {
        /// Where to go.
        url: String,
        /// What to do there.
        instructions: Option<String>,
    },
    /// A device code to type at a URL.
    DeviceCode {
        /// Code to enter.
        code: String,
        /// Where to enter it.
        url: String,
        /// Minutes until it expires.
        expires_in_minutes: u64,
    },
}

/// A prompt the wizard is currently showing.
#[derive(Debug)]
struct Pending {
    prompt: AuthPrompt,
    reply: tokio::sync::oneshot::Sender<Result<SecretString, AuthError>>,
    /// Typed answer (text/secret prompts).
    input: prompt::AnswerBuffer,
    /// Highlighted row (select prompts).
    selected: usize,
}

impl Pending {
    fn choices(&self) -> &[AuthChoice] {
        match &self.prompt.kind {
            AuthPromptKind::Select(choices) => choices,
            _ => &[],
        }
    }
}

/// Which step the wizard is on.
#[derive(Debug)]
pub enum Stage {
    /// Choosing a provider.
    Provider {
        /// Fuzzy filter text.
        query: String,
        /// Indices into `providers` that match `query`.
        filtered: Vec<usize>,
        /// Highlighted row within `filtered`.
        selected: usize,
    },
    /// Choosing how to sign in to `provider`.
    Method {
        /// Index into `providers`.
        provider: usize,
        /// Highlighted method.
        selected: usize,
    },
    /// A login is running.
    Running {
        /// Provider being configured.
        provider: String,
        /// What the flow has reported so far.
        lines: Vec<WizardLine>,
    },
    /// The login failed (or was cancelled with something worth saying).
    Failed(String),
}

/// What a keypress asked the app to do.
#[derive(Debug)]
pub enum WizardOutcome {
    /// Start this provider's login with this method.
    Start {
        /// Provider id.
        provider: String,
        /// Method id.
        method: String,
    },
    /// Answer the flow's pending prompt.
    Answer,
    /// Close the wizard, aborting anything running.
    Close,
    /// A credential landed. Args: `(outcome)`. The app closes the wizard and picks a model.
    Finished(Box<LoginOutcome>),
    /// Key consumed; the wizard stays as it is.
    Consumed,
}

/// The wizard's state.
#[derive(Debug)]
pub struct AuthWizardState {
    /// Every configurable provider, as the service reported them when the wizard opened.
    providers: Vec<ProviderOption>,
    /// Current step.
    stage: Stage,
    /// The question the running flow is waiting on. Orthogonal to the stage: a flow can report
    /// progress while a prompt is up, and the prompt outlives individual transcript lines.
    pending: Option<Pending>,
    /// A flow that opening the wizard already committed to (`/login openai`), waiting to be spawned
    /// by the caller. Opening can't start a task itself — it's a pure state transition.
    pending_start: Option<(String, String)>,
    /// What the providers were narrowed to, which is also how the panel introduces itself.
    scope: LoginScope,
}

impl AuthWizardState {
    /// Open on the provider list.
    ///
    /// `providers` is expected to be narrowed to `scope` already — the caller owns the service, and
    /// a list that disagrees with its own heading is worse than either.
    pub fn open(providers: Vec<ProviderOption>, scope: LoginScope) -> Self {
        let filtered = (0..providers.len()).collect();
        Self {
            providers,
            stage: Stage::Provider { query: String::new(), filtered, selected: 0 },
            pending: None,
            pending_start: None,
            scope,
        }
    }

    /// Open straight into one provider (`/login openai`), skipping the list.
    ///
    /// Falls back to the list when the id isn't one we know, so a typo is recoverable rather than an
    /// error dialog.
    pub fn open_for(providers: Vec<ProviderOption>, provider_id: &str, scope: LoginScope) -> Self {
        let mut state = Self::open(providers, scope);
        if let Some(index) = state.providers.iter().position(|option| option.id == provider_id) {
            if let Some(WizardOutcome::Start { provider, method }) = state.enter_provider(index) {
                state.pending_start = Some((provider, method));
            }
        }
        state
    }

    /// Take the login that opening the wizard already committed to, if any.
    pub fn take_pending_start(&mut self) -> Option<(String, String)> {
        self.pending_start.take()
    }

    /// The scope this wizard was opened over.
    pub fn scope(&self) -> LoginScope {
        self.scope
    }

    /// The current stage, for rendering.
    pub fn stage(&self) -> &Stage {
        &self.stage
    }

    /// The providers being offered.
    pub fn providers(&self) -> &[ProviderOption] {
        &self.providers
    }

    /// Highlighted row in the current list stage.
    pub fn selected_row(&self) -> usize {
        match &self.stage {
            Stage::Provider { selected, .. } => *selected,
            Stage::Method { selected, .. } => *selected,
            Stage::Running { .. } => self.pending.as_ref().map(|p| p.selected).unwrap_or(0),
            _ => 0,
        }
    }

    /// The current filter text, when a list is showing.
    pub fn query(&self) -> &str {
        match &self.stage {
            Stage::Provider { query, .. } => query,
            _ => "",
        }
    }

    /// The prompt the wizard is waiting on, for rendering.
    pub fn pending_prompt(&self) -> Option<(&AuthPrompt, &str)> {
        let pending = self.pending.as_ref()?;
        matches!(self.stage, Stage::Running { .. })
            .then_some((&pending.prompt, pending.input.as_str()))
    }

    /// Whether a running flow has a question outstanding.
    pub fn is_awaiting_answer(&self) -> bool {
        self.pending_prompt().is_some()
    }

    /// Transcript lines of a running (or finished) flow.
    pub fn lines(&self) -> &[WizardLine] {
        match &self.stage {
            Stage::Running { lines, .. } => lines,
            _ => &[],
        }
    }

    /// Move to a provider: straight into the flow when there's only one way in, else its methods.
    fn enter_provider(&mut self, index: usize) -> Option<WizardOutcome> {
        let option = self.providers.get(index)?;
        match option.default_method() {
            Some(method) => {
                let outcome =
                    WizardOutcome::Start { provider: option.id.clone(), method: method.id.clone() };
                self.stage = Stage::Running { provider: option.name.clone(), lines: Vec::new() };
                Some(outcome)
            }
            None => {
                self.stage = Stage::Method { provider: index, selected: 0 };
                None
            }
        }
    }

    /// Apply a message from the login task, answering what the app has to do about it.
    ///
    /// A new event supersedes a pending prompt: that is how a raced paste prompt disappears when the
    /// browser callback wins. A credential that landed is [`WizardOutcome::Finished`], because the
    /// wizard's work is over at that point and the next step belongs to the app.
    pub fn apply(&mut self, message: AuthUiMsg) -> WizardOutcome {
        match message {
            AuthUiMsg::Event(event) => {
                // Progress means the flow moved on, so anything it was asking is moot — that is how
                // a raced paste prompt clears itself when the browser callback wins. An info line
                // must *not* clear it, or a warning would eat a half-typed key.
                if matches!(event, AuthEvent::Progress { .. }) {
                    self.pending = None;
                }
                if let Stage::Running { lines, .. } = &mut self.stage {
                    lines.push(line_for(event));
                }
            }
            AuthUiMsg::Prompt { prompt, reply } => match self.stage {
                Stage::Running { .. } => {
                    self.pending =
                        Some(Pending { prompt, reply, input: Default::default(), selected: 0 });
                }
                // Nothing is running (the user closed the stage): decline rather than hang.
                _ => {
                    let _ = reply.send(Err(AuthError::Cancelled));
                }
            },
            AuthUiMsg::Finished(Ok(outcome)) => return WizardOutcome::Finished(Box::new(outcome)),
            AuthUiMsg::Finished(Err(AuthError::Cancelled)) => {
                self.stage = Stage::Failed("Cancelled. Nothing was saved.".to_string())
            }
            AuthUiMsg::Finished(Err(error)) => self.stage = Stage::Failed(error.to_string()),
        }
        WizardOutcome::Consumed
    }

    /// Handle a keypress.
    pub fn on_key(&mut self, key: &KeyEvent) -> WizardOutcome {
        match &mut self.stage {
            Stage::Provider { .. } => self.provider_key(key),
            Stage::Method { .. } => self.method_key(key),
            Stage::Running { .. } => self.running_key(key),
            Stage::Failed(_) => match key.code {
                // Retry from the top rather than making the user reopen the wizard.
                KeyCode::Char('r') | KeyCode::Enter => {
                    let filtered = (0..self.providers.len()).collect();
                    self.stage = Stage::Provider { query: String::new(), filtered, selected: 0 };
                    WizardOutcome::Consumed
                }
                _ => WizardOutcome::Close,
            },
        }
    }

    fn running_key(&mut self, key: &KeyEvent) -> WizardOutcome {
        let Some(current) = self.pending.as_mut() else {
            // Nothing to answer: Esc aborts the flow, everything else waits.
            return match key.code {
                KeyCode::Esc => WizardOutcome::Close,
                _ => WizardOutcome::Consumed,
            };
        };

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let choices = current.choices().len();
        match key.code {
            KeyCode::Esc => WizardOutcome::Close,
            KeyCode::Up if choices > 0 => {
                current.selected = current.selected.saturating_sub(1);
                WizardOutcome::Consumed
            }
            KeyCode::Down if choices > 0 => {
                current.selected = (current.selected + 1).min(choices.saturating_sub(1));
                WizardOutcome::Consumed
            }
            KeyCode::Char(ch) if choices > 0 && ch.is_ascii_digit() => {
                let row = (ch as usize).wrapping_sub('1' as usize);
                if row < choices {
                    current.selected = row;
                }
                WizardOutcome::Consumed
            }
            KeyCode::Enter => {
                if current.input.is_blank() && !current.prompt.allow_empty && choices == 0 {
                    return WizardOutcome::Consumed;
                }
                let Some(pending) = self.pending.take() else { return WizardOutcome::Consumed };
                let choice = pending.choices().get(pending.selected).map(|c| c.id.clone());
                let answer = choice.map_or_else(|| pending.input.into_secret(), SecretString::from);
                let _ = pending.reply.send(Ok(answer));
                WizardOutcome::Answer
            }
            KeyCode::Backspace if choices == 0 => {
                current.input.pop();
                WizardOutcome::Consumed
            }
            KeyCode::Char('u') if ctrl && choices == 0 => {
                current.input.clear();
                WizardOutcome::Consumed
            }
            KeyCode::Char(ch) if choices == 0 && !ctrl => {
                current.input.push(ch);
                WizardOutcome::Consumed
            }
            _ => WizardOutcome::Consumed,
        }
    }

    /// Take pasted text into whatever the wizard is collecting.
    ///
    /// The one way most people enter an API key, and for a while the only surface that never saw
    /// it: paste went to the composer behind the wizard, so a key appeared to vanish and then
    /// turned up in the next session's prompt. Answers `false` when there is nothing to type into
    /// — a list stage, a select prompt — which is still the wizard's answer, because a modal that
    /// declines a paste must swallow it rather than pass it back.
    pub fn paste(&mut self, text: &str) -> bool {
        let text = crate::editor::single_line_paste(text);
        if text.is_empty() {
            return false;
        }
        if let Some(pending) = self.pending.as_mut() {
            if matches!(self.stage, Stage::Running { .. }) && pending.choices().is_empty() {
                pending.input.push_str(&text);
                return true;
            }
            return false;
        }
        // The provider list is a search box, so a pasted name filters it like a typed one.
        if let Stage::Provider { query, .. } = &mut self.stage {
            query.push_str(&text);
            self.refilter();
            return true;
        }
        false
    }

    /// Decline any outstanding prompt so the login task stops waiting on a closed wizard.
    pub fn cancel_pending(&mut self) {
        if let Some(pending) = self.pending.take() {
            let _ = pending.reply.send(Err(AuthError::Cancelled));
        }
    }
}

/// Turn a flow event into a transcript line.
fn line_for(event: AuthEvent) -> WizardLine {
    match event {
        AuthEvent::Info { message, links } => {
            // Links ride along in the text: the panel is narrow and a bare URL is what the user
            // needs to see anyway.
            match links.first() {
                Some(link) => WizardLine::Info(format!("{message} {}", link.url)),
                None => WizardLine::Info(message),
            }
        }
        AuthEvent::OpenUrl { url, instructions } => WizardLine::Url { url, instructions },
        AuthEvent::DeviceCode { user_code, verification_uri, expires_in_secs, .. } => {
            WizardLine::DeviceCode {
                code: user_code,
                url: verification_uri,
                expires_in_minutes: expires_in_secs / 60,
            }
        }
        AuthEvent::Progress { message } => WizardLine::Progress(message),
    }
}

/// Fixtures shared by this module's test modules.
#[cfg(test)]
mod tests_support {
    use super::*;
    use luxe_core::auth::{AuthMethod, CredentialStatus};

    pub(super) fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn option(id: &str, methods: Vec<AuthMethod>) -> ProviderOption {
        ProviderOption {
            id: id.to_string(),
            name: id.to_uppercase(),
            methods,
            status: CredentialStatus::Missing,
            note: None,
            console_url: None,
            keyless: false,
            needs_endpoint: false,
        }
    }

    pub(super) fn providers() -> Vec<ProviderOption> {
        vec![
            option("openai", vec![AuthMethod::api_key(None)]),
            option(
                "openrouter",
                vec![
                    AuthMethod::subscription("Sign in with OpenRouter", None),
                    AuthMethod::api_key(None),
                ],
            ),
            option("anthropic", vec![AuthMethod::api_key(None)]),
        ]
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod mouse_tests {
    use super::tests_support::*;
    use super::*;

    /// A published click region that selects nothing is a dead affordance; these mirror what the
    /// renderer actually publishes.
    #[test]
    fn clicking_a_provider_row_selects_it() {
        let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
        wizard.select_provider(2);
        assert_eq!(wizard.selected_row(), 2);
        // Out of range clamps rather than panicking on a stale region from the previous frame.
        wizard.select_provider(99);
        assert_eq!(wizard.selected_row(), wizard.visible_providers().len() - 1);
    }

    #[test]
    fn clicking_a_method_row_selects_it_and_a_choice_row_when_one_is_up() {
        let mut wizard = AuthWizardState::open(providers(), LoginScope::Any);
        wizard.on_key(&key(KeyCode::Down));
        wizard.on_key(&key(KeyCode::Enter));
        wizard.select_method(1);
        assert_eq!(wizard.selected_row(), 1);

        // With a select prompt showing, the same click drives the prompt's list.
        let mut running = AuthWizardState::open(providers(), LoginScope::Any);
        running.on_key(&key(KeyCode::Enter));
        let (reply, _rx) = tokio::sync::oneshot::channel();
        running.apply(AuthUiMsg::Prompt {
            prompt: AuthPrompt::select(
                "How?",
                vec![
                    AuthChoice { id: "a".into(), label: "A".into(), detail: None },
                    AuthChoice { id: "b".into(), label: "B".into(), detail: None },
                ],
            ),
            reply,
        });
        running.select_method(1);
        assert_eq!(running.selected_row(), 1);
    }
}
