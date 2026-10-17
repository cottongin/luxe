//! Admission — what `UserPromptSubmit` decides about a prompt, for a turn's first message and for
//! every steer sent into it.
//!
//! One gate for both, because a policy that refuses a prompt has to refuse it however it arrives:
//! a steer committed straight from the buffer reached the transcript, and every request after it,
//! with text the same hook kept out of an ordinary submission.

use super::*;

/// What the `UserPromptSubmit` hook decided about this turn.
pub(super) enum PromptGate {
    /// Proceed, with whatever context hooks added folded in.
    Proceed(Option<String>),
    /// A hook rejected the prompt. The turn is over, and nothing of it was committed.
    Blocked,
}

/// What the `UserPromptSubmit` hooks made of one prompt.
pub(super) enum Admission {
    /// It goes ahead, with whatever context the hooks added.
    Admitted(Option<String>),
    /// A hook refused it, for this reason.
    Refused(String),
}

/// The block a hook's context rides in on its user message, or `None` when it says nothing.
pub(super) fn hook_context_block(context: &str) -> Option<ContentBlock> {
    if context.trim().is_empty() {
        return None;
    }
    let context = crate::redact::process_secrets().redact(context);
    Some(ContentBlock::Text(format!("{}{context}", crate::briefs::HOOK_CONTEXT)))
}

impl Engine {
    /// Runs the `UserPromptSubmit` hook and folds its verdict into this turn's context.
    ///
    /// A hook may reject the prompt outright (validation, prompt policy) or add context for this
    /// one turn — repository conventions, git state, a policy reminder.
    pub(super) async fn apply_prompt_hook(
        &self,
        user_message: &str,
        injected_context: Option<String>,
        hook_cx: &HookCtx,
        event_tx: &mpsc::Sender<Event>,
    ) -> PromptGate {
        match self.admit_prompt(user_message, hook_cx).await {
            Admission::Refused(reason) => {
                let _ =
                    event_tx.send(Event::Error(format!("Prompt blocked by hook: {reason}"))).await;
                let _ = event_tx
                    .send(Event::TurnEnded {
                        reason: StopReason::Other("prompt blocked".to_string()),
                    })
                    .await;
                PromptGate::Blocked
            }
            Admission::Admitted(None) => PromptGate::Proceed(injected_context),
            Admission::Admitted(Some(added)) => PromptGate::Proceed(Some(match injected_context {
                Some(existing) => format!("{existing}\n\n{added}"),
                None => added,
            })),
        }
    }

    /// Runs the `UserPromptSubmit` hooks over `prompt`.
    pub(super) async fn admit_prompt(&self, prompt: &str, hook_cx: &HookCtx) -> Admission {
        let event = HookEvent::Other(serde_json::json!({ "content": prompt }));
        let outcome =
            self.config.hook_registry.invoke(HookPoint::UserPromptSubmit, &event, hook_cx).await;
        match outcome {
            HookOutcome::Deny { reason } => {
                warn!(%reason, "user prompt blocked by hook");
                Admission::Refused(reason)
            }
            HookOutcome::InjectContext(added) => Admission::Admitted(Some(added)),
            _ => Admission::Admitted(None),
        }
    }

    /// Commits every buffered steer the prompt hooks admit as a user entry chained onto `leaf`.
    ///
    /// Returns the new leaf and whether anything was delivered, so the caller can frame the next
    /// provider call for a mid-task interruption. A refused steer is said and dropped, and the turn
    /// carries on: refusing one message is not stopping the work. One
    /// [`Event::SteeringDelivered`] follows, so a UI can retire its pending markers.
    ///
    /// The messages land between the previous iteration's tool results and the next provider call,
    /// which is the only point in a turn where inserting a user turn is coherent.
    pub(super) async fn deliver_pending_steers(
        &self,
        leaf: EntryId,
        hook_cx: &HookCtx,
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<(EntryId, bool), EngineError> {
        // Left buffered under a cancel, as they were before it: the turn they were for is over
        if self.is_cancelled() {
            return Ok((leaf, false));
        }
        let steers = std::mem::take(&mut *self.pending_steers.lock().await);
        if steers.is_empty() {
            return Ok((leaf, false));
        }
        let sid = self.current_session_id().await;
        let mut leaf = leaf;
        let mut count = 0;
        for content in steers {
            let added = match self.admit_prompt(&content, hook_cx).await {
                Admission::Admitted(added) => added,
                Admission::Refused(reason) => {
                    let _ = event_tx
                        .send(Event::Error(format!("Steer blocked by hook: {reason}")))
                        .await;
                    continue;
                }
            };
            debug!(%content, "delivering steering message mid-turn");
            let mut message = Message::user(&content);
            message.content.extend(added.as_deref().and_then(hook_context_block));
            let entry_id =
                self.sessions_mut().append(&sid, Some(leaf), EntryKind::Message(message))?;
            leaf = entry_id.clone();
            self.set_leaf(entry_id.clone()).await;
            count += 1;
            // What the reader typed, without what a hook added for the model
            let message = Message::user(content);
            let _ = event_tx
                .send(Event::MessageCommitted { session_id: sid.to_string(), entry_id, message })
                .await;
        }
        let _ = event_tx.send(Event::SteeringDelivered { count }).await;
        Ok((leaf, count > 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The block the engine writes is the one a front-end recognizes, or replay shows it as
    /// something the reader typed.
    #[test]
    fn the_hook_context_block_is_recognized_as_hook_context() {
        let block = hook_context_block("branch main").expect("a block");
        let message = [ContentBlock::Text("hi".into()), block];
        assert_eq!(crate::briefs::readers_words(&message), [(0, "hi".to_string())]);
        assert!(hook_context_block("  ").is_none());
    }
}
