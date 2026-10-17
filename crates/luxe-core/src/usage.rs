//! Usage accounting — one ledger for every token a session spends.
//!
//! Every provider call in the process reports here, tagged with the part of the harness that made
//! it, so "what has this session cost me" is a sum the engine owns rather than something a UI
//! infers from a stream of events: a reducer reading per-call usage as if it were cumulative per
//! turn would report about one call's worth of a long turn.
//!
//! Three rules hold the accounting together:
//!
//! - **Every call is recorded.** A provider stream consumed without a [`UsageLedger::record`] is a
//!   bug, not an omission: the guide pass and the compaction summarizer spend money too.
//! - **Sources are separated.** [`UsageSource`] says which part of the harness spent it, so a
//!   reviewer can see what the review itself costs instead of guessing.
//! - **Unknown pricing is not zero.** A model with no published rates increments `unpriced_calls`;
//!   a total carrying one renders with a `≥` so it never passes as exact.
//! - **A plan is not a bill.** Tokens billed to a subscription accumulate in `plan_usd`, separate
//!   from `cost_usd`, so a ChatGPT or Copilot turn reports what it was *worth* without claiming the
//!   user was charged for it.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::provider::{Billing, Cost, Usage};

/// Which part of the harness spent the tokens.
///
/// Ordering is display order, and `BTreeMap` keeps the breakdown in it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    /// The agent loop — the turn the user asked for.
    #[default]
    Agent,
    /// Guided Review: the guide pass, and the summary drafted to post with it.
    Review,
    /// The compaction summarizer.
    Compaction,
    /// Keeping the prompt cache alive through a long wait ([`crate::cache_warming`]).
    CacheWarm,
}

impl UsageSource {
    /// Every source, in display order.
    pub const ALL: [Self; 4] = [Self::Agent, Self::Review, Self::Compaction, Self::CacheWarm];

    /// A short label for a breakdown row.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Review => "review",
            Self::Compaction => "compaction",
            Self::CacheWarm => "cache warming",
        }
    }
}

/// Somewhere a provider call's cost is reported *in addition to* the session ledger.
///
/// A [`UsageLedger`] measures one session and is reset and rebuilt whenever a session is loaded, so
/// it cannot answer a question that spans sessions. A continuous loop is exactly such a question: its
/// phase reviews run in an engine and a session of their own, and the reviewer is usually the
/// expensive model. A sink is handed to every engine taking part in a run, and adds up what they all
/// spent.
///
/// Called on a turn's path, so an implementation holds nothing for long.
#[async_trait::async_trait]
pub trait SpendSink: Send + Sync + std::fmt::Debug {
    /// Record one provider call.
    ///
    /// Args: `(model_id, source, totals)`, where `totals` covers this call alone.
    async fn record(&self, model_id: &str, source: UsageSource, totals: &UsageTotals);
}

/// Token and cost totals for one slice of a session's spend.
///
/// Counts follow [`Usage`]'s normalization: `prompt_tokens` is cache-inclusive, and the cache and
/// reasoning figures are subsets of it and of `completion_tokens` respectively. Summing them would
/// double-count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageTotals {
    /// Provider calls folded into these totals.
    #[serde(default)]
    pub calls: u64,
    /// All input tokens (cached + uncached).
    #[serde(default)]
    pub prompt_tokens: u64,
    /// Output tokens (including reasoning).
    #[serde(default)]
    pub completion_tokens: u64,
    /// Cache hits read, a subset of `prompt_tokens`.
    #[serde(default)]
    pub cache_read_tokens: u64,
    /// Tokens written to the cache, a subset of `prompt_tokens`.
    #[serde(default)]
    pub cache_write_tokens: u64,
    /// Reasoning tokens, a subset of `completion_tokens`.
    #[serde(default)]
    pub reasoning_tokens: u64,
    /// Spend in USD across the priced, per-token calls.
    #[serde(default)]
    pub cost_usd: f64,
    /// Calls whose model published no rates, so their spend is missing from `cost_usd`.
    #[serde(default)]
    pub unpriced_calls: u64,
    /// What the subscription-billed calls would have cost at their published rates. Value, not
    /// spend: the user already paid for these in a plan.
    #[serde(default)]
    pub plan_usd: f64,
    /// Calls billed to a subscription rather than per token.
    #[serde(default)]
    pub plan_calls: u64,
}

