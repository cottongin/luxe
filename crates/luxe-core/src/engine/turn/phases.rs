//! Turn phases — the steps `run_turn` walks, each nameable on its own.
//!
//! A turn is a prologue and then a loop: commit what the user said, let hooks weigh in, and then
//! repeatedly build a request, stream it, commit what came back, and run whatever tools it asked
//! for. Those steps are here so the loop that sequences them reads as the sequence, and so each
//! one can be reasoned about without holding the other five in mind.

use super::*;

/// What one finished provider call left the turn holding.
pub(super) enum ReplyOutcome {
    /// The model produced something, and it is now in the transcript.
    Committed(CommittedReply),
    /// Not one block came back, so nothing was committed.
    ///
    /// Either the stream was dropped before it yielded anything (a cancel already in effect) or
    /// the model genuinely answered with nothing. Both leave the turn with nothing to say and
    /// nothing to run, and writing a placeholder message for them is what ends a session: an
    /// empty text block is a `400` that replays on every later request.
    Nothing {
        /// The leaf, unchanged except for any usage the call still billed.
        leaf: EntryId,
    },
}

/// What one committed assistant reply left the turn holding.
pub(super) struct CommittedReply {
    /// The new leaf, after the reply and any usage entry chained onto it.
    pub leaf: EntryId,
    /// The reply's own entry, which a remediation report is attributed to.
    pub entry_id: EntryId,
    /// The tool calls it made, empty when the model answered in prose.
    pub tool_calls: Vec<ToolCallInfo>,
    /// The answer text, kept only when something reads it back: a remediation report or a draft.
    pub text: Option<String>,
    /// Whether a tool call's arguments were cut off mid-JSON.
    pub truncated_tool_args: bool,
}

/// Tells the model, on this one outgoing copy, that the user turn it is reading is a mid-task steer.
///
/// The framing rides at the tail of the conversation, after the steer itself, and never in the
/// system prompt. The system prompt sits in front of every cached message, so changing it for one
/// call re-wrote the whole conversation as a cache write — about $2.50 per steer on a 400k Opus
/// session. Here the change is confined to the last turn, the next request finds the entry the call
/// before the steer wrote, and the durable transcript never holds the framing at all.
pub(super) fn frame_steer(messages: &mut Vec<Message>) {
    let framing = ContentBlock::Text(steer_instruction().to_string());
    match messages.last_mut() {
        Some(last) if last.role == Role::User => last.content.push(framing),
        _ => messages.push(Message { role: Role::User, content: vec![framing] }),
    }
}

/// Which kind of provider call an iteration is making.
///
/// The distinction decides two things at once — which tools the model is offered and what the
/// system prompt says about them — so they are chosen together rather than derived twice.
#[derive(Clone, Copy)]
pub(super) enum IterationPass {
    /// An ordinary pass: every tool is on the table.
    Normal {
        /// A steer landed while the last iteration ran, so this one call frames it as one
        steered: bool,
    },
    /// The final pass after the tool-call limit tripped: no tools, and an instruction saying why.
    WrapUp {
        /// Tool iterations already spent, quoted back to the model in the instruction.
        steps: usize,
    },
}

impl IterationPass {
    /// The tools this pass offers, the deferred ones it may load, and the system prompt that
    /// frames them.
    ///
    /// Args: `(engine, system_prompt, activated)`, where `activated` is what the session's
    /// transcript has loaded.
    pub(super) async fn tools_and_system(
        &self,
        engine: &Engine,
        system_prompt: Option<&str>,
        activated: &std::collections::BTreeSet<String>,
    ) -> (Vec<crate::provider::ToolSchema>, Vec<crate::provider::ToolSchema>, Option<String>) {
        let registry = &engine.config.tool_registry;
        let (instruction, tools, deferred) = match self {
            // Withholding the tools entirely is the only reliable way to make a model stop
            // calling them; saying why makes the answer a hand-off rather than a refusal.
            Self::WrapUp { steps } => (Some(wrap_up_instruction(*steps)), Vec::new(), Vec::new()),
            // A steer's framing is in the conversation (`frame_steer`), where it costs one turn
            // of cache rather than all of it
            Self::Normal { .. } => {
                (None, registry.request_schemas(activated), registry.deferred_schemas())
            }
        };
        let system = match (system_prompt, instruction) {
            (Some(prompt), Some(instruction)) => Some(format!("{prompt}\n\n{instruction}")),
            (Some(prompt), None) => Some(prompt.to_string()),
            (None, instruction) => instruction,
        };
        (tools, deferred, system)
    }
}

/// The result `ran` holds for call `id`, if that call already ran.
pub(super) fn result_for(ran: &[ContentBlock], id: &str) -> Option<ContentBlock> {
    ran.iter()
        .find(|block| matches!(block, ContentBlock::ToolResult { call_id, .. } if call_id == id))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::super::commit::user_message_with;
    use super::*;

    /// The framing joins the steer's own turn when that is the last one, and otherwise becomes a
    /// user turn of its own rather than being glued onto the agent's reply or a tool's result.
    #[test]
    fn a_steers_framing_lands_on_a_user_turn_and_nowhere_else() {
        let framing = || ContentBlock::Text(steer_instruction().to_string());

        let mut steered = vec![Message::user("start"), Message::user("also check the tests")];
        frame_steer(&mut steered);
        assert_eq!(steered.len(), 2, "no turn added");
        assert_eq!(steered[1].content.last(), Some(&framing()));

        let mut replied = vec![Message::user("start"), Message::assistant("on it")];
        frame_steer(&mut replied);
        assert_eq!(replied.len(), 3, "the reply is left as the model wrote it");
        assert_eq!(replied[1].content.len(), 1);
        assert_eq!(replied[2], Message { role: Role::User, content: vec![framing()] });
    }

    fn screenshot() -> ImageAttachment {
        ImageAttachment {
            data: "iVBOR".to_string(),
            mime: "image/png".to_string(),
            name: Some("shot.png".to_string()),
        }
    }

    /// Pasting a screenshot and pressing Enter is a whole question. What it must not carry is an
    /// empty text block, which Anthropic rejects on this request and every later one.
    #[test]
    fn an_attachment_with_no_caption_travels_alone() {
        let message = user_message_with("   ", &[screenshot()]);

        assert_eq!(message.role, Role::User);
        assert_eq!(message.content.len(), 1);
        assert!(matches!(message.content[0], ContentBlock::Image { .. }));
    }

    #[test]
    fn a_caption_still_leads_its_attachment() {
        let message = user_message_with("what is wrong here?", &[screenshot()]);

        assert!(
            matches!(&message.content[0], ContentBlock::Text(text) if text == "what is wrong here?")
        );
        assert!(matches!(message.content[1], ContentBlock::Image { .. }));
    }
}
