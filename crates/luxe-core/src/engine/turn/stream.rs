//! Provider stream consumption — one call's events into one committable reply.
//!
//! Sits between the provider adapter and the transcript: [`Engine::consume_stream`] drains a
//! [`ProviderEvent`] stream, forwarding what a reader should see *now* as `Event`s and gathering
//! what the session must keep into a [`StreamedReply`]. Everything the turn does afterwards —
//! committing the message, detecting truncation, dispatching tools — reads that value rather than
//! the stream, so the wire format stops mattering the moment this returns.
//!
//! # A call runs as soon as it is written
//!
//! A model writes a message's calls one after another. Running none of them until the last had
//! arrived would leave a stack of idle cards that then all run inside a second, which is very hard
//! to follow. So the reading and the running happen side by side: the moment a call's
//! arguments are complete it is handed to [`Engine::run_written`], which runs the handed calls one
//! at a time, in order, while the stream carries on. Card by card, the model writes a call, it runs,
//! and the next one is written.
//!
//! What runs early is narrow on purpose (see [`Handover`]): a call the model has closed, whose
//! arguments parse, that is not a lookup, and whose predecessors all ran early too. Lookups wait
//! for the committed message because their fold is declared by it, and the first call that waits
//! makes every later one wait, so order is never broken. Whatever did not run early runs after the
//! commit.

use super::*;
use crate::activity::{describe_partial_call, partial_card_body};

mod composing;
mod reply;

use composing::{composes, runs_away, Composed};
pub(crate) use reply::{BlockRef, StreamedReply};

/// What reading one provider call left behind: the reply, how the stream ended, and the calls
/// that already ran.
pub(crate) struct StreamOutcome {
    /// Everything that arrived, complete or not
    pub reply: StreamedReply,
    /// The provider's failure, when the stream ended on one rather than finishing
    pub error: Option<EngineError>,
    /// One result per call that ran while the reply was still arriving, in call order
    pub ran: Vec<ContentBlock>,
}

/// Which written calls have been handed over to run, and whether more may follow.
///
/// The model closes a call in one of three ways the adapters can see: an explicit end for its
/// block (Anthropic), a later block opening (the OpenAI shapes, which close nothing explicitly), or
/// the stream ending. Only the first two hand a call over here; at the end the turn commits and runs
/// the rest itself.
struct Handover {
    /// Where handed calls go; `None` when nothing may run early, as in a turn's wrap-up
    to_run: Option<mpsc::UnboundedSender<ToolCallInfo>>,
    /// How many of the reply's calls, from the first, have been handed over
    sent: usize,
    /// Cleared by the first call that has to wait, which makes every later one wait too
    open: bool,
    /// Stream indexes of the calls the model has finished writing
    closed: std::collections::HashSet<usize>,
}

impl Handover {
    fn new(to_run: Option<mpsc::UnboundedSender<ToolCallInfo>>) -> Self {
        Self { to_run, sent: 0, open: true, closed: Default::default() }
    }

    /// Every call so far is written in full, because something after them has begun.
    fn close_all(&mut self, reply: &StreamedReply) {
        self.closed.extend(reply.tool_calls.iter().map(|call| call.index));
    }

    /// Hands over every newly runnable call, in order. Returns whether anything moved.
    fn advance(&mut self, reply: &StreamedReply) -> bool {
        let Some(to_run) = self.to_run.as_ref() else { return false };
        let mut moved = false;
        while self.open {
            let Some(call) = reply.tool_calls.get(self.sent) else { break };
            if !self.closed.contains(&call.index) {
                break;
            }
            if !runs_early(call) || to_run.send(call.clone()).is_err() {
                self.open = false;
                break;
            }
            self.sent += 1;
            moved = true;
        }
        moved
    }
}

/// Whether a written call may run before its message commits.
///
/// Both halves of its identity, arguments that parse to an object, and not a lookup: three adjacent
/// lookups fold into one line, and the fold is declared by the committed message, so one run early
/// would draw the card the fold then has to take back. A call the model closed with no arguments at
/// all is a call that takes none — dispatch reads it as `{}` — not one still arriving.
fn runs_early(call: &ToolCallInfo) -> bool {
    let arguments = call.arguments.trim();
    !call.id.is_empty()
        && !call.name.is_empty()
        && !crate::activity::is_lookup(&call.name)
        && (arguments.is_empty()
            || serde_json::from_str::<serde_json::Value>(arguments)
                .is_ok_and(|arguments| arguments.is_object()))
}

