//! Turn engine — the inner agentic loop (Action → stream → tools → repeat).
//!
//! One `run_turn` call streams the provider, appends the assistant reply (text + tool
//! calls), runs the tools, feeds their results back, and repeats until the model answers
//! with no tool calls or `max_tool_iterations` trips.
//!
//! This file owns the *sequence*. Each step it sequences lives beside it: [`phases`] holds the
//! commit, hook and dispatch steps; [`stream`] turns one provider call into a committable reply.

use std::collections::HashMap;

use super::*;

mod admission;
mod commit;
mod phases;
mod report;
mod request;
mod retry;
mod stream;
mod tool_call;

use admission::PromptGate;
use phases::{IterationPass, ReplyOutcome};
use stream::StreamedReply;
pub(crate) use tool_call::{malformed_arguments, parse_tool_arguments, ToolCallInfo};

impl Engine {
    /// Run a single agent turn (user message → provider stream → tool dispatch → loop).
    ///
    /// Returns the leaf the turn ended on — `None` only when a hook refused the prompt in a
    /// session that had nothing in it yet, since a refused prompt commits nothing.
    pub(crate) async fn run_turn(
        &self,
        user_message: String,
        images: Vec<ImageAttachment>,
        injected_context: Option<String>,
        remediation: Option<RemediationTurnContext>,
        event_tx: mpsc::Sender<Event>,
    ) -> Result<Option<EntryId>, EngineError> {
        debug!(content = %user_message, "starting turn");
        // Before the message is committed, so a turn nobody can serve leaves no trace in the
        // transcript. Every turn comes through here, which is why the gate is here rather than in
        // each front-end's submit path — those ask first as a courtesy; this is what holds.
        if self.effective_model().await.r#ref.is_unset() {
            return Err(EngineError::NoModel);
        }
        // Fresh turn: clear any leftover cancellation from a previous Cancel. See
        // [`Engine::set_cancelled`] for why this cannot be a bare `send`.
        self.set_cancelled(false);

        let hook_cx = self.hook_context(&event_tx).await;
        // Before the commit, so a refused prompt leaves nothing behind. Committed first, a prompt a
        // policy hook refused for carrying a secret was in the session file and went to the
        // provider as history on the very next turn.
        let injected_context = match self
            .apply_prompt_hook(&user_message, injected_context, &hook_cx, &event_tx)
            .await
        {
            PromptGate::Proceed(context) => context,
            PromptGate::Blocked => return self.resolve_leaf().await,
        };
        let user_entry_id = self
            .commit_user_message(
                &user_message,
                &images,
                remediation.as_ref(),
                injected_context.as_deref(),
                &event_tx,
            )
            .await?;

        // Stable across the turn's inner iterations, unlike the tools and the model, which are
        // resolved per-iteration so a mid-turn `activate_tools` or `SetModel` takes effect at once.
        let system_prompt = Some(self.system_prompt().await?);
        let mut leaf = user_entry_id.clone();
        self.config
            .hook_registry
            .invoke(HookPoint::AgentStart, &HookEvent::Other(serde_json::Value::Null), &hook_cx)
            .await;

        let mut steps = 0usize;
        // Set once a configured iteration bound is reached: the next (final) provider call runs
        // without tools so the model closes out in prose instead of the turn being cut mid-flight.
        let mut wrapping_up = false;
        // Why this turn stopped, for the `TurnEnded` a front-end reads. Every `break` below sets it,
        // so a cancelled or truncated turn never reads as one that ended normally.
        let mut ended = StopReason::EndTurn;
        let mut report = report::ReportWatch::default();

