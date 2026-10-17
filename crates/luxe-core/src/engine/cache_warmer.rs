//! Cache warmer — the engine's half of [`crate::cache_warming`].
//!
//! One task per conversation, holding the last request a turn sent. It wakes just before that
//! request's cache entries expire, asks the policy whether a refresh pays, and if so has the
//! adapter replay the request with a one-token cap. A real request replaces the task, the end of a
//! turn moves it to its idle phase (or stops it, in `streaming` mode), and anything that changes
//! what the next request will look like — a model, a session, a compaction — stops it outright.
//!
//! Best-effort throughout: a refresh that fails stops warming and never touches the turn.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::cache_warming::{self, CacheWarming, Phase};
use crate::event::Event;
use crate::provider::{CompletionRequest, Provider};
use crate::usage::{SpendSink, UsageLedger, UsageSource};

/// A request whose cache entries are worth keeping alive, exactly as it was sent.
pub(crate) struct WarmRequest {
    /// The adapter that served it
    pub provider: Arc<dyn Provider + Send + Sync>,
    /// The request itself; its model carries the prices a refresh is weighed against
    pub request: CompletionRequest,
    /// How long its cache entries live after a read
    pub ttl: Duration,
    /// When it went out, which is when its entries' lifetime started
    pub sent_at: Instant,
    /// Its prompt size until the provider measures it
    pub estimated_prompt_tokens: u64,
}

/// Where a refresh's spend is reported: the same places every other provider call reports to.
pub(crate) struct Accounting {
    pub ledger: Arc<UsageLedger>,
    pub sink: Option<Arc<dyn SpendSink>>,
    pub events: mpsc::Sender<Event>,
}

/// Keeps the last request's prompt cache warm, per [`CacheWarming`].
pub(crate) struct CacheWarmer {
    mode: CacheWarming,
    run: Mutex<Option<Run>>,
}

/// The task warming one request, and what the engine tells it while it runs.
struct Run {
    task: tokio::task::JoinHandle<()>,
    idle: Arc<AtomicBool>,
    prompt_tokens: Arc<AtomicU64>,
}

impl CacheWarmer {
    pub(crate) fn new(mode: CacheWarming) -> Self {
        Self { mode: mode.resolved(), run: Mutex::new(None) }
    }

    /// Whether a turn should hand its requests over at all.
    pub(crate) fn enabled(&self) -> bool {
        self.mode.enabled()
    }

    /// Starts keeping `warm` alive, replacing whatever was being kept before.
    pub(crate) fn start(&self, warm: WarmRequest, accounting: Accounting) {
        if !self.enabled() {
            return;
        }
        let idle = Arc::new(AtomicBool::new(false));
        let prompt_tokens = Arc::new(AtomicU64::new(warm.estimated_prompt_tokens));
        let task = tokio::spawn(keep_warm(warm, accounting, idle.clone(), prompt_tokens.clone()));
        self.replace(Some(Run { task, idle, prompt_tokens }));
    }

    /// Records the prompt size the provider measured for the request being kept warm.
    pub(crate) fn measured(&self, prompt_tokens: u64) {
        if let Some(run) = self.lock().as_ref() {
            run.prompt_tokens.store(prompt_tokens, Ordering::Relaxed);
        }
    }

    /// The turn is over: keep warming on idle terms, or stop if only turns are warmed.
    pub(crate) fn settle(&self) {
        match self.mode {
            CacheWarming::Idle => {
                if let Some(run) = self.lock().as_ref() {
                    run.idle.store(true, Ordering::Relaxed);
                }
            }
            _ => self.stop(),
        }
    }

    /// Stops warming, because the next request will not read what is being kept.
    pub(crate) fn stop(&self) {
        self.replace(None);
    }

    fn replace(&self, next: Option<Run>) {
        if let Some(previous) = std::mem::replace(&mut *self.lock(), next) {
            previous.task.abort();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Run>> {
        self.run.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Drop for CacheWarmer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Refreshes `warm` each time its entries are about to expire, until the policy says stop.
async fn keep_warm(
    warm: WarmRequest,
    accounting: Accounting,
    idle: Arc<AtomicBool>,
    prompt_tokens: Arc<AtomicU64>,
) {
    let model = &warm.request.model;
    let Some(delay) = cache_warming::refresh_delay(warm.ttl) else { return };
    let deadline = cache_warming::refresh_deadline(warm.ttl, delay);
    let mut last_sent = warm.sent_at;
    loop {
        tokio::time::sleep_until(last_sent + delay).await;
        let phase = if idle.load(Ordering::Relaxed) { Phase::Idle } else { Phase::Streaming };
        let now = Instant::now();
        if now.duration_since(warm.sent_at) > phase.max_age() {
            debug!(?phase, "cache warming reached its time limit");
            return;
        }
        if now > last_sent + deadline {
            debug!("woke too late to refresh the prompt cache before it expired; stopping");
            return;
        }
        let tokens = prompt_tokens.load(Ordering::Relaxed);
        let Some(decision) = cache_warming::decide(phase, tokens, &model.cost, model.billing)
        else {
            debug!(model = %model.r#ref, "no per-token prices to weigh a cache refresh against");
            return;
        };
        if !decision.warms() {
            debug!(
                ?phase,
                prompt_tokens = tokens,
                expected_saving = decision.expected_saving(),
                "a cache refresh would not pay for itself; stopping"
            );
            return;
        }
        let sent = Instant::now();
        match warm.provider.warm_cache(warm.request.clone()).await {
            Ok(usage) => {
                info!(
                    model = %model.r#ref,
                    ?phase,
                    cache_read = usage.cache_read_tokens,
                    cache_write = usage.cache_write_tokens,
                    "kept the prompt cache warm"
                );
                super::usage::account_provider_call(
                    &accounting.ledger,
                    accounting.sink.as_ref(),
                    UsageSource::CacheWarm,
                    model,
                    &usage,
                    &accounting.events,
                )
                .await;
            }
            Err(error) => {
                warn!(%error, model = %model.r#ref, "could not keep the prompt cache warm; stopping");
                return;
            }
        }
        last_sent = sent;
    }
}

#[cfg(test)]
mod tests;