/// Hands over every call the model has finished writing, as of `event`.
///
/// Anything opening after a call means the model has finished writing it, and so does its own
/// end. Handed over before the next card is announced, and with a yield so the call is running by
/// the time it is: otherwise the finished card flashes "queued" for a frame on its way from being
/// written to running.
async fn close_written_calls(
    event: &ProviderEvent,
    handover: &mut Handover,
    reply: &StreamedReply,
) {
    let closes = match event {
        ProviderEvent::ToolCallEnd { index } => {
            handover.closed.insert(*index);
            true
        }
        ProviderEvent::ToolCallStart { .. }
        | ProviderEvent::TextDelta { .. }
        | ProviderEvent::ThinkingDelta { .. } => {
            handover.close_all(reply);
            true
        }
        _ => false,
    };
    if closes && handover.advance(reply) {
        tokio::task::yield_now().await;
    }
}

/// Opens a card for a call the model has started writing, when it is one worth watching arrive.
///
/// A front-end opens the card here rather than at dispatch, so a call the model is still writing
/// is visible for the whole time it takes to arrive.
///
/// Args: `(composed, index, id, name, event_tx)`.
async fn announce_composing(
    composed: &mut HashMap<usize, Composed>,
    index: usize,
    id: &str,
    name: &str,
    event_tx: &mpsc::Sender<Event>,
) {
    if !composes(id, name) {
        return;
    }
    composed.insert(index, Composed::default());
    let _ = event_tx
        .send(Event::ToolCallComposing {
            call_id: id.to_string(),
            name: name.to_string(),
            subject: None,
            body: None,
        })
        .await;
}

/// Adds a fragment to its call's arguments, and tells its card when there is something new.
///
/// Breaks when the model is writing nothing but whitespace: dropping the stream is what stops the
/// billing. The call is closed as written, so dispatch answers its unfinished JSON and the model
/// sends it again, rather than the turn ending on a cut-off it never reached.
///
/// Args: `(reply, composed, index, json_delta, event_tx)`.
async fn accept_tool_delta(
    reply: &mut StreamedReply,
    composed: &mut HashMap<usize, Composed>,
    index: usize,
    json_delta: &str,
    event_tx: &mpsc::Sender<Event>,
) -> std::ops::ControlFlow<()> {
    let Some(call) = reply.tool_calls.iter_mut().find(|call| call.index == index) else {
        return std::ops::ControlFlow::Continue(());
    };
    call.arguments.push_str(json_delta);
    if json_delta.trim().is_empty() && runs_away(&call.arguments) {
        warn!(tool = %call.name, "the model is writing only whitespace; cutting the reply off");
        call.arguments.truncate(call.arguments.trim_end().len());
        reply.stop_reason = Some(crate::provider::StopReason::ToolCall);
        return std::ops::ControlFlow::Break(());
    }
    let Some(state) = composed.get_mut(&index) else {
        return std::ops::ControlFlow::Continue(());
    };
    if let Some(frame) = state.next_frame(std::time::Instant::now(), &call.name, &call.arguments) {
        let _ = event_tx
            .send(Event::ToolCallComposing {
                call_id: call.id.clone(),
                name: call.name.clone(),
                subject: frame.subject,
                body: frame.body,
            })
            .await;
    }
    std::ops::ControlFlow::Continue(())
}

impl Engine {
    /// Drains one provider stream, running each call as soon as it is written when `early` allows.
    ///
    /// `priced_model` is the model that actually served this call, not whatever the engine was
    /// configured with — otherwise a mid-session `SetModel` swap keeps reporting the old context
    /// window and the old prices. `early` is off for a turn's wrap-up, whose calls are refused.
    pub(super) async fn consume_stream(
        &self,
        stream: futures::stream::BoxStream<'static, ProviderEvent>,
        priced_model: &crate::provider::ModelInfo,
        early: bool,
        event_tx: &mpsc::Sender<Event>,
    ) -> StreamOutcome {
        let (to_run, written) = mpsc::unbounded_channel();
        let reading = self.read_stream(stream, priced_model, early.then_some(to_run), event_tx);
        let running = self.run_written(written, event_tx);
        let ((reply, error), ran) = tokio::join!(reading, running);
        StreamOutcome { reply, error, ran }
    }