impl UsageTotals {
    /// Folds one provider call in. `cost` is `None` when the model published no rates.
    pub fn add_call(&mut self, usage: &Usage, cost: Option<f64>) {
        self.add_billed_call(usage, cost, Billing::PerToken);
    }

    /// Folds one provider call in, saying how it was paid for.
    ///
    /// A subscription call's value lands in `plan_usd` and never in `cost_usd`, so a session that
    /// mixes an API key and a plan can report each honestly.
    pub fn add_billed_call(&mut self, usage: &Usage, cost: Option<f64>, billing: Billing) {
        self.calls += 1;
        self.prompt_tokens += u64::from(usage.prompt_tokens);
        self.completion_tokens += u64::from(usage.completion_tokens);
        self.cache_read_tokens += u64::from(usage.cache_read_tokens);
        self.cache_write_tokens += u64::from(usage.cache_write_tokens);
        self.reasoning_tokens += u64::from(usage.reasoning_tokens);
        match (billing, cost) {
            (Billing::Subscription, Some(value)) => {
                self.plan_usd += value;
                self.plan_calls += 1;
            }
            // A plan whose model publishes no rates is still a plan call, not an unpriced one:
            // there is no missing spend to warn about.
            (Billing::Subscription, None) => self.plan_calls += 1,
            (Billing::PerToken, Some(cost)) => self.cost_usd += cost,
            (Billing::PerToken, None) => self.unpriced_calls += 1,
        }
    }

    /// Folds another total in (restoring a persisted tally, or rolling sources up).
    pub fn merge(&mut self, other: &Self) {
        self.calls += other.calls;
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
        self.reasoning_tokens += other.reasoning_tokens;
        self.cost_usd += other.cost_usd;
        self.unpriced_calls += other.unpriced_calls;
        self.plan_usd += other.plan_usd;
        self.plan_calls += other.plan_calls;
    }

    /// Whether anything has been recorded.
    pub fn is_empty(&self) -> bool {
        self.calls == 0
    }

    /// Input tokens that were not cache hits or cache writes — what was billed as fresh input.
    pub fn fresh_prompt_tokens(&self) -> u64 {
        self.prompt_tokens
            .saturating_sub(self.cache_read_tokens)
            .saturating_sub(self.cache_write_tokens)
    }

    /// Share of input tokens served from the cache, as a percentage.
    pub fn cache_hit_pct(&self) -> Option<f32> {
        (self.prompt_tokens > 0)
            .then(|| self.cache_read_tokens as f32 / self.prompt_tokens as f32 * 100.0)
    }

    /// Share of input tokens *written* to the cache, as a percentage.
    ///
    /// The figure that matters for a bill, and the one a hit rate hides. A write costs around
    /// 12x what a read does, so a session can be 73% "cached" and still spend four fifths of its
    /// input budget re-writing a prefix it should have been reading — which is exactly what a real
    /// session did. Healthy is a few percent: one turn's delta, written once.
    pub fn cache_write_pct(&self) -> Option<f32> {
        (self.prompt_tokens > 0)
            .then(|| self.cache_write_tokens as f32 / self.prompt_tokens as f32 * 100.0)
    }

    /// The cost as it should appear on screen, or `None` when there is nothing honest to say.
    ///
    /// Nothing priced means no figure at all — a local model shows no cost rather than `$0.00`.
    /// A partially priced total is prefixed `≥`, because the missing calls are spend we cannot
    /// name, not spend that did not happen.
    pub fn cost_label(&self) -> Option<String> {
        self.cost_label_with(format_usd)
    }

    /// [`Self::cost_label`] with enough precision to tell two sub-cent figures apart, for `/cost`.
    pub fn cost_label_exact(&self) -> Option<String> {
        self.cost_label_with(format_usd_exact)
    }

    fn cost_label_with(&self, money: fn(f64) -> String) -> Option<String> {
        let per_token_calls = self.calls - self.plan_calls;
        if per_token_calls == 0 || self.unpriced_calls == per_token_calls {
            return None;
        }
        Some(match self.unpriced_calls {
            0 => money(self.cost_usd),
            _ => format!("≥{}", money(self.cost_usd)),
        })
    }

    /// What the plan-billed calls were worth, for a UI that wants to say so.
    ///
    /// Deliberately not summed with [`Self::cost_label`]: one is money that left an account and the
    /// other is usage against something already paid for.
    pub fn plan_label(&self) -> Option<String> {
        self.plan_label_with(format_usd)
    }

    /// [`Self::plan_label`] at `/cost`'s precision.
    pub fn plan_label_exact(&self) -> Option<String> {
        self.plan_label_with(format_usd_exact)
    }

