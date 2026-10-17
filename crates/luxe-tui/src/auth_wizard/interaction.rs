//! The wizard's channel — the messages a sign-in sends the UI, and the bridge that turns the auth
//! service's questions into them.

use luxe_core::auth::{
    AuthError, AuthEvent, AuthInteraction, AuthPrompt, AuthService, LoginOutcome,
};
use secrecy::SecretString;
use std::sync::Arc;

/// A message from the running login task to the UI.
#[derive(Debug)]
pub enum AuthUiMsg {
    /// The flow reported progress.
    Event(AuthEvent),
    /// The flow needs an answer.
    Prompt {
        /// What to ask.
        prompt: AuthPrompt,
        /// Where the answer goes. Dropping it cancels the flow's prompt.
        reply: tokio::sync::oneshot::Sender<Result<SecretString, AuthError>>,
    },
    /// The flow finished.
    Finished(Result<LoginOutcome, AuthError>),
}

/// An [`AuthInteraction`] that forwards to the UI over a channel.
///
/// Lives here rather than in the CLI because the receiving half is the wizard's; the CLI only has to
/// hand the sender to `AuthService::login`.
pub struct ChannelInteraction {
    tx: tokio::sync::mpsc::Sender<AuthUiMsg>,
}

impl ChannelInteraction {
    /// Forward prompts and events to `tx`.
    pub fn new(tx: tokio::sync::mpsc::Sender<AuthUiMsg>) -> Self {
        Self { tx }
    }
}

#[async_trait::async_trait]
impl AuthInteraction for ChannelInteraction {
    async fn prompt(&self, prompt: AuthPrompt) -> Result<SecretString, AuthError> {
        let (reply, answer) = tokio::sync::oneshot::channel();
        if self.tx.send(AuthUiMsg::Prompt { prompt, reply }).await.is_err() {
            // The wizard closed; the flow has nobody left to ask.
            return Err(AuthError::Cancelled);
        }
        match answer.await {
            Ok(answer) => answer,
            Err(_) => Err(AuthError::Cancelled),
        }
    }

    fn notify(&self, event: AuthEvent) {
        // `try_send` on purpose: progress must never block a network call, and a full channel means
        // the UI is already behind on lines that only inform.
        let _ = self.tx.try_send(AuthUiMsg::Event(event));
    }
}

/// The catalog the wizard opens over, plus whether anything is configured yet.
///
/// Built by the CLI (which owns the service) and handed in, matching how the model catalog reaches
/// the picker.
pub type AuthServiceHandle = Arc<dyn AuthService>;
