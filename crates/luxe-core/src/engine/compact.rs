//! Compaction — threshold check + structured summarization.

use super::*;
use crate::compaction::CompactionTrigger;

mod summarizer;

impl Engine {
    /// Run the compaction check over the active path ending at `leaf`, executing it if the
    /// threshold is crossed. Failures are announced and non-fatal.
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
        crate::session::with_active_path(&**store, &sid, leaf, |entries| {
            crate::compaction::context_tokens(entries)
        })
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
    /// for and cannot see happened is indistinguishable from one that failed, and a failure
    /// reported as "nothing to do" leaves no way to tell what went wrong.
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
        let compacted = self.try_compact(leaf, CompactionTrigger::Manual, focus, event_tx).await?;
        if compacted.is_none() && !self.is_cancelled() {
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
    /// from. A failure is announced and non-fatal: a turn is still worth taking.
    pub(super) async fn compact_now(
        &self,
        leaf: EntryId,
        trigger: CompactionTrigger,
        focus: Option<String>,
        event_tx: &mpsc::Sender<Event>,
    ) -> EntryId {
        match self.try_compact(leaf.clone(), trigger, focus, event_tx).await {
            Ok(Some(compacted)) => compacted,
            Ok(None) => leaf,
            Err(error) => {
                self.announce_compaction_failure(&error, event_tx).await;
                leaf
            }
        }
    }

    /// One compaction attempt: the new leaf when a summary was committed, `None` when there was
    /// nothing to do or a cancel stopped it, and the reason when it failed.
    async fn try_compact(
        &self,
        leaf: EntryId,
        trigger: CompactionTrigger,
        focus: Option<String>,
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<Option<EntryId>, EngineError> {
        let sid = self.current_session_id().await;
        let entries = self.sessions().active_path(&sid, &leaf)?;
        let Some(plan) = self.plan_compaction(&entries, trigger).await? else { return Ok(None) };
        let before = crate::compaction::context_tokens(&entries);
        let _ = event_tx.send(Event::CompactionStarted { tokens_before: plan.tokens_before }).await;
        let outcome = self.execute_compaction(leaf, &plan, focus, event_tx).await;
        // Paired with the start on every ending, cancel included: a cancel unwinds the summary
        // call to an error rather than dropping this future, so this line always runs
        if outcome.is_err() {
            let _ = event_tx.send(Event::CompactionStopped).await;
        }
        let compacted = match outcome {
            Ok(compacted) => compacted,
            // The reader stopped it; nothing failed, so the breaker does not hear of it
            Err(_) if self.is_cancelled() => return Ok(None),
            Err(e) => {
                self.compaction_failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return Err(e);
            }
        };
        self.weigh_compaction(before, &compacted).await;
        // The summary now stands for everything it replaced, so the occupancy the next threshold
        // check reads has to be re-measured rather than carried over: a stale figure compacts
        // again immediately, every turn.
        self.last_context_tokens.store(0, std::sync::atomic::Ordering::Relaxed);
        self.set_leaf(compacted.clone()).await;
        Ok(Some(compacted))
    }

    /// Where to cut `entries`, or `None` when there is nothing worth summarizing.
    async fn plan_compaction(
        &self,
        entries: &[crate::session::Entry],
        trigger: CompactionTrigger,
    ) -> Result<Option<crate::compaction::CompactionPlan>, EngineError> {
        let settings = &self.config.compaction_settings;
        let budget = self.context_budget().await;
        match crate::compaction::find_compaction_point(entries, settings, budget, trigger) {
            Ok(plan) => {
                debug!(
                    ?trigger,
                    tokens_before = plan.tokens_before,
                    messages_to_summarize = plan.messages_to_summarize.len(),
                    "compaction triggered"
                );
                Ok(Some(plan))
            }
            Err(crate::compaction::CompactionError::Cancelled) => {
                debug!("compaction cancelled or below threshold");
                Ok(None)
            }
            Err(e) => Err(EngineError::Compaction(format!("could not plan the cut: {e}"))),
        }
    }

    /// The window the next turn's model gives the planner, and what of it is already measured.
    ///
    /// The model the next turn will use, not the one that filled the window: switching to a
    /// million-token model mid-session has to raise the threshold, not keep compacting at the old
    /// one.
    async fn context_budget(&self) -> crate::compaction::ContextBudget {
        let observed = self.last_context_tokens.load(std::sync::atomic::Ordering::Relaxed);
        let model = self.effective_model().await;
        crate::compaction::ContextBudget {
            context_window: model.context_window as u64,
            max_tokens: model.max_tokens,
            observed_tokens: (observed > 0).then_some(observed as u64),
            prefix_tokens: u64::from(
                self.last_prefix_tokens.load(std::sync::atomic::Ordering::Relaxed),
            ),
        }
    }

    /// Reset the failure breaker after a compaction that freed real room, or count one that did not.
    ///
    /// A summary that freed almost nothing leaves occupancy over the line, so the next iteration
    /// asks again: a provider call per tool call for the rest of the turn. Counting it keeps the
    /// transcript (the summary is not *wrong*, only insufficient) while bounding how many times a
    /// session may pay to learn the same thing.
    async fn weigh_compaction(&self, before: u64, compacted: &EntryId) {
        let after = self.estimated_context_tokens(compacted).await;
        if crate::compaction::compaction_was_effective(before, after) {
            self.compaction_failures.store(0, std::sync::atomic::Ordering::Relaxed);
            return;
        }
        let failures =
            self.compaction_failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        warn!(
            before,
            after,
            failures,
            "compaction freed too little to be worth repeating; \
             raise the window or lower compaction.keep_recent"
        );
    }

    /// Tell the reader a compaction failed, and that automatic compaction has stopped when this one
    /// tripped the breaker: otherwise the first they hear of it is a window too full to answer in.
    async fn announce_compaction_failure(
        &self,
        error: &EngineError,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let failures = self.compaction_failures.load(std::sync::atomic::Ordering::Relaxed);
        warn!(%error, failures, "compaction failed");
        let reason = match error {
            EngineError::Compaction(reason) => reason.clone(),
            other => other.to_string(),
        };
        let mut notice = format!("Couldn't compact the conversation: {reason}.");
        if crate::compaction::compaction_circuit_open(failures) {
            warn!(failures, "auto-compaction disabled for this session after repeated failures");
            notice.push_str(
                " Automatic compaction is off for this session; run /compact to try again.",
            );
        }
        let _ = event_tx.send(Event::Notice(notice)).await;
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
        let tokens_after =
            crate::session::with_active_path(&**self.sessions(), &sid, &compacted, |path| {
                crate::compaction::context_tokens(path)
            })
            .unwrap_or_default();
        // What drafts carried of a conversation went with the transcript
        self.sent_digests.lock().await.forget();
        let _ = event_tx
            .send(Event::CompactionDone { tokens_before: plan.tokens_before, tokens_after })
            .await;

        debug!(%compacted, tokens_before = plan.tokens_before, tokens_after, "compaction executed");
        Ok(compacted)
    }
}