    fn plan_label_with(&self, money: fn(f64) -> String) -> Option<String> {
        (self.plan_calls > 0 && self.plan_usd > 0.0).then(|| format!("≈{}", money(self.plan_usd)))
    }

    /// Whether every call so far was billed to a subscription.
    pub fn is_all_plan(&self) -> bool {
        self.calls > 0 && self.plan_calls == self.calls
    }
}

/// One source's share of the spend.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SourceUsage {
    /// Which part of the harness spent it.
    pub source: UsageSource,
    /// That source's totals.
    pub totals: UsageTotals,
}

/// The whole session's spend: the roll-up plus the breakdown behind it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionUsage {
    /// Every source summed.
    #[serde(default)]
    pub total: UsageTotals,
    /// Per-source shares in display order; a source that never spent is omitted.
    #[serde(default)]
    pub by_source: Vec<SourceUsage>,
}

impl SessionUsage {
    /// A tally with a single source — a session that only ran turns, or a fixture.
    pub fn single(source: UsageSource, totals: UsageTotals) -> Self {
        Self { total: totals, by_source: vec![SourceUsage { source, totals }] }
    }

    /// One source's share, or `None` when it never spent.
    pub fn source(&self, source: UsageSource) -> Option<&UsageTotals> {
        self.by_source.iter().find(|entry| entry.source == source).map(|entry| &entry.totals)
    }

    /// Whether anything at all has been recorded.
    pub fn is_empty(&self) -> bool {
        self.total.is_empty()
    }
}

/// The session's running tally, shared by every provider call site.
///
/// Internally synchronized because the guide pass runs its batches concurrently; callers hold an
/// `Arc` and never a lock.
#[derive(Debug, Default)]
pub struct UsageLedger {
    state: Mutex<BTreeMap<UsageSource, UsageTotals>>,
    /// Spend recorded but not yet written to the transcript, keyed by source and model.
    ///
    /// Calls happen in places that cannot reach the session store — the guide pass runs detached
    /// and concurrent — so recording and persisting are decoupled: the ledger is always current,
    /// and whoever next holds the store drains this.
    pending: Mutex<BTreeMap<(UsageSource, String), UsageTotals>>,
    /// Set once the session it tallies is no longer the open one: work still holding it records
    /// its spend for that session, and nothing it records is announced as the open session's.
    retired: std::sync::atomic::AtomicBool,
}

/// One drained record waiting to be written to the transcript.
#[derive(Debug, Clone)]
pub struct PendingUsage {
    /// Which part of the harness spent it.
    pub source: UsageSource,
    /// The model that served the calls.
    pub model: String,
    /// Their combined totals.
    pub totals: UsageTotals,
}

/// What one recorded call cost, and what the session totals now.
#[derive(Debug, Clone)]
pub struct RecordedCall {
    /// This call's own spend, or `None` when the model published no rates.
    pub cost_usd: Option<f64>,
    /// The session totals after folding this call in.
    pub session: SessionUsage,
}

impl UsageLedger {
    /// An empty ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one provider call against `source`, priced with the model that actually served it.
    ///
    /// Pricing is per call rather than per session on purpose: an in-session model switch changes
    /// the rates, and a review or summarizer pass may run on a different model than the turn.
    pub fn record(
        &self,
        source: UsageSource,
        model: &str,
        usage: &Usage,
        pricing: &Cost,
    ) -> RecordedCall {
        self.record_billed(source, model, usage, pricing, Billing::PerToken)
    }

    /// Record one provider call, saying how it was paid for.
    pub fn record_billed(
        &self,
        source: UsageSource,
        model: &str,
        usage: &Usage,
        pricing: &Cost,
        billing: Billing,
    ) -> RecordedCall {
        let cost = pricing.is_known().then(|| pricing.usd(usage));
        let session = {
            let mut state = self.lock();
            state.entry(source).or_default().add_billed_call(usage, cost, billing);
            snapshot_of(&state)
        };
        self.pending_lock()
            .entry((source, model.to_string()))
            .or_default()
            .add_billed_call(usage, cost, billing);
        // A plan call reports no per-turn charge: there isn't one.
        let charged = billing.is_per_token().then_some(cost).flatten();
        RecordedCall { cost_usd: charged, session }
    }

    /// Takes everything awaiting persistence, leaving the running totals untouched.
    pub fn take_pending(&self) -> Vec<PendingUsage> {
        std::mem::take(&mut *self.pending_lock())
            .into_iter()
            .map(|((source, model), totals)| PendingUsage { source, model, totals })
            .collect()
    }

