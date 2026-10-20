//! Request — building one iteration's request and opening its stream.

use super::phases::frame_steer;
use super::*;

impl Engine {
    /// Builds one iteration's request from the active path, and picks the model to serve it.
    ///
    /// The transcript is rebuilt every iteration rather than appended to, so tool results from
    /// earlier iterations are visible on the next call. The model is resolved here too, not once
    /// per turn, so an in-session `SetModel` swap takes effect immediately.
    pub(super) async fn build_iteration_request(
        &self,
        leaf: &EntryId,
        system_prompt: Option<&str>,
        pass: IterationPass,
    ) -> Result<CompletionRequest, EngineError> {
        let sid = self.current_session_id().await;
        let (mut messages, activated) = self.outgoing_transcript(&sid, leaf)?;
        // Two shapes a transcript can reach here in that no provider will take, both of them a
        // hard 400 that replays on every later request — so they end the session rather than just
        // the turn. Fixed on this copy only, in the order that leaves the result valid: drop what
        // is empty first, then check that every call still has its answer.
        //
        // An empty block arrives from an image sent with no caption, a tool that returned no
        // output, or a session an older build left an empty assistant message in.
        let scrubbed = crate::empty_blocks::scrub(&mut messages);
        if !scrubbed.is_clean() {
            debug!(
                blocks_dropped = scrubbed.blocks_dropped,
                messages_dropped = scrubbed.messages_dropped,
                results_filled = scrubbed.results_filled,
                "dropped empty content from the outgoing transcript"
            );
        }
        // A tool call and its result are one unit on the wire, and the halves come apart easily —
        // an interrupted batch, a process that died between the commit and the dispatch, a
        // compaction cut through the middle of a pair.
        let paired = crate::tool_pairing::repair(&mut messages);
        if !paired.is_clean() {
            debug!(
                calls_answered = paired.calls_answered,
                results_dropped = paired.results_dropped,
                "repaired tool-call pairing in the outgoing transcript"
            );
        }
        if matches!(pass, IterationPass::Normal { steered: true }) {
            frame_steer(&mut messages);
        }
        let model = self.effective_model().await;
        let thinking = model.effective_thinking_level(*self.active_thinking.lock().await);
        let (tools, deferred_tools, system) =
            pass.tools_and_system(self, system_prompt, &activated).await;
        // The last line of defense before summarization: shrink stale tool-result bodies in the
        // outgoing copy (the durable session keeps the full text). Gated on how full the window
        // is, because eliding rewrites the middle of the transcript and re-bills everything after
        // it as a cache write — worth paying for runway, never worth paying for tidiness.
        let settings = &self.config.compaction_settings;
        let usable = crate::compaction::usable_context_window(
            u64::from(model.context_window),
            model.max_tokens,
            settings.reserve_tokens,
        );
        let (pressure, prompt_tokens) =
            crate::session::with_active_path(&**self.sessions(), &sid, leaf, |entries| {
                let system = system.as_deref();
                (
                    crate::compaction::ContextPressure::measure(entries, system, &tools, usable),
                    crate::compaction::prompt_tokens(entries, system, &tools),
                )
            })?;
        crate::compaction::prune_tool_outputs(&mut messages, settings, pressure);
        // What this request spends before a word of conversation. Recorded here because this is
        // the only place the system prompt and the resolved tool block exist together, and
        // compaction needs it to know how much window the kept tail is really sharing.
        self.record_prefix_tokens(crate::compaction::estimate_prefix_tokens(
            system.as_deref(),
            &tools,
        ));
        // Anthropic *requires* max_tokens, so the cap follows the resolved model — reduced to what
        // the window has left, since a ceiling that overlaps the prompt is a 400 on a strict
        // endpoint and a silent truncation everywhere else.
        let max_tokens = crate::compaction::clamp_max_tokens_to_context(
            model.context_window,
            prompt_tokens,
            model.max_tokens,
        );
        Ok(CompletionRequest {
            model,
            messages,
            tools,
            deferred_tools,
            thinking,
            max_tokens,
            reuse: crate::provider::PromptReuse::Conversation,
            // Only for the prompt it was measured on; a pass that framed another one gets none
            shared_system_len: system
                .as_deref()
                .zip(system_prompt)
                .filter(|(sent, built)| sent.starts_with(built))
                .map_or(0, |_| self.shared_prompt_len()),
            system,
        })
    }

