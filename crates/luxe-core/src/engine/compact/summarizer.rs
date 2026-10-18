//! Summarizer — the one-off request a compaction sends, and reading its reply.
//!
//! `compact.rs` decides *when* and *where* to cut; this owns the conversation with the model that
//! writes the summary.

use super::*;

impl Engine {
    /// The one-off request that asks the summarizer for `input`, the provider to send it to, and
    /// the model its cost is priced against.
    pub(super) async fn summary_request(
        &self,
        input: &str,
        focus: Option<&str>,
    ) -> (
        Arc<dyn crate::provider::Provider>,
        crate::provider::ModelInfo,
        crate::provider::CompletionRequest,
    ) {
        // The `## Heading` shape is named because it is what the reply is parsed by: a model told
        // only to be "structured" may answer in JSON, and every section of it is then dropped as
        // preamble — which a 27B local model did, three times, for a summary that then replaced the
        // transcript with nothing. The last sentence is there because a smaller model will otherwise
        // file *these rules* under `Constraints & Preferences` as though they were the session's.
        let system_prompt =
            "You are a session compactor. Summarize the conversation segment below \
                             so the work can continue from the summary alone. Write about the \
                             session's own goal, constraints, progress (done, in progress, \
                             blocked), decisions, next steps, and anything that would be expensive \
                             to rediscover. Answer in Markdown using exactly these headings, one \
                             `## ` heading per section with prose under it and nothing before the \
                             first: `## Goal`, `## Constraints & Preferences`, `## Progress`, \
                             `## Key Decisions`, `## Next Steps`, `## Critical Context`, \
                             `## Files` (every file read or modified). These formatting rules are \
                             not part of the session: never describe them in the summary.";

        // The user's steer goes *before* the transcript, where an instruction still reads as one:
        // after fifty thousand characters of conversation it reads as more conversation.
        let user_message = match focus.map(str::trim).filter(|focus| !focus.is_empty()) {
            Some(focus) => format!(
                "Summarize the following conversation, paying particular attention to: {focus}\n\n{input}"
            ),
            None => format!("Summarize the following conversation:\n\n{input}"),
        };

        // Resolve the summarizer: the configured `summarizer` override when set, else the active
        // model. Route through the provider registry so a cross-provider summarizer works, falling
        // back to the default provider when the registry has no entry (tests/single-provider).
        let summarizer_model = crate::compaction::summarizer_model(
            self.config.compaction_settings.summarizer.as_ref(),
            &self.effective_model().await,
        );
        let provider = self.provider_for(&summarizer_model);

        let priced_model = summarizer_model.clone();
        let messages = vec![crate::message::Message::user(user_message)];
        // The one call whose prompt is a whole window by construction, so the summary's own
        // allowance has to come out of what is left rather than on top of it.
        let max_tokens = crate::compaction::summary_max_tokens(
            &summarizer_model,
            crate::compaction::estimate_message_tokens(&messages)
                + crate::compaction::estimate_prefix_tokens(Some(system_prompt), &[]),
        );
        let request = crate::provider::CompletionRequest {
            shared_system_len: 0,
            model: summarizer_model,
            messages,
            system: Some(system_prompt.to_string()),
            tools: vec![],
            deferred_tools: Vec::new(),
            thinking: crate::provider::ThinkingLevel::Off,
            max_tokens,
            reuse: crate::provider::PromptReuse::OneOff,
        };

        (provider, priced_model, request)
    }

    /// Sends the summarizer its request, unless a cancel lands first.
    pub(super) async fn open_summary(
        &self,
        provider: Arc<dyn Provider + Send + Sync>,
        request: crate::provider::CompletionRequest,
    ) -> Result<futures::stream::BoxStream<'static, crate::provider::ProviderEvent>, EngineError>
    {
        let mut cancel = self.cancel_tx.subscribe();
        tokio::select! {
            biased;
            _ = cancel.wait_for(|cancelled| *cancelled) => Err(cancelled()),
            opened = provider.stream(request, self.cancel_tx.subscribe()) => {
                opened.map_err(EngineError::Provider)
            }
        }
    }

    /// Read the summarizer's reply, billing its usage. Only a reply that finished counts: an
    /// error, a length stop or a stream that just ends leave a fragment, however complete it looks.
    pub(super) async fn collect_summary(
        &self,
        mut stream: futures::stream::BoxStream<'static, crate::provider::ProviderEvent>,
        priced_model: &crate::provider::ModelInfo,
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<String, EngineError> {
        use futures::StreamExt;
        let mut summary_output = String::new();
        let mut finished = None;
        // Raced rather than left to the provider, since one mid-request may not look at its own
        // cancel receiver until the next byte arrives
        let mut cancel = self.cancel_tx.subscribe();
        loop {
            let event = tokio::select! {
                biased;
                _ = cancel.wait_for(|cancelled| *cancelled) => return Err(cancelled()),
                event = stream.next() => event,
            };
            let Some(event) = event else { break };
            match event {
                crate::provider::ProviderEvent::TextDelta { delta, .. } => {
                    summary_output.push_str(&delta);
                }
                // the call nobody asked for, and often the most expensive one in a session
                crate::provider::ProviderEvent::Usage(usage) => {
                    self.account_call(
                        crate::usage::UsageSource::Compaction,
                        priced_model,
                        &usage,
                        event_tx,
                    )
                    .await;
                }
                crate::provider::ProviderEvent::Done { reason } => {
                    finished = Some(reason);
                    break;
                }
                crate::provider::ProviderEvent::Error { message, .. } => {
                    return Err(EngineError::Compaction(format!(
                        "the summarizer failed partway ({message}); the transcript stands"
                    )));
                }
                _ => {}
            }
        }
        match finished {
            None => Err(EngineError::Compaction(
                "the summarizer's reply ended before it finished; the transcript stands".into(),
            )),
            Some(
                reason @ (crate::provider::StopReason::Length
                | crate::provider::StopReason::ContentFilter),
            ) => Err(EngineError::Compaction(format!(
                "the summary stopped short ({reason:?}); the transcript stands"
            ))),
            Some(_) => Ok(summary_output),
        }
    }
}

/// The error a cancelled summary unwinds with; callers tell it apart by the cancel flag.
fn cancelled() -> EngineError {
    EngineError::Compaction("cancelled; the transcript stands".into())
}
