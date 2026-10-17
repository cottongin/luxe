//! Sign-in — the login wizard, the flows it starts, and what a login leaves the session on.

use super::{App, ConvoBlock};

impl App {
    /// Whether a turn could authenticate right now.
    ///
    /// Asked of the auth service, which is the thing that reads the credential store; a build with
    /// no service injected (a test harness, an embedder) is assumed configured, because it is not
    /// this app's place to claim otherwise.
    pub fn provider_configured(&self) -> bool {
        self.auth_service.as_ref().is_none_or(|service| service.any_configured())
    }

    /// Opens the login wizard over every provider (`/login`, `/login openai`).
    pub fn open_auth_wizard(&mut self, provider: Option<&str>) {
        self.open_auth_wizard_scoped(provider, luxe_core::auth::LoginScope::Any);
    }

    /// Opens the login wizard over the providers `scope` covers.
    ///
    /// `provider` jumps straight to one, skipping the list. No-ops with a hint when the CLI didn't
    /// inject an auth service (a test harness or an embedder).
    pub fn open_auth_wizard_scoped(
        &mut self,
        provider: Option<&str>,
        scope: luxe_core::auth::LoginScope,
    ) {
        let Some(service) = self.auth_service.clone() else {
            self.push_convo(ConvoBlock::System(format!(
                "Provider sign-in isn't available in this build — set credentials in {}, or run \
                 `luxe auth login`.",
                luxe_core::user_dirs::config_path_display("auth.json")
            )));
            return;
        };
        // Narrowed here rather than in the wizard: the service owns what a provider offers, and a
        // list that disagrees with the heading above it is worse than either.
        let providers = scope.filter(service.providers());
        // Resolve through the service so `/login copilot` reaches `github-copilot`: the alias table
        // belongs to the provider catalog, not to the UI. An unknown name falls back to the list,
        // which is friendlier than an error for what is usually a typo.
        let target = provider.and_then(|id| service.provider(id)).map(|option| option.id);
        self.overlays.auth_wizard = Some(match target {
            Some(id) => crate::auth_wizard::AuthWizardState::open_for(providers, &id, scope),
            None => crate::auth_wizard::AuthWizardState::open(providers, scope),
        });
        // A provider named on the command line can start its flow immediately.
        self.start_pending_login();
    }

    /// Closes the wizard, declining anything the flow was still waiting on.
    pub fn close_auth_wizard(&mut self) {
        if let Some(mut wizard) = self.overlays.auth_wizard.take() {
            wizard.cancel_pending();
        }
    }

    /// Report every provider's credential status into the transcript (`/auth`).
    pub fn report_auth_status(&mut self) {
        let Some(service) = self.auth_service.clone() else {
            self.push_convo(ConvoBlock::System(
                "Credential status isn't available in this build.".to_string(),
            ));
            return;
        };
        let mut lines = vec![format!("Credentials ({}):", service.store_path().display())];
        let configured: Vec<_> = service
            .providers()
            .into_iter()
            .filter(|option| option.status.is_configured())
            .collect();
        if configured.is_empty() {
            lines.push("  none yet — run /login to set one up".to_string());
        }
        for option in configured {
            let detail = match &option.status {
                luxe_core::auth::CredentialStatus::Env(name) => format!("env: {name}"),
                luxe_core::auth::CredentialStatus::Config(file) => format!("config: {file}"),
                luxe_core::auth::CredentialStatus::Local(endpoint) => {
                    format!("local server — {endpoint}")
                }
                luxe_core::auth::CredentialStatus::Stored(info) => match info.account.as_deref() {
                    Some(account) => format!("{} — {account}", info.kind.label()),
                    None => info.kind.label().to_string(),
                },
                luxe_core::auth::CredentialStatus::Missing => "—".to_string(),
            };
            lines.push(format!("  {:<16} {detail}", option.id));
        }
        lines.push("  /login to add one · /logout <provider> to remove one".to_string());
        self.push_convo(ConvoBlock::System(lines.join("\n")));
    }

    /// Remove a stored credential (`/logout <provider>`), or list what could be removed.
    pub fn logout_provider(&mut self, provider: Option<&str>) {
        let Some(service) = self.auth_service.clone() else {
            self.push_convo(ConvoBlock::System(
                "Sign-out isn't available in this build.".to_string(),
            ));
            return;
        };
        let Some(provider) = provider else {
            let stored: Vec<String> =
                service.stored().into_iter().map(|option| option.id).collect();
            let message = match stored.is_empty() {
                true => "Nothing is signed in. /login sets a provider up.".to_string(),
                false => format!("Usage: /logout <provider> — signed in: {}", stored.join(", ")),
            };
            self.push_convo(ConvoBlock::System(message));
            return;
        };
        let message = match service.logout(provider) {
            Ok(true) => format!("Signed out of {provider}. Its stored credential is gone."),
            Ok(false) => format!("Nothing stored for {provider}."),
            Err(error) => format!("Couldn't sign out of {provider}: {error}"),
        };
        self.push_convo(ConvoBlock::System(message));
    }

    /// Start a login the wizard asked for, spawning the flow on a task.
    ///
    /// Called after any wizard interaction that can produce a `Start`, so the wizard's state machine
    /// stays synchronous and testable while the flow itself runs async.
    pub(crate) fn start_login(&mut self, provider: String, method: String) {
        let (Some(service), Some(tx)) = (self.auth_service.clone(), self.auth_tx.clone()) else {
            return;
        };
        let interaction: std::sync::Arc<dyn luxe_core::auth::AuthInteraction> =
            std::sync::Arc::new(crate::auth_wizard::ChannelInteraction::new(tx.clone()));
        tokio::spawn(async move {
            let outcome = service.login(&provider, &method, interaction).await;
            let _ = tx.send(crate::auth_wizard::AuthUiMsg::Finished(outcome)).await;
        });
    }

