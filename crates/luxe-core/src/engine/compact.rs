//! Compaction — threshold check + structured summarization.

use super::*;
use crate::compaction::CompactionTrigger;

/// Output allowance for one summary. Structured prose over a whole window's worth of transcript;
/// the clamp below trims it further when the slice being summarized leaves less than this.
const SUMMARY_MAX_TOKENS: u32 = 4_096;

impl Engine {
    /// Run the compaction check over the active path ending at `leaf`, executing it if the
    /// threshold is crossed. Failures are logged and non-fatal.
    ///
    /// Returns the leaf the turn should continue from. A compaction that runs appends its summary
    /// after `leaf` and becomes the new leaf itself — the caller has to adopt it, or the summary
    /// sits on a twig the next request never walks and the whole conversation is sent anyway.
    ///
    /// Called once per tool iteration, not once per turn: an agentic turn is where a window
    /// actually fills, and one asked to build a feature can spend hundreds of calls growing the
    /// context it will overflow. Cheap when there is nothing to do — see [`Self::far_from_full`].
    pub(crate) async fn maybe_compact(
        &self,
        leaf: EntryId,
        event_tx: &mpsc::Sender<Event>,
    ) -> EntryId {
        // Under a cancel the turn is ending, and a summary would only be stopped partway
        if !self.config.compaction_settings.enabled || self.is_cancelled() {
            return leaf;
        }
        // Circuit breaker: stop auto-compacting once the summarizer has failed too many times in a
        // row, rather than burning a failing provider call every turn while stuck over threshold.
        if crate::compaction::compaction_circuit_open(
            self.compaction_failures.load(std::sync::atomic::Ordering::Relaxed),
        ) {
            return leaf;
        }
        if self.far_from_full().await {
            return leaf;
        }
        self.compact_now(leaf, CompactionTrigger::Threshold, None, event_tx).await
    }

    /// Record what the request being built spends before any conversation.
    pub(crate) fn record_prefix_tokens(&self, tokens: u64) {
        self.last_prefix_tokens
            .store(tokens.min(u64::from(u32::MAX)) as u32, std::sync::atomic::Ordering::Relaxed);
    }

    /// The char/4 estimate of what a request built from `leaf` would carry, conversation only.
    ///
    /// Only ever compared against another figure of the same shape, so the estimate's inaccuracy
    /// cancels; an unreadable path reports 0, which reads as "freed everything" and is the
    /// forgiving direction for a check whose failure mode is switching compaction off.
    async fn estimated_context_tokens(&self, leaf: &EntryId) -> u64 {
        let sid = self.current_session_id().await;
        let Ok(store) = self.config.session_store.read() else { return 0 };
        store
            .active_path(&sid, leaf)
            .map(|entries| crate::compaction::context_tokens(&entries))
            .unwrap_or(0)
    }

    /// Whether the last measured occupancy is comfortably under the threshold.
    ///
    /// Planning a cut means loading the whole active path, which on a long session is the
    /// transcript itself; asking every iteration would copy megabytes to be told "not yet". This
    /// answers from the figure the last call already reported, and only says yes when it is sure:
    /// nothing measured (a fresh session, or the reset a compaction just did) falls through to the
    /// real planner rather than skipping it.
    async fn far_from_full(&self) -> bool {
        let observed = self.last_context_tokens.load(std::sync::atomic::Ordering::Relaxed);
        if observed == 0 {
            return false;
        }
        let model = self.effective_model().await;
        let usable = crate::compaction::usable_context_window(
            u64::from(model.context_window),
            model.max_tokens,
            self.config.compaction_settings.reserve_tokens,
        );
        crate::compaction::compaction_threshold(usable, &self.config.compaction_settings)
            .is_some_and(|threshold| u64::from(observed) < threshold)
    }