    /// Runs the calls the reader hands over, one at a time and in order, until it stops handing.
    async fn run_written(
        &self,
        mut written: mpsc::UnboundedReceiver<ToolCallInfo>,
        event_tx: &mpsc::Sender<Event>,
    ) -> Vec<ContentBlock> {
        let mut ran = Vec::new();
        while let Some(call) = written.recv().await {
            ran.push(self.dispatch_one(&call, event_tx).await);
        }
        ran
    }

    /// Forwards live events and gathers the committable reply, handing each finished call over.
    ///
    /// A provider error ends the read with the reply as far as it got, because calls may already
    /// have run from it and the turn has to commit them.
    async fn read_stream(
        &self,
        mut stream: futures::stream::BoxStream<'static, ProviderEvent>,
        priced_model: &crate::provider::ModelInfo,
        to_run: Option<mpsc::UnboundedSender<ToolCallInfo>>,
        event_tx: &mpsc::Sender<Event>,
    ) -> (StreamedReply, Option<EngineError>) {
        let mut handover = Handover::new(to_run);
        let context_window = priced_model.context_window;
        let mut reply = StreamedReply {
            text: String::new(),
            thinking: HashMap::new(),
            order: Vec::new(),
            tool_calls: Vec::new(),
            stop_reason: None,
        };
        let mut text_started = false;
        // What each in-flight call's card has already been told, by stream index.
        let mut composed: HashMap<usize, Composed> = HashMap::new();

        while let Some(provider_event) = stream.next().await {
            close_written_calls(&provider_event, &mut handover, &reply).await;
            match provider_event {
                ProviderEvent::TextDelta { delta, .. } => {
                    if !text_started {
                        text_started = true;
                        reply.order.push(BlockRef::Text);
                    }
                    reply.text.push_str(&delta);
                    let _ = event_tx.send(Event::TokenDelta { block: 0, delta }).await;
                }
                ProviderEvent::ThinkingDelta { index, delta } => {
                    reply.note_thinking_block(index).text.push_str(&delta);
                    // The provider's own index, not zero: a message can carry several reasoning
                    // items, they commit as several blocks, and a front-end told they were all
                    // block 0 merges them into one — which then has to be torn down and rebuilt
                    // the moment the real shape arrives.
                    let _ = event_tx.send(Event::ThinkingDelta { block: index, delta }).await;
                }
                ProviderEvent::ReasoningOpaque { index, opaque } => {
                    // Copilot may split it across chunks, so it is gathered like the text
                    reply
                        .note_thinking_block(index)
                        .opaque
                        .get_or_insert_with(String::new)
                        .push_str(&opaque);
                }
                ProviderEvent::ThinkingSignature { index, signature } => {
                    reply.note_thinking_block(index).signature = Some(signature);
                }
                ProviderEvent::ReasoningItem { index, id, encrypted } => {
                    let accumulated = reply.note_thinking_block(index);
                    if id.is_some() {
                        accumulated.id = id;
                    }
                    if encrypted.is_some() {
                        accumulated.encrypted = encrypted;
                    }
                }
                ProviderEvent::RedactedThinking { index, data } => {
                    reply.note_thinking_block(index).redacted = Some(data);
                    let _ = event_tx
                        .send(Event::ThinkingDelta {
                            block: 0,
                            delta: "[redacted reasoning]".to_string(),
                        })
                        .await;
                }
                ProviderEvent::ToolCallStart { index, id, name } => {
                    reply.order.push(BlockRef::Tool(index));
                    announce_composing(&mut composed, index, &id, &name, event_tx).await;
                    reply.tool_calls.push(ToolCallInfo {
                        index,
                        id,
                        name,
                        arguments: String::new(),
                    });
                }
                ProviderEvent::ToolCallDelta { index, json_delta } => {
                    if accept_tool_delta(&mut reply, &mut composed, index, &json_delta, event_tx)
                        .await
                        .is_break()
                    {
                        break;
                    }
                }
                // Surface context-window fullness + token/cost to the UIs.
                ProviderEvent::Usage(usage) if context_window > 0 && usage.total_tokens > 0 => {
                    self.account_stream_usage(&usage, priced_model, event_tx).await;
                }
                ProviderEvent::Done { reason } => {
                    debug!(reason = ?reason, "provider done");
                    reply.stop_reason = Some(reason);
                    break;
                }
                // A subscription's remaining headroom: forwarded as-is, since the provider is the
                // only thing that knows it and nothing here can improve on it.
                ProviderEvent::PlanUsage(plan) => {
                    let _ = event_tx.send(Event::PlanUsage(plan)).await;
                }
                ProviderEvent::Error { message, kind } => {
                    error!(error = %message, "provider error");
                    return (reply, Some(ProviderError::reported(kind, message).into()));
                }
                _ => {}
            }
        }
        (reply, None)
    }

