//! Usage accounting for the engine — the one path every provider call reports through.
//!
//! Three places in the engine open a provider stream: the turn loop, the review's guide pass, and
//! the compaction summarizer. All three go through [`account_provider_call`], which makes "was this
//! call accounted for?" a question with one answer and gives the ledger the model that actually
//! served the call, so an in-session model switch is priced correctly.
//!
//! Recording and persisting are deliberately decoupled. The guide pass runs detached and
//! concurrent, with no route back to the session store, so it records into the ledger (the UI
//! moves immediately) and leaves the write to [`Engine::flush_usage`], which whoever next holds
//! the store calls. The ledger is therefore always current; the transcript catches up.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::event::Event;
use crate::provider::{ModelInfo, Usage};
use crate::session::latest_leaf;
use crate::session::{EntryId, EntryKind, SessionId};
use crate::usage::{
    PendingUsage, SessionUsage, SourceUsage, UsageLedger, UsageSource, UsageTotals,
};

use super::Engine;

/// Records one provider call and publishes both the call and the new session total.
///
/// Free-standing because the guide pass runs its batches from an associated function with no
/// `&self` to reach the engine through.
pub(crate) async fn account_provider_call(
    ledger: &UsageLedger,
    sink: Option<&Arc<dyn crate::usage::SpendSink>>,
    source: UsageSource,
    model: &ModelInfo,
    usage: &Usage,
    event_tx: &mpsc::Sender<Event>,
) {
    // The model says whether its rates are a charge or a plan equivalent; the ledger keeps them in
    // separate columns so a subscription turn never reports spend the user didn't incur.
    let recorded = ledger.record_billed(source, &model.r#ref.id, usage, &model.cost, model.billing);
    if let Some(sink) = sink {
        // The ledger's own figures rather than the event's: only here are a real charge and a
        // subscription's plan equivalent still in separate columns.
        let mut call = UsageTotals::default();
        call.add_billed_call(usage, recorded.cost_usd, model.billing);
        sink.record(&model.r#ref.id, source, &call).await;
    }
    if ledger.is_retired() {
        // Spent for a session switched away from: it settles there, and the open one's readout
        // is not this call's to move
        return;
    }
    let _ = event_tx
        .send(Event::TurnUsage {
            prompt_tokens: u64::from(usage.prompt_tokens),
            completion_tokens: u64::from(usage.completion_tokens),
            cost_usd: recorded.cost_usd,
            cache_read_tokens: u64::from(usage.cache_read_tokens),
            cache_write_tokens: u64::from(usage.cache_write_tokens),
            reasoning_tokens: u64::from(usage.reasoning_tokens),
            source,
        })
        .await;
    let _ = event_tx.send(Event::SessionUsage(recorded.session)).await;
}

/// The open session's ledger, and those of sessions switched away from while detached work still
/// held them.
///
/// A switch hands the new session a fresh ledger instead of resetting the old one in place: a
/// guide pass still writing to the old one would otherwise bill its late spend to the new session,
/// live and on disk.
#[derive(Default)]
pub(crate) struct SessionLedgers {
    active: std::sync::Mutex<Arc<UsageLedger>>,
    retired: std::sync::Mutex<Vec<(SessionId, Arc<UsageLedger>)>>,
}

impl SessionLedgers {
    fn active(&self) -> Arc<UsageLedger> {
        Arc::clone(&self.active.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    /// Retires the open ledger as `session`'s and starts a fresh one.
    fn switch_from(&self, session: SessionId) {
        let fresh = Arc::new(UsageLedger::new());
        let old = std::mem::replace(
            &mut *self.active.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
            fresh,
        );
        old.retire();
        self.retired().push((session, old));
    }

    fn retired(&self) -> std::sync::MutexGuard<'_, Vec<(SessionId, Arc<UsageLedger>)>> {
        self.retired.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// What retired ledgers have recorded since they were last settled, by session; a ledger
    /// nothing else held before it was drained is let go.
    ///
    /// Judged before draining, under the lock: a ledger unheld then can take no more spend, while
    /// one still held may record a call between its drain and the check.
    fn take_retired_pending(&self) -> Vec<(SessionId, Vec<PendingUsage>)> {
        let mut retired = self.retired();
        let unheld: Vec<bool> =
            retired.iter().map(|(_, ledger)| Arc::strong_count(ledger) == 1).collect();
        let drained = retired
            .iter()
            .map(|(session, ledger)| (session.clone(), ledger.take_pending()))
            .filter(|(_, pending)| !pending.is_empty())
            .collect();
        let mut unheld = unheld.into_iter();
        retired.retain(|_| !unheld.next().unwrap_or(false));
        drained
    }
}

impl Engine {
    /// Records one provider call against the session ledger and publishes the result.
    ///
    /// Emits the call's own figures ([`Event::TurnUsage`]) and the session's running total
    /// ([`Event::SessionUsage`]) so a UI can show both without doing arithmetic of its own.
    pub(crate) async fn account_call(
        &self,
        source: UsageSource,
        model: &ModelInfo,
        usage: &Usage,
        event_tx: &mpsc::Sender<Event>,
    ) {
        account_provider_call(
            &self.usage_ledger.active(),
            self.config.spend_sink.as_ref(),
            source,
            model,
            usage,
            event_tx,
        )
        .await;
    }

    /// The ledger, for a pass whose calls happen away from the engine's `&self`.
    pub(crate) fn usage_ledger(&self) -> Arc<UsageLedger> {
        self.usage_ledger.active()
    }

    /// Writes any recorded-but-unwritten spend to the transcript, chained after `parent`.
    ///
    /// Chained rather than free-floating on purpose: an entry with no parent is its own root, and
    /// a resume that landed on it would see a conversation of one line. Usage entries are inert
    /// for context assembly — only messages reach the model.
    ///
    /// Returns the new leaf, or `parent` unchanged when there is nothing to write or the write
    /// fails: a lost cost record must never cost the user their turn.
    pub(crate) async fn flush_usage(&self, parent: EntryId) -> EntryId {
        let pending = self.usage_ledger().take_pending();
        if pending.is_empty() {
            return parent;
        }
        let sid = self.current_session_id().await;
        let leaf = self.append_pending(&sid, pending, parent);
        self.set_leaf(leaf.clone()).await;
        leaf
    }

    /// Chains `pending` onto `session` after `parent`, returning the last entry written.
    fn append_pending(
        &self,
        session: &SessionId,
        pending: Vec<PendingUsage>,
        parent: EntryId,
    ) -> EntryId {
        let mut leaf = parent;
        for record in pending {
            if let Some(id) = self.append_usage_entry(
                session,
                record.source,
                &record.model,
                record.totals,
                leaf.clone(),
            ) {
                leaf = id;
            }
        }
        leaf
    }

    /// Hands the open ledger to `previous` as the engine switches away from it.
    pub(crate) fn retire_session_ledger(&self, previous: SessionId) {
        self.usage_ledger.switch_from(previous);
    }

    /// Writes what work for an earlier session spent after the switch, into that session.
    ///
    /// Chained after its latest entry, which is where a resume of it picks up. A session that is
    /// open again takes the spend into its own ledger instead, so its readout counts it and the
    /// flush that follows chains it where the conversation goes on. Returns whether the open
    /// session took any back.
    async fn settle_retired_usage(&self) -> bool {
        let open = self.current_session_id().await;
        let mut absorbed = false;
        for (session, pending) in self.usage_ledger.take_retired_pending() {
            if session == open {
                self.usage_ledger().absorb(pending);
                absorbed = true;
                continue;
            }
            // Bound first, so the read guard is gone before the write below takes the lock
            let latest = self.sessions().load(&session).map(|entries| latest_leaf(&entries));
            match latest {
                Ok(Some(parent)) => {
                    self.append_pending(&session, pending, parent);
                }
                Ok(None) => {
                    tracing::warn!(%session, "an earlier session's late spend has no entry to follow")
                }
                Err(error) => {
                    tracing::warn!(%session, %error, "an earlier session's late spend was not persisted")
                }
            }
        }
        absorbed
    }

    /// Flushes pending spend without a known leaf — for work that happens between turns, like a
    /// review's guide pass, whose cost still belongs to the session.
    ///
    /// Spend an earlier pass recorded for the open session is announced here, since no call of
    /// this session's did.
    pub(crate) async fn flush_usage_at_current_leaf(&self, event_tx: &mpsc::Sender<Event>) {
        if self.settle_retired_usage().await {
            let _ = event_tx.send(Event::SessionUsage(self.usage_snapshot())).await;
        }
        if !self.usage_ledger().has_pending() {
            return;
        }
        let Ok(Some(parent)) = self.resolve_leaf().await else {
            tracing::warn!("no session leaf; pending usage not persisted yet");
            return;
        };
        self.flush_usage(parent).await;
    }

    /// Sums the usage a session already recorded, so resuming continues its tally.
    ///
    /// Reads **every** entry rather than the active path: money spent on a branch that was later
    /// abandoned was still spent, and a cost readout that forgets it is wrong in the direction
    /// that matters.
    pub(crate) fn restore_session_usage(&self, session: &SessionId) {
        let ledger = self.usage_ledger();
        ledger.reset();
        let Ok(store) = self.config.session_store.read() else {
            tracing::warn!("session store lock poisoned; session usage not restored");
            return;
        };
        let Ok(entries) = store.load(session) else {
            return;
        };
        let mut restored = SessionUsage::default();
        for entry in &entries {
            match &entry.kind {
                EntryKind::Usage { source, totals, .. } => {
                    merge_source(&mut restored, *source, totals);
                }
                EntryKind::CacheWarm { totals, .. } => {
                    merge_source(&mut restored, UsageSource::CacheWarm, totals);
                }
                _ => {}
            }
        }
        ledger.restore(&restored);
    }

    /// The session's spend so far.
    pub(crate) fn usage_snapshot(&self) -> SessionUsage {
        self.usage_ledger().snapshot()
    }

    /// Records that this session worked on pull request `number`, unless it already has.
    ///
    /// Appended at the current leaf and never made the leaf, the way a usage record is: it is a
    /// fact about the session, not a turn of the conversation.
    pub(crate) async fn link_pull_request(&self, number: u64, url: String) {
        let session = self.current_session_id().await;
        let Ok(Some(parent)) = self.resolve_leaf().await else {
            tracing::debug!(number, "no session leaf yet; pull request not linked");
            return;
        };
        let Ok(mut store) = self.config.session_store.write() else {
            tracing::warn!("session store lock poisoned; pull request not linked");
            return;
        };
        let linked = store
            .load(&session)
            .map(|entries| crate::session::linked_pull_requests(&entries).contains(&number))
            .unwrap_or(false);
        if linked {
            return;
        }
        if let Err(error) =
            store.append(&session, Some(parent), EntryKind::PullRequestLink { number, url })
        {
            tracing::warn!(%error, number, "could not link the session to its pull request");
        }
    }

    fn append_usage_entry(
        &self,
        session: &SessionId,
        source: UsageSource,
        model: &str,
        totals: UsageTotals,
        parent: EntryId,
    ) -> Option<EntryId> {
        let model = model.to_string();
        let kind = match source {
            UsageSource::CacheWarm => EntryKind::CacheWarm { model, totals },
            source => EntryKind::Usage { source, model, totals },
        };
        let appended = match self.config.session_store.write() {
            Ok(mut store) => store.append(session, Some(parent), kind),
            Err(_) => {
                tracing::warn!("session store lock poisoned; usage not persisted");
                return None;
            }
        };
        appended.inspect_err(|error| tracing::warn!(%error, "could not persist call usage")).ok()
    }
}

fn merge_source(usage: &mut SessionUsage, source: UsageSource, totals: &UsageTotals) {
    match usage.by_source.iter_mut().find(|entry| entry.source == source) {
        Some(entry) => entry.totals.merge(totals),
        None => usage.by_source.push(SourceUsage { source, totals: *totals }),
    }
    usage.total.merge(totals);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Billing, ModelRef};
    use crate::usage::SpendSink;
    use std::sync::Mutex;

    /// Remembers what it was told to record.
    #[derive(Debug, Default)]
    struct Recorder {
        calls: Mutex<Vec<(String, UsageSource, UsageTotals)>>,
    }

    #[async_trait::async_trait]
    impl SpendSink for Recorder {
        async fn record(&self, model_id: &str, source: UsageSource, totals: &UsageTotals) {
            self.calls.lock().unwrap().push((model_id.to_string(), source, *totals));
        }
    }

    fn a_model(id: &str) -> ModelInfo {
        ModelInfo { r#ref: ModelRef::new("test", id), ..ModelInfo::default() }
    }

    /// Every provider call reaches the run's sink, tagged with the model that made it.
    ///
    /// The break this catches: the sink existing and never being called, which is the same
    /// under-reported total it was added to fix — and a total that is quietly wrong is worse than
    /// one that is missing.
    #[tokio::test]
    async fn a_provider_call_is_reported_to_the_run_as_well_as_the_session() {
        let ledger = UsageLedger::new();
        let recorder = Arc::new(Recorder::default());
        let sink: Arc<dyn SpendSink> = recorder.clone();
        let (event_tx, _event_rx) = mpsc::channel(16);
        let usage = Usage { prompt_tokens: 100, completion_tokens: 20, ..Usage::default() };

        account_provider_call(
            &ledger,
            Some(&sink),
            UsageSource::Agent,
            &a_model("claude-sonnet-5"),
            &usage,
            &event_tx,
        )
        .await;

        let calls = recorder.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "one call in, one recorded");
        assert_eq!(calls[0].0, "claude-sonnet-5", "tagged with the model that spent it");
        assert_eq!(calls[0].1, UsageSource::Agent);
        assert_eq!(calls[0].2.calls, 1, "the totals cover this call alone");
        assert_eq!(calls[0].2.prompt_tokens, 100);
        assert_eq!(calls[0].2.completion_tokens, 20);
    }

    /// A call billed to a session switched away from still reaches the run's sink, and says
    /// nothing to the open session's readout.
    #[tokio::test]
    async fn a_retired_ledger_s_call_reaches_the_sink_and_publishes_nothing() {
        let ledger = UsageLedger::new();
        ledger.retire();
        let recorder = Arc::new(Recorder::default());
        let sink: Arc<dyn SpendSink> = recorder.clone();
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let usage = Usage { prompt_tokens: 100, ..Usage::default() };

        account_provider_call(
            &ledger,
            Some(&sink),
            UsageSource::Review,
            &a_model("m"),
            &usage,
            &event_tx,
        )
        .await;

        assert_eq!(recorder.calls.lock().unwrap().len(), 1, "the run's total counts it");
        assert!(event_rx.try_recv().is_err(), "the open session's readout does not");
        assert_eq!(ledger.take_pending().len(), 1, "it waits to settle to its own session");
    }

    /// A model billed to a plan reports its plan equivalent, never a charge.
    ///
    /// The break this catches: reporting a subscription turn as money the user spent.
    #[tokio::test]
    async fn a_plan_billed_call_reaches_the_sink_as_plan() {
        let ledger = UsageLedger::new();
        let recorder = Arc::new(Recorder::default());
        let sink: Arc<dyn SpendSink> = recorder.clone();
        let (event_tx, _event_rx) = mpsc::channel(16);
        let mut model = a_model("claude-sonnet-5");
        model.billing = Billing::Subscription;
        let usage = Usage { prompt_tokens: 100, completion_tokens: 20, ..Usage::default() };

        account_provider_call(&ledger, Some(&sink), UsageSource::Agent, &model, &usage, &event_tx)
            .await;

        let calls = recorder.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "one call in, one recorded");
        assert_eq!(calls[0].0, "claude-sonnet-5", "tagged with the model that spent it");
        assert_eq!(calls[0].2.prompt_tokens, 100);
        assert_eq!(calls[0].2.cost_usd, 0.0, "a plan turn is not a charge");
    }
}