    /// Compact the active path on request, whatever the occupancy — `/compact`.
    ///
    /// `focus` is the user's optional steer for the summary ("keep the API decisions"), passed
    /// through to the summarizer. Announces the outcome either way: a compaction the user asked
    /// for and cannot see happened is indistinguishable from one that failed.
    pub(crate) async fn compact_on_request(
        &self,
        focus: Option<String>,
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<(), EngineError> {
        // A summary is written by a model, so this asks for one as much as a turn does.
        if self.effective_model().await.r#ref.is_unset() {
            return Err(EngineError::NoModel);
        }
        // Serviced like a turn, so it starts from a lowered flag the same way
        self.set_cancelled(false);
        let Some(leaf) = self.resolve_leaf().await? else {
            let _ = event_tx
                .send(Event::Error("Nothing to compact — this session is empty.".to_string()))
                .await;
            return Ok(());
        };
        let compacted =
            self.compact_now(leaf.clone(), CompactionTrigger::Manual, focus, event_tx).await;
        if compacted == leaf && !self.is_cancelled() {
            let _ = event_tx
                .send(Event::Error(
                    "Nothing to compact — the conversation already fits within the recent-context \
                     budget."
                        .to_string(),
                ))
                .await;
        }
        Ok(())
    }

    /// Plan and run one compaction over the path ending at `leaf`, returning the leaf to continue
    /// from. Failures are logged, announced, and non-fatal — a turn is still worth taking.
    pub(super) async fn compact_now(
        &self,
        leaf: EntryId,
        trigger: CompactionTrigger,
        focus: Option<String>,
        event_tx: &mpsc::Sender<Event>,
    ) -> EntryId {
        let sid = self.current_session_id().await;
        let entries = match self.sessions().active_path(&sid, &leaf) {
            Ok(e) => e,
            Err(e) => {
                warn!(error = %e, "compaction: failed to load active path");
                return leaf;
            }
        };
        let observed = self.last_context_tokens.load(std::sync::atomic::Ordering::Relaxed);
        // The budget belongs to the model the next turn will use: switching to a million-token
        // model mid-session has to raise the threshold, not keep compacting at the old one.
        let model = self.effective_model().await;
        let budget = crate::compaction::ContextBudget {
            context_window: model.context_window as u64,
            max_tokens: model.max_tokens,
            observed_tokens: (observed > 0).then_some(observed as u64),
            prefix_tokens: u64::from(
                self.last_prefix_tokens.load(std::sync::atomic::Ordering::Relaxed),
            ),
        };
        match crate::compaction::find_compaction_point(
            &entries,
            &self.config.compaction_settings,
            budget,
            trigger,
        ) {
            Ok(plan) => {
                debug!(
                    ?trigger,
                    tokens_before = plan.tokens_before,
                    messages_to_summarize = plan.messages_to_summarize.len(),
                    "compaction triggered"
                );
                let before = crate::compaction::context_tokens(&entries);
                match self.execute_compaction(leaf.clone(), &plan, focus, event_tx).await {
                    Ok(compacted) => {
                        // A summary that freed almost nothing leaves occupancy over the line, so
                        // the next iteration asks again — a provider call per tool call for the
                        // rest of the turn. Counting it against the breaker keeps the transcript
                        // (the summary is not *wrong*, only insufficient) while bounding how many
                        // times a session may pay to learn the same thing.
                        let after = self.estimated_context_tokens(&compacted).await;
                        if crate::compaction::compaction_was_effective(before, after) {
                            self.compaction_failures.store(0, std::sync::atomic::Ordering::Relaxed);
                        } else {
                            let failures = self
                                .compaction_failures
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                                + 1;
                            warn!(
                                before,
                                after,
                                failures,
                                "compaction freed too little to be worth repeating; \
                                 raise the window or lower compaction.keep_recent"
                            );
                        }
                        // The summary now stands for everything it replaced, so the occupancy the
                        // next threshold check reads has to be re-measured rather than carried
                        // over — a stale figure compacts again immediately, every turn.
                        self.last_context_tokens.store(0, std::sync::atomic::Ordering::Relaxed);
                        self.set_leaf(compacted.clone()).await;
                        return compacted;
                    }
                    // The reader stopped it; nothing failed, so the breaker does not hear of it
                    Err(_) if self.is_cancelled() => debug!("compaction cancelled"),
                    Err(e) => {
                        let failures = self
                            .compaction_failures
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                            + 1;
                        warn!(error = %e, failures, "compaction execution failed");
                        if crate::compaction::compaction_circuit_open(failures) {
                            warn!(
                                failures,
                                "auto-compaction disabled for this session after repeated failures"
                            );
                        }
                    }
                }
            }
            Err(crate::compaction::CompactionError::Cancelled) => {
                debug!("compaction cancelled or below threshold");
            }
            Err(e) => warn!(error = %e, "compaction check failed"),
        }
        leaf
    }