    /// Surfaces context-window fullness and the call's cost, and remembers the exact occupancy so
    /// the next turn's compaction check can use it instead of the char/4 estimate.
    async fn account_stream_usage(
        &self,
        usage: &crate::provider::Usage,
        priced_model: &crate::provider::ModelInfo,
        event_tx: &mpsc::Sender<Event>,
    ) {
        self.last_context_tokens.store(usage.total_tokens, std::sync::atomic::Ordering::Relaxed);
        self.cache_warmer.measured(u64::from(usage.prompt_tokens));
        let _ = event_tx
            .send(Event::ContextUsage {
                used_tokens: usage.total_tokens,
                context_window: priced_model.context_window,
            })
            .await;
        self.account_call(self.turn_source, priced_model, usage, event_tx).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Copied from a real gpt-6.1-sol reply that went on like this for 128k tokens.
    #[test]
    fn a_blank_run_with_line_breaks_runs_away_and_indentation_does_not() {
        let stuck = format!("{{\"subjects\":[\"a\"] \t\n \r\n{}", " \n".repeat(140));
        assert!(runs_away(&stuck));
        assert!(!runs_away(&stuck[..stuck.len() - 200]), "not yet that long");
        let pretty = format!("{{\n{}\"n\": 1\n}}", " ".repeat(40));
        assert!(!runs_away(&pretty));
        // Spaces alone can sit inside a string, where they may be the content
        let padded = format!("{{\"text\":\"{}", " ".repeat(1000));
        assert!(!runs_away(&padded));
    }

    fn call(index: usize, name: &str, arguments: &str) -> ToolCallInfo {
        ToolCallInfo {
            index,
            id: format!("call-{index}"),
            name: name.into(),
            arguments: arguments.into(),
        }
    }

    fn reply(calls: Vec<ToolCallInfo>) -> StreamedReply {
        StreamedReply {
            text: String::new(),
            thinking: HashMap::new(),
            order: Vec::new(),
            tool_calls: calls,
            stop_reason: None,
        }
    }

    /// Reasoning that is only an opaque token still commits, because it is replayed: Copilot's
    /// Gemini sends its state with no readable text at all.
    #[test]
    fn opaque_reasoning_commits_without_text() {
        let mut reply = reply(Vec::new());
        reply.note_thinking_block(0).opaque = Some("CpcBAb4+9vs".into());
        let blocks = reply.into_blocks();
        assert!(matches!(
            blocks.as_slice(),
            [ContentBlock::Thinking { text, opaque: Some(opaque), .. }]
                if text.is_empty() && opaque == "CpcBAb4+9vs"
        ));
    }

    /// A call is handed over once it is closed, in order; the first call that has to wait makes
    /// every later one wait, so nothing runs ahead of a call written before it.
    #[test]
    fn calls_are_handed_over_in_order_once_closed() {
        let (to_run, mut handed) = mpsc::unbounded_channel();
        let mut handover = Handover::new(Some(to_run));
        let mut reply = reply(vec![call(0, "edit", r#"{"path":"a"}"#)]);
        assert!(!handover.advance(&reply), "still being written");

        reply.tool_calls.push(call(1, "bash", r#"{"command":"#));
        handover.close_all(&reply);
        reply.tool_calls[1].arguments.push_str(r#""ls"}"#);
        assert!(handover.advance(&reply), "a later block closed both");
        assert_eq!(handed.try_recv().map(|call| call.index), Ok(0));
        assert_eq!(handed.try_recv().map(|call| call.index), Ok(1));

        reply.tool_calls.push(call(2, "read", r#"{"path":"b"}"#));
        reply.tool_calls.push(call(3, "test", ""));
        handover.close_all(&reply);
        assert!(!handover.advance(&reply), "a lookup waits for the message");
        assert!(handed.try_recv().is_err(), "and so does the call after it");
    }

    /// Only a call that is whole, named, and not a lookup runs early.
    #[test]
    fn what_may_run_early() {
        assert!(runs_early(&call(0, "edit", r#"{"path":"a"}"#)));
        assert!(runs_early(&call(0, "test", "")), "closed with no arguments takes none");
        assert!(!runs_early(&call(0, "edit", r#"{"path":"#)), "cut off mid-argument");
        assert!(!runs_early(&call(0, "edit", "[1]")), "arguments are an object");
        assert!(!runs_early(&call(0, "grep", r#"{"pattern":"x"}"#)), "a lookup folds");
        let mut nameless = call(0, "edit", "{}");
        nameless.id.clear();
        assert!(!runs_early(&nameless), "half an identity");
    }

    /// A wrap-up turn hands nothing over, however finished its calls are.
    #[test]
    fn nothing_is_handed_over_without_somewhere_to_run_it() {
        let mut handover = Handover::new(None);
        let reply = reply(vec![call(0, "edit", "{}")]);
        handover.close_all(&reply);
        assert!(!handover.advance(&reply));
    }

    /// A lookup never composes: three adjacent ones fold into a single line, declared by the
    /// committed message, so a card opened early is a row the fold then has to take back.
    #[test]
    fn only_a_call_worth_watching_arrive_opens_a_card_early() {
        for tool in ["write", "edit", "bash", "task", "screenshot"] {
            assert!(composes("call-1", tool), "{tool} should open its card early");
        }
        for tool in ["read", "grep", "glob", "symbol_search", crate::tool::ACTIVATE_TOOLS_NAME] {
            assert!(!composes("call-1", tool), "{tool} must wait for dispatch");
        }
        // Half an identity cannot be correlated with the call that follows it, so it keeps the old
        // behavior and gets its card at `ToolStarted`.
        assert!(!composes("", "write"));
        assert!(!composes("call-1", ""));
    }

    /// The subject is worth interrupting the cadence for — one small string, sent once, and it is
    /// what turns a bare spinner into a card. The payload is resent whole each time, so it waits
    /// its turn rather than spending the channel on frames nobody draws.
    #[test]
    fn the_payload_is_rate_limited_and_the_name_is_not() {
        let start = std::time::Instant::now();
        let mut state = Composed::default();
        let frame = |state: &mut Composed, after_ms: u64, arguments: &str| {
            state.next_frame(start + std::time::Duration::from_millis(after_ms), "write", arguments)
        };

        assert!(frame(&mut state, 0, "{\"path\":\"a.rs").is_none(), "nothing has closed yet");

        // The path closes: named at once, carrying whatever of the file has landed — there is no
        // cadence to respect until something has actually gone out.
        let named =
            frame(&mut state, 1, "{\"path\":\"a.rs\",\"content\":\"one\\n").expect("a frame");
        assert_eq!(named.subject.as_deref(), Some("a.rs"));
        assert_eq!(named.body.map(|body| body.total_lines), Some(1));

        // Inside the window the file keeps arriving and the channel stays quiet.
        assert!(frame(&mut state, 50, "{\"path\":\"a.rs\",\"content\":\"one\\ntwo\\n").is_none());

        let caught_up = frame(&mut state, 200, "{\"path\":\"a.rs\",\"content\":\"one\\ntwo\\n")
            .expect("the window has passed");
        assert!(caught_up.subject.is_none(), "the name cannot improve, so it is not resent");
        assert_eq!(caught_up.body.map(|body| body.total_lines), Some(2), "and it caught up");

        // A payload that has not grown is not a frame, however long it has been.
        assert!(frame(&mut state, 900, "{\"path\":\"a.rs\",\"content\":\"one\\ntwo\\n").is_none());
        let grown =
            frame(&mut state, 1_000, "{\"path\":\"a.rs\",\"content\":\"one\\ntwo\\nthree\\n")
                .expect("a third line");
        assert_eq!(grown.body.map(|body| body.total_lines), Some(3));
    }
}