    /// Marks this ledger's session as no longer the open one.
    pub(crate) fn retire(&self) {
        self.retired.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether [`Self::retire`] was called.
    pub(crate) fn is_retired(&self) -> bool {
        self.retired.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Takes in spend recorded elsewhere for this ledger's session, as recorded and not yet
    /// written.
    pub(crate) fn absorb(&self, records: Vec<PendingUsage>) {
        let (mut state, mut pending) = (self.lock(), self.pending_lock());
        for record in records {
            state.entry(record.source).or_default().merge(&record.totals);
            pending.entry((record.source, record.model)).or_default().merge(&record.totals);
        }
    }

    /// Whether anything is waiting to be written.
    pub fn has_pending(&self) -> bool {
        !self.pending_lock().is_empty()
    }

    /// The current totals.
    pub fn snapshot(&self) -> SessionUsage {
        snapshot_of(&self.lock())
    }

    /// Replaces the tally wholesale — resuming a session adopts its persisted spend.
    ///
    /// Restored spend is already on disk, so nothing is queued for persistence.
    pub fn restore(&self, usage: &SessionUsage) {
        let mut state = self.lock();
        state.clear();
        for entry in &usage.by_source {
            state.entry(entry.source).or_default().merge(&entry.totals);
        }
        self.pending_lock().clear();
    }

    /// Clears the tally (a new session starts its own).
    pub fn reset(&self) {
        self.lock().clear();
        self.pending_lock().clear();
    }

    /// A poisoned ledger is recoverable: the counts are plain integers, so the worst a panicking
    /// writer can leave behind is a partially folded call.
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<UsageSource, UsageTotals>> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn pending_lock(
        &self,
    ) -> std::sync::MutexGuard<'_, BTreeMap<(UsageSource, String), UsageTotals>> {
        self.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn snapshot_of(state: &BTreeMap<UsageSource, UsageTotals>) -> SessionUsage {
    let mut total = UsageTotals::default();
    let mut by_source = Vec::with_capacity(state.len());
    for (source, totals) in state {
        if totals.is_empty() {
            continue;
        }
        total.merge(totals);
        by_source.push(SourceUsage { source: *source, totals: *totals });
    }
    SessionUsage { total, by_source }
}

/// Formats a USD figure to the cent, which is the unit anyone is actually billed in.
///
/// The one thing it must never do is report real work as free, and coding-agent turns cost fractions
/// of a cent as often as they cost dollars — so a spend that rounds away becomes `<$0.01` rather than
/// `$0.00`. Four decimals on a glance surface would be three digits of noise in front of the figure
/// that matters.
///
/// [`format_usd_exact`] is for the one surface that itemizes.
pub fn format_usd(cost: f64) -> String {
    if !cost.is_finite() || cost <= 0.0 {
        return "$0.00".to_string();
    }
    // Half a cent is where `{:.2}` starts printing zero, so that is where the floor has to sit.
    if cost < 0.005 {
        return "<$0.01".to_string();
    }
    format!("${cost:.2}")
}

/// Formats a USD figure with enough precision to tell two sub-cent figures apart.
///
/// For `/cost`, whose whole job is attributing spend: rounding every source to `<$0.01` would answer
/// "is this cheap" and lose "which part of it cost the most". Dollars get cents, sub-dollar amounts
/// get up to four decimals with trailing zeros trimmed, and anything below the last representable
/// step says so rather than rounding down to zero.
pub fn format_usd_exact(cost: f64) -> String {
    if !cost.is_finite() || cost <= 0.0 {
        return "$0.00".to_string();
    }
    if cost >= 1.0 {
        return format!("${cost:.2}");
    }
    let text = format!("{cost:.4}");
    if text == "0.0000" {
        return "<$0.0001".to_string();
    }
    let trimmed = text.trim_end_matches('0');
    // Keep at least two decimals so a round figure reads as money ($0.50, not $0.5).
    let kept = if trimmed.len() < 4 { &text[..4] } else { trimmed };
    format!("${kept}")
}

/// Formats a token count compactly (`1.2k`, `1.2M`).
pub fn format_tokens(count: u64) -> String {
    match count {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1_000_000.0),
        n if n >= 1_000 => format!("{:.1}k", n as f64 / 1_000.0),
        n => n.to_string(),
    }
}

#[cfg(test)]
mod tests;
