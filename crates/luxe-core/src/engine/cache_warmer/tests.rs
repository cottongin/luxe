use super::*;
use crate::provider::{Cost, ModelInfo, ProviderError, ProviderEvent, Usage};
use futures::stream::BoxStream;

/// Counts refreshes, answering each as a full read of a 200k prompt.
struct Counting {
    refreshes: Arc<AtomicU64>,
}

#[async_trait::async_trait]
impl Provider for Counting {
    fn name(&self) -> &str {
        "counting"
    }

    async fn stream(
        &self,
        _req: CompletionRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        Err(ProviderError::Other("not under test".into()))
    }

    async fn warm_cache(&self, req: CompletionRequest) -> Result<Usage, ProviderError> {
        assert_eq!(req.messages.len(), 1, "the request is replayed as it was sent");
        self.refreshes.fetch_add(1, Ordering::Relaxed);
        Ok(Usage {
            prompt_tokens: 200_000,
            completion_tokens: 1,
            total_tokens: 200_001,
            cache_read_tokens: 200_000,
            ..Usage::default()
        })
    }
}

struct Harness {
    warmer: CacheWarmer,
    refreshes: Arc<AtomicU64>,
    ledger: Arc<UsageLedger>,
    _events: mpsc::Receiver<Event>,
}

impl Harness {
    fn new(mode: CacheWarming) -> (Self, Accounting) {
        let (events, receiver) = mpsc::channel(64);
        let ledger = Arc::new(UsageLedger::new());
        let accounting = Accounting { ledger: ledger.clone(), sink: None, events };
        let harness = Self {
            warmer: CacheWarmer::new(mode),
            refreshes: Arc::new(AtomicU64::new(0)),
            ledger,
            _events: receiver,
        };
        (harness, accounting)
    }

    fn start(&self, prompt_tokens: u64, accounting: Accounting) {
        self.start_with_ttl(prompt_tokens, Duration::from_secs(300), accounting);
    }

    fn start_with_ttl(&self, prompt_tokens: u64, ttl: Duration, accounting: Accounting) {
        let model = ModelInfo {
            cost: Cost {
                input: 5.0,
                output: 25.0,
                cache_read: 0.5,
                cache_write: 6.25,
                cache_write_1h: 0.0,
                tiers: Vec::new(),
            },
            ..ModelInfo::default()
        };
        self.warmer.start(
            WarmRequest {
                provider: Arc::new(Counting { refreshes: self.refreshes.clone() }),
                request: CompletionRequest {
                    model,
                    messages: vec![crate::message::Message::user("hi")],
                    ..CompletionRequest::default()
                },
                ttl,
                sent_at: Instant::now(),
                estimated_prompt_tokens: prompt_tokens,
            },
            accounting,
        );
    }

    fn refreshes(&self) -> u64 {
        self.refreshes.load(Ordering::Relaxed)
    }
}

/// Lets the spawned task run up to `secs` of paused time.
async fn advance(secs: u64) {
    for _ in 0..secs {
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
    }
}

/// The case the whole feature exists for: a turn waiting on something slow keeps its cache.
/// One refresh every 270 seconds — inside the five-minute lifetime — each recorded as spend.
#[tokio::test(start_paused = true)]
async fn a_long_wait_inside_a_turn_is_refreshed_just_before_each_expiry() {
    let (harness, accounting) = Harness::new(CacheWarming::Streaming);
    harness.start(200_000, accounting);

    advance(269).await;
    assert_eq!(harness.refreshes(), 0, "nothing before the entry is close to expiring");
    advance(2).await;
    assert_eq!(harness.refreshes(), 1);
    advance(270).await;
    assert_eq!(harness.refreshes(), 2, "and again, timed from the refresh");

    let spent = harness.ledger.snapshot();
    let warming = spent
        .by_source
        .iter()
        .find(|entry| entry.source == UsageSource::CacheWarm)
        .expect("a refresh is spend like any other call");
    assert_eq!(warming.totals.calls, 2);
    assert!(warming.totals.cost_usd > 0.19, "two reads of 200k on Opus");
}