    /// Kick off a login the wizard queued while opening (`/login <provider>` with one method).
    fn start_pending_login(&mut self) {
        let pending =
            self.overlays.auth_wizard.as_mut().and_then(|wizard| wizard.take_pending_start());
        if let Some((provider, method)) = pending {
            self.start_login(provider, method);
        }
    }

    /// Apply a wizard outcome: start a flow, close, or walk a finished login into the picker.
    ///
    /// Closing over the Configuration surface returns to its menu rather than out of it, so a
    /// cancelled login lands back where it was chosen.
    pub(crate) fn apply_wizard_outcome(&mut self, outcome: crate::auth_wizard::WizardOutcome) {
        use crate::auth_wizard::WizardOutcome;
        match outcome {
            WizardOutcome::Start { provider, method } => self.start_login(provider, method),
            WizardOutcome::Answer | WizardOutcome::Consumed => {}
            WizardOutcome::Close => {
                self.close_auth_wizard();
                self.refresh_configuration();
            }
            WizardOutcome::Finished(outcome) => self.finish_login(*outcome),
        }
    }

    /// Close the wizard on a credential that landed and go straight to choosing a model.
    ///
    /// Choosing one is the rest of the same job — a provider with no model runs nothing — so the
    /// flow does not stop to be congratulated. What the login had to say (the account, how many
    /// models it reached, any advice that only makes sense now) becomes the picker's opening line,
    /// and the transcript keeps the whole of it for a reader who scrolls back.
    fn finish_login(&mut self, outcome: luxe_core::auth::LoginOutcome) {
        self.close_auth_wizard();
        let adopted = self.adopt_login(&outcome);
        self.refresh_configuration();
        let configured = self.provider_configured();
        self.set_provider_configured(configured);
        self.push_convo(ConvoBlock::System(login_report(&outcome, adopted.as_deref())));
        self.open_model_picker_with(login_note(&outcome, adopted.as_deref()));
    }

    /// Re-read what is configured, so the Configuration summary reflects a login that just landed.
    fn refresh_configuration(&mut self) {
        let Some(service) = self.auth_service.clone() else { return };
        if let Some(configuration) = self.overlays.configuration.as_mut() {
            configuration.refresh(&service.providers());
        }
    }

    /// Switch the session onto a provider that was just configured, answering what it landed on.
    ///
    /// Without this, a login only takes effect on the next launch: the engine keeps routing to
    /// whatever was active when the session started. The catalog is rebuilt first so a model that
    /// only exists because of this login can be found.
    fn adopt_login(&mut self, outcome: &luxe_core::auth::LoginOutcome) -> Option<String> {
        if let Some(sources) = &self.model_sources {
            self.model_catalog = (sources.build)().catalog;
        }
        let provider = outcome.provider_id.as_str();
        let entry = self.model_catalog.iter().find(|entry| {
            entry.info.r#ref.provider == provider
                && outcome.suggested_model.as_deref().is_some_and(|id| entry.info.r#ref.id == id)
        });
        // Fall back to any model for that provider, so a login always leaves a usable session.
        let info = entry
            .or_else(|| {
                self.model_catalog.iter().find(|entry| entry.info.r#ref.provider == provider)
            })
            .map(|entry| entry.info.clone())?;
        let display = info.display().to_string();
        self.apply_model_selection(info);
        Some(display)
    }
}

/// What the model picker says at the top of itself after a login. Args: `(outcome, adopted)`.
///
/// Two lines at most, because the reader is on their way to choosing a model and this is a caption
/// rather than a report — the whole of it is in the transcript ([`login_report`]). `adopted` is
/// the model the login already left the session on, which is the row the cursor is sitting on;
/// saying so is what stops it reading as a choice the reader has not made yet.
fn login_note(outcome: &luxe_core::auth::LoginOutcome, adopted: Option<&str>) -> Vec<String> {
    // No count of the models, and none of their names: they are the rows underneath this line.
    vec![
        format!("{}.", configured_headline(outcome)),
        match adopted {
            Some(model) => format!("Now on {model} — enter keeps it, or choose another."),
            None => "Choose a model to finish setting up.".to_string(),
        },
    ]
}

/// "Signed in to OpenAI (API key)", or "Connected to LM Studio (local)" when nothing was stored.
///
/// A local server takes no credential, so calling it a sign-in describes a file that does not
/// exist — and its name already says what it is, which is why the kind is only named when there
/// is one.
fn configured_headline(outcome: &luxe_core::auth::LoginOutcome) -> String {
    match outcome.kind {
        Some(kind) => {
            format!("Signed in to {} ({})", outcome.provider_name, kind.label())
        }
        None => format!("Connected to {}", outcome.provider_name),
    }
}

/// The whole of what a login had to say, for the transcript. Args: `(outcome, adopted)`.
fn login_report(outcome: &luxe_core::auth::LoginOutcome, adopted: Option<&str>) -> String {
    let mut lines = vec![format!("{}.", configured_headline(outcome))];
    if let Some(account) = &outcome.account {
        lines.push(format!("Account: {account}"));
    }
    lines.extend(outcome.notes.iter().cloned());
    if let Some(model) = adopted {
        lines.push(format!("Model: {model}"));
    }
    lines.extend(outcome.advice.iter().cloned());
    lines.join("\n")
}