    /// The one-off request that asks the summarizer for `input`, the provider to send it to, and
    /// the model its cost is priced against.
    async fn summary_request(
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
        let max_tokens = crate::compaction::clamp_max_tokens_to_context(
            summarizer_model.context_window,
            crate::compaction::estimate_message_tokens(&messages)
                + crate::compaction::estimate_prefix_tokens(Some(system_prompt), &[]),
            SUMMARY_MAX_TOKENS,
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

    /// Execute compaction: call the summarizer, format the summary, append it after `leaf`.
    ///
    /// Returns the appended entry's id, which is the session's new leaf.
    ///
    /// Fires `BeforeCompact`/`AfterCompact` hooks around the work (observe-only — e.g. a command
    /// hook can snapshot the transcript or log) so users get parity with Claude Code's
    /// pre/post-compact hooks.
    async fn execute_compaction(
        &self,
        leaf: EntryId,
        plan: &crate::compaction::CompactionPlan,
        focus: Option<String>,
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<EntryId, EngineError> {
        let hook_cx = self.hook_context(event_tx).await;
        self.config
            .hook_registry
            .invoke(
                HookPoint::BeforeCompact,
                &HookEvent::Other(serde_json::json!({ "tokens_before": plan.tokens_before })),
                &hook_cx,
            )
            .await;

        let input = crate::compaction::serialize_for_summary(&plan.messages_to_summarize);
        // The planner already refuses a span with no conversation in it; this is the same question
        // asked of the text that would actually be sent, because the one thing worse than an empty
        // summary is paying for one.
        if input.trim().is_empty() {
            return Err(EngineError::Compaction(
                "the span to summarize carries no conversation".into(),
            ));
        }

        let (provider, priced_model, request) =
            self.summary_request(&input, focus.as_deref()).await;
        let stream = self.open_summary(provider, request).await?;
        let summary_output = self.collect_summary(stream, &priced_model, event_tx).await?;

        let formatted_summary =
            crate::compaction::format_structured_summary(&summary_output, &plan.file_ops);
        // A summary that says nothing must not be allowed to stand for what it replaces. The
        // headings and the file list are ours, not the summarizer's, so an empty reply still
        // formats into a full-looking document — and committing it deletes the session's memory
        // while reporting success. Refusing here leaves the transcript intact and puts the attempt
        // on the failure breaker, which is the same treatment a provider error gets.
        if !crate::compaction::summary_carries_content(&formatted_summary) {
            return Err(EngineError::Compaction(format!(
                "the summarizer returned nothing usable ({} characters); the transcript stands",
                summary_output.trim().len()
            )));
        }

        // Append the Compaction entry to the session. Scoped so the (non-Send) write guard is
        // released before the AfterCompact await below.
        let sid = self.current_session_id().await;
        let compacted = {
            let mut store = self.config.session_store.write().map_err(|e| {
                EngineError::Session(crate::session::SessionError::Io(std::io::Error::other(
                    e.to_string(),
                )))
            })?;
            store.compact(
                &sid,
                Some(leaf),
                &formatted_summary,
                &plan.first_kept,
                plan.tokens_before,
            )?
        };

        self.config
            .hook_registry
            .invoke(
                HookPoint::AfterCompact,
                &HookEvent::Other(serde_json::json!({ "tokens_before": plan.tokens_before })),
                &hook_cx,
            )
            .await;

        // What the cut actually bought, measured over the path it produced rather than predicted.
        // A client watching for this is watching for the one thing compaction is for.
        let tokens_after = self
            .sessions()
            .active_path(&sid, &compacted)
            .map(|path| crate::compaction::context_tokens(&path))
            .unwrap_or_default();
        // What drafts carried of a conversation went with the transcript
        self.sent_digests.lock().await.forget();
        let _ = event_tx
            .send(Event::CompactionDone { tokens_before: plan.tokens_before, tokens_after })
            .await;

        debug!(%compacted, tokens_before = plan.tokens_before, tokens_after, "compaction executed");
        Ok(compacted)
    }

    /// Sends the summarizer its request, unless a cancel lands first.
    async fn open_summary(
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
    async fn collect_summary(
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