        loop {
            // Checkpoint: anything the user steered in while the last iteration ran becomes a real
            // user message now, so the model sees it on this call rather than after the turn.
            let (next_leaf, steered_now) =
                self.deliver_pending_steers(leaf, &hook_cx, &event_tx).await?;

            // Every iteration, not just the first: a turn is where the window actually fills, and a
            // check outside the loop lets a long build run hundreds of calls past the threshold.
            // Safe here and nowhere else: the last thing committed is a complete message, so a cut
            // cannot land between a tool call and its result.
            leaf = self.maybe_compact(next_leaf, &event_tx).await;

            // Every way out of a cancelled turn, in one place: one that arrived mid-dispatch, one
            // that landed while this iteration was compacting or assembling its prompt. Steering
            // and compaction both stand down under a raised flag, so this is the first thing a
            // cancel changes. A request sent into a raised flag is billed and then dropped unread,
            // because the stream it opens is cancelled before its first event.
            if self.is_cancelled() {
                debug!("turn cancelled before the next call");
                ended = StopReason::Other("cancelled".to_string());
                break;
            }

            let pass = match wrapping_up {
                true => IterationPass::WrapUp { steps },
                false => IterationPass::Normal { steered: steered_now && steps > 0 },
            };
            let (priced_model, stream) = self
                .open_iteration_stream(&mut leaf, system_prompt.as_deref(), pass, &event_tx)
                .await?;
            // A wrap-up call is made without tools, and a call it conjures anyway is refused below
            // rather than run, so nothing may start early from it either.
            let streamed =
                self.consume_stream(stream, &priced_model, !wrapping_up, &event_tx).await;
            let ran = streamed.ran;
            if let Some(error) = streamed.error {
                if !ran.is_empty() {
                    let keeps_text = remediation.is_some();
                    self.commit_interrupted_reply(leaf, streamed.reply, ran, keeps_text, &event_tx)
                        .await?;
                }
                return Err(error);
            }
            let reply = streamed.reply;

            let committed = match self
                .commit_assistant_reply(leaf, reply, remediation.is_some(), &event_tx)
                .await?
            {
                ReplyOutcome::Committed(committed) => committed,
                // Nothing came back, so there is nothing to answer and nothing to run. A placeholder
                // would be an empty text block, which Anthropic rejects on every later request.
                ReplyOutcome::Nothing { leaf: unchanged } => {
                    leaf = unchanged;
                    ended = self.report_empty_reply(&event_tx).await;
                    break;
                }
            };
            leaf = committed.leaf.clone();
            if let (Some(true), Some(text)) =
                (remediation.as_ref().map(|context| context.reports), &committed.text)
            {
                report.heard(text);
            }

            if committed.truncated_tool_args {
                (leaf, ended) = self
                    .finish_truncated_reply(leaf, &committed, ran, &mut report, &event_tx)
                    .await?;
                break;
            }

            // No tool calls → the model produced its final answer for this turn.
            if committed.tool_calls.is_empty() {
                let context = remediation.as_ref();
                match self
                    .finish_terminal_reply(
                        leaf.clone(),
                        &committed,
                        context,
                        &mut report,
                        &event_tx,
                    )
                    .await?
                {
                    TerminalNext::Continue(next) => {
                        leaf = next;
                        continue;
                    }
                    TerminalNext::End => break,
                }
            }

            if wrapping_up {
                self.refuse_tool_calls(leaf.clone(), &committed.tool_calls, &event_tx).await?;
                break;
            }

            leaf =
                self.dispatch_and_commit_tools(leaf, &committed.tool_calls, ran, &event_tx).await?;

            steps += 1;
            if self.config.max_tool_iterations.is_some_and(|limit| steps >= limit) {
                warn!(steps, "tool-iteration limit reached; asking the model to wrap up");
                let _ = event_tx
                    .send(Event::Notice(format!(
                        "tool-call limit reached ({steps}); finishing this turn with a summary \
                         instead of more tool calls"
                    )))
                    .await;
                wrapping_up = true;
                // Set here rather than at the break below, because the ordinary way a wrapped-up
                // turn ends is the *normal* door: the tool-less call answers in prose and the
                // "no tool calls" branch takes it. Reporting that as `EndTurn` would say the model
                // finished when in fact it was stopped.
                ended = StopReason::Other("tool limit".to_string());
            }
        }

        if let Some(context) = remediation.as_ref() {
            self.note_unreported(context, &report);
        }
        let _ = event_tx.send(Event::TurnEnded { reason: ended }).await;
        Ok(Some(leaf))
    }
}

/// Where a turn goes after a reply that called no tools.
enum TerminalNext {
    /// Another iteration, from this leaf
    Continue(EntryId),
    /// The turn is over
    End,
}