/// In the default mode warming belongs to the turn, so the turn ending ends it.
#[tokio::test(start_paused = true)]
async fn the_default_mode_stops_when_the_turn_ends() {
    let (harness, accounting) = Harness::new(CacheWarming::Streaming);
    harness.start(200_000, accounting);
    harness.warmer.settle();
    advance(600).await;
    assert_eq!(harness.refreshes(), 0);
}

/// Idle warming is a bet on the user coming back, so it is placed only on a conversation large
/// enough to make the bet pay, and never past half an hour.
#[tokio::test(start_paused = true)]
async fn idle_warming_bets_only_on_a_large_conversation_and_gives_up_after_half_an_hour() {
    let (harness, accounting) = Harness::new(CacheWarming::Idle);
    harness.start(200_000, accounting);
    harness.warmer.settle();
    advance(3_600).await;
    assert_eq!(harness.refreshes(), 6, "every 270s until the 30-minute limit, then nothing");

    let (small, accounting) = Harness::new(CacheWarming::Idle);
    small.start(100_000, accounting);
    small.warmer.settle();
    advance(600).await;
    assert_eq!(small.refreshes(), 0, "worth it mid-turn, not on a 15% chance of a reply");
}

/// The decision prices what the provider measured, not the estimate the run started with.
#[tokio::test(start_paused = true)]
async fn the_measured_prompt_replaces_the_estimate() {
    let (harness, accounting) = Harness::new(CacheWarming::Streaming);
    harness.start(1_000, accounting);
    harness.warmer.measured(200_000);
    advance(271).await;
    assert_eq!(harness.refreshes(), 1, "a 1k estimate alone would never be refreshed");
}

#[tokio::test(start_paused = true)]
async fn a_new_request_or_a_stop_retires_the_old_one() {
    let (harness, accounting) = Harness::new(CacheWarming::Streaming);
    harness.start(200_000, accounting);
    advance(200).await;
    let (_, accounting) = Harness::new(CacheWarming::Streaming);
    harness.start(200_000, accounting);
    advance(100).await;
    assert_eq!(harness.refreshes(), 0, "the replacement's clock started over");
    harness.warmer.stop();
    advance(600).await;
    assert_eq!(harness.refreshes(), 0);
}

/// A timer that wakes late — a suspended laptop, a stalled runtime — finds the entry probably
/// gone, and a refresh then would be a full-price write. It gives up instead. One that wakes
/// exactly at its deadline is still in time.
#[tokio::test(start_paused = true)]
async fn a_timer_that_wakes_past_its_deadline_gives_up_rather_than_write() {
    let (late, accounting) = Harness::new(CacheWarming::Streaming);
    late.start(200_000, accounting);
    tokio::time::advance(Duration::from_secs(286)).await;
    advance(600).await;
    assert_eq!(late.refreshes(), 0, "286s is past the 285s deadline for a 300s entry");

    let (on_time, accounting) = Harness::new(CacheWarming::Streaming);
    on_time.start(200_000, accounting);
    tokio::time::advance(Duration::from_secs(285)).await;
    tokio::task::yield_now().await;
    assert_eq!(on_time.refreshes(), 1, "exactly at the deadline still reads");
}

/// The age limit is inclusive: a refresh due exactly at 30 minutes idle still goes out. A
/// 200s lifetime refreshes every 180s, so the tenth lands on the limit itself.
#[tokio::test(start_paused = true)]
async fn a_refresh_due_exactly_at_the_age_limit_still_goes() {
    let (harness, accounting) = Harness::new(CacheWarming::Idle);
    harness.start_with_ttl(200_000, Duration::from_secs(200), accounting);
    harness.warmer.settle();
    advance(3_600).await;
    assert_eq!(harness.refreshes(), 10);
}

/// The engine going away takes its warming with it; an orphaned task would go on spending.
#[tokio::test(start_paused = true)]
async fn dropping_the_warmer_ends_its_task() {
    let (harness, accounting) = Harness::new(CacheWarming::Streaming);
    harness.start(200_000, accounting);
    let refreshes = harness.refreshes.clone();
    drop(harness);
    advance(600).await;
    assert_eq!(refreshes.load(Ordering::Relaxed), 0);
}

#[tokio::test(start_paused = true)]
async fn off_sends_nothing() {
    let (harness, accounting) = Harness::new(CacheWarming::Off);
    harness.start(400_000, accounting);
    advance(600).await;
    assert_eq!(harness.refreshes(), 0);
}
