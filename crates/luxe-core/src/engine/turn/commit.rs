//! Commit — what a turn writes to the transcript: the user's message, the reply, and the results.

use super::admission::hook_context_block;
use super::phases::{result_for, CommittedReply};
use super::*;

/// A user message carrying `text` and `images`, with no empty block where the text would be.
///
/// A pasted screenshot with no caption is an ordinary way to ask a question, and an empty text
/// block beside it is one Anthropic rejects — on that request and, the message being history, on
/// every later one.
pub(super) fn user_message_with(text: &str, images: &[ImageAttachment]) -> Message {
    let mut message = match text.trim().is_empty() {
        true => Message { role: Role::User, content: Vec::new() },
        false => Message::user(text),
    };
    message.content.extend(images.iter().map(ImageAttachment::to_block));
    message
}

impl Engine {
    /// Commits the user's message and makes it the active leaf, returning its entry.
    ///
    /// `context` is what hooks added for this turn, and it is committed *with* the message, as a
    /// block after what the user wrote. In the system prompt, a hook reporting anything live (git
    /// state, a clock) would change the prefix every turn and re-bill the whole conversation; in
    /// the transcript it is history like everything else: replayed verbatim, cached, resumed.
    ///
    /// A remediation turn that is *re-running* an existing feedback submission reuses that
    /// submission's entry rather than appending a second copy of the same request — otherwise a
    /// retry grows the transcript by one identical message every time it is attempted. That entry
    /// is already history, so a re-run's context is not added to it.
    ///
    /// Every committed message is announced with `MessageCommitted`, the reader's own words too:
    /// a client joined to the session didn't type them, and the front-end that did reconciles the
    /// copy it already shows. A message luxe composed (review feedback, a draft's brief) has no
    /// such copy anywhere, so it's announced the same way.
    pub(super) async fn commit_user_message(
        &self,
        user_message: &str,
        images: &[ImageAttachment],
        remediation: Option<&RemediationTurnContext>,
        context: Option<&str>,
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<EntryId, EngineError> {
        let sid = self.current_session_id().await;
        let existing = remediation
            .and_then(|context| context.submission_id.as_ref())
            .map(|submission_id| {
                self.sessions().load(&sid).map(|entries| {
                    entries.into_iter().find_map(|entry| {
                        matches!(&entry.kind, EntryKind::Message(message) if is_feedback_submission_message(message, submission_id))
                            .then_some(entry.id)
                    })
                })
            })
            .transpose()?
            .flatten();

        let entry_id = match existing {
            Some(entry_id) => entry_id,
            None => {
                // Chain fresh user input onto the current leaf so the whole conversation (not just
                // this message) reaches the provider.
                let parent = self.resolve_leaf().await?;
                let mut message = user_message_with(user_message, images);
                // In the transcript, never the system prompt: none of it moves a cached prefix
                message.content.extend(self.one_time_context().await);
                message.content.extend(context.and_then(hook_context_block));
                let entry_id =
                    self.sessions_mut().append(&sid, parent, EntryKind::Message(message))?;
                // Only what was sent: what a hook added rides along for the model, not the reader
                let _ = event_tx
                    .send(Event::MessageCommitted {
                        session_id: sid.to_string(),
                        entry_id: entry_id.clone(),
                        message: user_message_with(user_message, images),
                    })
                    .await;
                entry_id
            }
        };
        self.set_leaf(entry_id.clone()).await;
        Ok(entry_id)
    }

    /// Commits an assistant reply and everything that follows from it: the usage entry chained
    /// after the call it paid for, the `MessageCommitted` event, and the truncation warning.
    ///
    /// A call that produced nothing commits nothing — see [`ReplyOutcome::Nothing`] — but still
    /// flushes its usage, because a call that was billed is spend whatever came back.
    pub(super) async fn commit_assistant_reply(
        &self,
        leaf: EntryId,
        reply: StreamedReply,
        is_remediation: bool,
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<ReplyOutcome, EngineError> {
        let truncated_tool_args = reply.has_truncated_tool_args();
        let stop_reason = reply.stop_reason.clone();
        let tool_calls = reply.tool_calls.clone();
        // Only a remediation or a drafting turn has anything that reads the answer text back
        let text = (is_remediation
            || self.read_only == Some(crate::engine::read_only::ReadOnlyTurn::Drafting))
        .then(|| reply.text.clone());

        let blocks = reply.into_blocks();
        if blocks.is_empty() {
            return Ok(ReplyOutcome::Nothing { leaf: self.flush_usage(leaf).await });
        }
        let message = Message { role: Role::Assistant, content: blocks };
        let sid = self.current_session_id().await;
        let entry_id =
            self.sessions_mut().append(&sid, Some(leaf), EntryKind::Message(message.clone()))?;
        self.set_leaf(entry_id.clone()).await;
        // Write what this call (and anything a detached pass recorded meanwhile) cost, chained
        // after the reply it paid for, so a resumed session continues the same tally.
        let leaf = self.flush_usage(entry_id.clone()).await;
        let _ = event_tx
            .send(Event::MessageCommitted {
                session_id: sid.to_string(),
                entry_id: entry_id.clone(),
                message,
            })
            .await;

        if truncated_tool_args || matches!(stop_reason, Some(crate::provider::StopReason::Length)) {
            warn!(
                ?stop_reason,
                truncated_tool_args, "assistant response truncated; raise this model's max_tokens"
            );
        }
        Ok(ReplyOutcome::Committed(CommittedReply {
            leaf,
            entry_id,
            tool_calls,
            text,
            truncated_tool_args,
        }))
    }

    /// Reports a provider call that came back with nothing, and says why the turn stops.
    ///
    /// A cancel is the user's own doing and the UI has already said so; anything else is the
    /// model answering with silence, which looked exactly like luxe hanging — the spinner
    /// stopped, no message appeared, and the next thing the reader saw was a `400` from the turn
    /// after it.
    pub(super) async fn report_empty_reply(&self, event_tx: &mpsc::Sender<Event>) -> StopReason {
        if self.is_cancelled() {
            debug!("the provider stream was dropped by a cancel before it produced anything");
            return StopReason::Other("cancelled".to_string());
        }
        let used = self.last_context_tokens.load(std::sync::atomic::Ordering::Relaxed);
        let window = self.effective_model().await.context_window;
        warn!(used, window, "the provider returned an empty response; nothing was committed");
        let _ = event_tx.send(Event::Error(empty_reply_message(used, window))).await;
        StopReason::Other("empty response".to_string())
    }

    /// Answers every tool call with a refusal, ending a wrap-up iteration cleanly.
    ///
    /// The wrap-up call is made *without* tools; a model that conjured one anyway does not get to
    /// run it. Each call still needs a result block or the transcript stops being valid for
    /// providers that require one per `tool_use`.
    pub(super) async fn refuse_tool_calls(
        &self,
        leaf: EntryId,
        tool_calls: &[ToolCallInfo],
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<(), EngineError> {
        warn!(count = tool_calls.len(), "ignoring tool calls in the wrap-up response");
        let refusals = tool_calls
            .iter()
            .map(|call| ContentBlock::ToolResult {
                call_id: call.id.clone(),
                content: vec![crate::message::ToolOutput::Text(
                    "not executed: this turn reached its configured tool-call limit".to_string(),
                )],
                is_error: true,
            })
            .collect();
        self.commit_tool_message(leaf, Message { role: Role::Tool, content: refusals }, event_tx)
            .await?;
        Ok(())
    }

    /// Runs the batch's calls that have not run yet and commits every result as one Tool message.
    ///
    /// `ran` holds the results of the calls that already ran while the reply was arriving; the rest
    /// run now, in order. Every call gets a result block, cancelled ones included, and the commit is
    /// unconditional — the window between committing the calls and committing their results is
    /// exactly where an interrupted turn would strand them.
    pub(super) async fn dispatch_and_commit_tools(
        &self,
        leaf: EntryId,
        tool_calls: &[ToolCallInfo],
        ran: Vec<ContentBlock>,
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<EntryId, EngineError> {
        let mut results = Vec::with_capacity(tool_calls.len());
        for call in tool_calls {
            match result_for(&ran, &call.id) {
                Some(block) => results.push(block),
                None => results.push(self.dispatch_one(call, event_tx).await),
            }
        }
        self.attach_nested_instructions(&leaf, tool_calls, &mut results, event_tx).await;
        self.commit_tool_message(leaf, Message { role: Role::Tool, content: results }, event_tx)
            .await
    }

    /// Commits one result per call without running anything: `ran`'s where a call already ran,
    /// and never started for the rest.
    ///
    /// For a reply that cannot go on — cut off, cancelled, or failed — after some of its calls
    /// had already run. The ones that ran did something, and the transcript has to say what.
    pub(super) async fn commit_results(
        &self,
        leaf: EntryId,
        tool_calls: &[ToolCallInfo],
        ran: Vec<ContentBlock>,
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<EntryId, EngineError> {
        let results = tool_calls
            .iter()
            .map(|call| {
                result_for(&ran, &call.id)
                    .unwrap_or_else(|| super::super::dispatch::never_started(call))
            })
            .collect();
        self.commit_tool_message(leaf, Message { role: Role::Tool, content: results }, event_tx)
            .await
    }

    /// Appends a Tool message, announces it, and reports each call's outcome.
    async fn commit_tool_message(
        &self,
        leaf: EntryId,
        message: Message,
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<EntryId, EngineError> {
        let completed: Vec<(String, bool)> = message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolResult { call_id, is_error, .. } => {
                    Some((call_id.clone(), *is_error))
                }
                _ => None,
            })
            .collect();
        let sid = self.current_session_id().await;
        let entry_id =
            self.sessions_mut().append(&sid, Some(leaf), EntryKind::Message(message.clone()))?;
        self.set_leaf(entry_id.clone()).await;
        let _ = event_tx
            .send(Event::MessageCommitted {
                session_id: sid.to_string(),
                entry_id: entry_id.clone(),
                message,
            })
            .await;
        for (call_id, is_error) in completed {
            let _ = event_tx.send(Event::ToolEnded { call_id, is_error }).await;
        }
        Ok(entry_id)
    }
}

/// What an empty reply tells the reader. Near a full window the silence is most likely the model
/// having no room left to answer in, and retrying the same message fails the same way until
/// `/compact` makes room.
fn empty_reply_message(used: u32, window: u32) -> String {
    let nearly_full = window > 0 && u64::from(used) * 10 >= u64::from(window) * 9;
    if !nearly_full {
        return "the model returned an empty response — no text, no tool call. Nothing was added \
                to the conversation; send your message again to retry."
            .to_string();
    }
    let percent = (u64::from(used) * 100 / u64::from(window)).min(100);
    format!(
        "the model returned an empty response, most likely because the context window is full \
         ({percent}% used). Run /compact to summarize the conversation, then send your message \
         again."
    )
}

#[cfg(test)]
mod tests {
    use super::empty_reply_message;

    /// The silence at a full window looked like a retryable blip, and every retry failed alike.
    #[test]
    fn an_empty_reply_at_a_full_window_points_at_compact() {
        let message = empty_reply_message(195_000, 200_000);
        assert!(message.contains("/compact") && message.contains("97% used"), "{message}");
    }

    #[test]
    fn an_empty_reply_with_room_left_says_retry() {
        let message = empty_reply_message(100_000, 200_000);
        assert!(!message.contains("/compact") && message.contains("again to retry"), "{message}");
    }

    /// Before a first measurement, or for a model whose window is unknown, there is nothing to
    /// blame the silence on.
    #[test]
    fn an_unmeasured_window_is_not_called_full() {
        assert!(!empty_reply_message(0, 0).contains("/compact"));
        assert!(!empty_reply_message(5_000, 0).contains("/compact"));
    }
}