    /// The conversation a request carries, and the deferred tools it has loaded.
    ///
    /// Borrows the path rather than cloning it: it holds every entry ever written, including the
    /// ones a compaction replaced (it is append-only, so the summary is a new entry rather than a
    /// deletion), and only what `context_messages` keeps after that cut is copied. Reasoning blocks
    /// stay verbatim; each provider adapter decides how to replay them (Anthropic must resend
    /// signed thinking during a tool-use turn; the OpenAI paths drop it).
    fn outgoing_transcript(
        &self,
        sid: &SessionId,
        leaf: &EntryId,
    ) -> Result<(Vec<Message>, std::collections::BTreeSet<String>), EngineError> {
        let read = crate::session::with_active_path(&**self.sessions(), sid, leaf, |entries| {
            let mut messages = crate::compaction::context_messages(entries);
            // A summary that replaced an `activate_tools` call would unload the tool it loaded on
            // an adapter that loads tools in place, so the outgoing copy restates it right after
            // the summary. Before the scrub and the pairing repair, which then see an ordinary pair.
            crate::tool_activation::restore_after_compaction(entries, &mut messages);
            (messages, crate::tool_activation::activated_in(entries))
        })?;
        Ok(read)
    }

    /// Opens the provider stream for one iteration, making room and trying once more when the
    /// prompt turns out not to fit.
    ///
    /// A context overflow is the one provider refusal the harness can actually fix. Every estimate
    /// on the way in is an estimate — char/4 for anything not yet measured, a tokenizer we do not
    /// have for the rest — so a prompt can clear the threshold and still be rejected, and a
    /// mid-turn tool result large enough to do it arrives *after* the last check. Failing there
    /// ends the turn on the one error whose remedy Luxe already implements.
    ///
    /// The retry summarizes on the provider's authority rather than the threshold's: the window is
    /// full because the endpoint says so, whatever the local arithmetic believed. Once only, and
    /// only when compaction actually moved the leaf — retrying an identical request is how a
    /// recoverable error becomes an expensive loop.
    pub(super) async fn open_iteration_stream(
        &self,
        leaf: &mut EntryId,
        system_prompt: Option<&str>,
        pass: IterationPass,
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<
        (crate::provider::ModelInfo, futures::stream::BoxStream<'static, ProviderEvent>),
        EngineError,
    > {
        let request = self.build_iteration_request(leaf, system_prompt, pass).await?;
        let model = request.model.clone();
        let error = match self.stream_and_keep_warm(&model, request, event_tx).await {
            Ok(stream) => return Ok((model, stream)),
            Err(error) if error.kind() == crate::provider::ProviderErrorKind::Overflow => error,
            Err(error) => return Err(EngineError::Provider(error)),
        };

        warn!(%error, "the prompt did not fit; summarizing and trying once more");
        let _ = event_tx
            .send(Event::Notice(
                "The context window filled up mid-turn. Summarizing the conversation and \
                 retrying."
                    .to_string(),
            ))
            .await;

        let before = leaf.clone();
        // `Manual`, because the threshold has already been overruled by the endpoint.
        *leaf = self
            .compact_now(
                before.clone(),
                crate::compaction::CompactionTrigger::Manual,
                None,
                event_tx,
            )
            .await;
        // A cancel stopped the summary, and the turn ends the way any cancelled call does
        if self.is_cancelled() {
            return Ok((model, futures::stream::empty().boxed()));
        }
        if *leaf == before {
            warn!("nothing left to summarize; the prompt cannot be made to fit");
            return Err(EngineError::Provider(error));
        }

        let request = self.build_iteration_request(leaf, system_prompt, pass).await?;
        let model = request.model.clone();
        let stream = self
            .stream_and_keep_warm(&model, request, event_tx)
            .await
            .map_err(EngineError::Provider)?;
        Ok((model, stream))
    }

    /// Opens `request`'s stream, then hands the request to the cache warmer when the cache it
    /// writes can be kept alive — so a slow tool or a prompt waiting on the user doesn't cost the
    /// next request a full re-write.
    async fn stream_and_keep_warm(
        &self,
        model: &crate::provider::ModelInfo,
        request: CompletionRequest,
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<futures::stream::BoxStream<'static, ProviderEvent>, crate::provider::ProviderError>
    {
        let warmable =
            self.cache_warmer.enabled().then(|| self.provider_for(model)).and_then(|provider| {
                provider.prompt_cache_ttl(&request).map(|ttl| (provider, ttl, request.clone()))
            });
        // Before the send: an entry's lifetime runs from the start of the request that read it
        let sent_at = tokio::time::Instant::now();
        let stream = self.stream_with_retry(model, request, event_tx).await?;
        if let Some((provider, ttl, request)) = warmable {
            self.cache_warmer.start(
                super::super::cache_warmer::WarmRequest {
                    provider,
                    request,
                    ttl,
                    sent_at,
                    // the last call's size, until this one's usage arrives
                    estimated_prompt_tokens: u64::from(
                        self.last_context_tokens.load(std::sync::atomic::Ordering::Relaxed),
                    ),
                },
                super::super::cache_warmer::Accounting {
                    ledger: self.usage_ledger(),
                    sink: self.config.spend_sink.clone(),
                    events: event_tx.clone(),
                },
            );
        }
        Ok(stream)
    }
}