impl Engine {
    /// Commits the calls that ran before the provider failed, with their results.
    ///
    /// Calls that already ran changed something, and a model that is never told so would redo
    /// it, or worse, reason from a tree it no longer has. The calls that never ran never happened.
    ///
    /// Args: `(leaf, reply, ran, keeps_text, event_tx)`.
    async fn commit_interrupted_reply(
        &self,
        leaf: EntryId,
        mut reply: StreamedReply,
        ran: Vec<ContentBlock>,
        keeps_text: bool,
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<(), EngineError> {
        reply.keep_only(&ran);
        if let ReplyOutcome::Committed(committed) =
            self.commit_assistant_reply(leaf, reply, keeps_text, event_tx).await?
        {
            self.commit_results(committed.leaf, &committed.tool_calls, ran, event_tx).await?;
        }
        Ok(())
    }

    /// Ends a turn whose reply was cut off mid-call, answering what ran before the cut.
    ///
    /// What ran is answered with what it did and the rest as never started, so the transcript
    /// holds every call with its result. Answers the leaf and why the turn stopped.
    async fn finish_truncated_reply(
        &self,
        mut leaf: EntryId,
        committed: &phases::CommittedReply,
        ran: Vec<ContentBlock>,
        report: &mut report::ReportWatch,
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<(EntryId, StopReason), EngineError> {
        if !ran.is_empty() {
            leaf = self.commit_results(leaf, &committed.tool_calls, ran, event_tx).await?;
        }
        // A cancel cuts the call being written as surely as a token limit does, and the reader
        // who pressed it does not need telling to raise one
        if self.is_cancelled() {
            return Ok((leaf, StopReason::Other("cancelled".to_string())));
        }
        let _ = event_tx
            .send(Event::Error(
                "response was truncated (increase this model's max_tokens in models.json); \
                 a tool call's arguments were cut off"
                    .to_string(),
            ))
            .await;
        report.cut_short("its reply was cut off before the report");
        Ok((leaf, StopReason::Other("truncated".to_string())))
    }

    /// Handles the model's final answer: keeps a draft's text, goes on for a steer that arrived
    /// while it streamed, and reads what a review turn said for the review.
    async fn finish_terminal_reply(
        &self,
        leaf: EntryId,
        committed: &phases::CommittedReply,
        remediation: Option<&RemediationTurnContext>,
        report: &mut report::ReportWatch,
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<TerminalNext, EngineError> {
        if self.read_only == Some(crate::engine::read_only::ReadOnlyTurn::Drafting) {
            *self.draft_answer.lock().await = committed.text.clone();
        }
        // The checkpoint that delivers a steer only runs at the top of an iteration, so ending here
        // would leave it buffered until the user's next submission — a message they watched Luxe
        // accept and then apparently ignore. Each continuation consumes the buffer, so this can
        // only extend the turn as often as the user actually types.
        if !self.pending_steers.lock().await.is_empty() {
            debug!("steering arrived during the final response; continuing the turn");
            return Ok(TerminalNext::Continue(leaf));
        }
        // The turn is ending, so what it said for the review is read now, and once
        match remediation {
            Some(context) if context.answers_only => {
                if let Some(text) = &committed.text {
                    self.answer_questions(context, text, event_tx).await;
                }
            }
            Some(context) if context.reports => {
                let next = self.read_report(context, &committed.entry_id, report, event_tx).await;
                if let report::ReportNext::Ask(reason) = next {
                    let leaf = self.ask_for_report(leaf, context, reason, report, event_tx).await?;
                    return Ok(TerminalNext::Continue(leaf));
                }
            }
            _ => {}
        }
        Ok(TerminalNext::End)
    }
}

fn is_feedback_submission_message(
    message: &Message,
    submission_id: &crate::guided_review::SubmissionId,
) -> bool {
    let marker = format!("Submission: {}", submission_id.as_str());
    message.role == Role::User
        && message.content.iter().any(|block| {
            matches!(block, ContentBlock::Text(text) if text.lines().any(|line| line == marker))
        })
}

/// The framing the one call after a mid-turn steer carries at the end of its last user turn.
///
/// A steer is an interruption, not a new assignment: the user typed it while watching the agent
/// work and expects that work to continue. Without this, the common failure is a model that answers
/// the interruption in prose — often promising to "continue now" — which ends the turn, because a
/// response with no tool calls *is* the end of a turn. Say so explicitly, and leave the decision to
/// stop where it belongs: with a steer that actually asked to stop.
fn steer_instruction() -> &'static str {
    "MID-TURN STEER: the user's latest message arrived while you were already working; the tool \
     results above are yours, from this same turn. Address it, then pick the interrupted work back \
     up in this turn rather than ending on an acknowledgement — a reply with no tool calls ends \
     your turn, so \"continuing now\" as your final sentence would strand the task. End the turn \
     only if the steer asked you to stop, asked a question that is genuinely all that is left, or \
     the work is actually finished."
}

/// The system-prompt addendum for a wrap-up call: the tools are gone this iteration, so say why
/// and ask for a hand-off the user (or the next turn) can act on.
fn wrap_up_instruction(steps: usize) -> String {
    format!(
        "TURN LIMIT REACHED: this turn has used its configured budget of {steps} tool-call \
         round-trips, so no tools are available on this response. Do not apologize and do not \
         attempt further tool calls. Write a short hand-off instead: what you did, what you \
         learned, what is still unfinished, and the exact next step to take. The user can send \
         another message to continue from there."
    )
}
