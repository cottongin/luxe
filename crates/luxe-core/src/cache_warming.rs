//! Cache warming — keeping a conversation's prompt cache alive through a long wait.
//!
//! A provider's prompt cache forgets an entry a fixed time after it was last read: five minutes on
//! Anthropic unless the hour-long tier was asked for. A turn that waits on a slow tool, a
//! permission prompt or a question longer than that loses the whole conversation's entry, and the
//! next request re-writes it at the write rate — about $2 on a 400k Opus transcript. Re-sending the
//! last request with a one-token cap just before the entry expires reads it instead, which costs a
//! tenth of the input rate and starts the lifetime over.
//!
//! This module is the policy: which [`CacheWarming`] mode is on, when a refresh is due, and whether
//! one is worth sending. The engine runs it (`Action → update → Command → worker → Event → apply`
//! has no room for a timer, so the engine owns one task per conversation), and the adapter decides
//! whether a request can be replayed at all (`Provider::prompt_cache_ttl`).
//!
//! # Design
//!
//! Same policy as Pi's, checked against luxe's own ledgers:
//!
//! - **While a turn runs** the next request is certain, so a refresh pays whenever a read costs
//!   less than the write it saves. Across 2,980 recorded Claude calls this rescued 12 of 14 waits
//!   that outlived the cache and came out about $15 ahead.
//! - **Between turns** nobody knows whether the user is coming back. A refresh is sent only when
//!   the expected saving at a 15% chance of a reply clears the threshold, and never past 30 minutes
//!   idle. On the same ledgers that lost about $7, which is why it is opt-in.
//! - **Nothing is sent below $0.05 of expected saving**, so a small conversation is never warmed:
//!   the read would cost about what the write it saves would have.
//! - **A late timer gives up.** A refresh after the entry expired writes the whole prefix, which is
//!   the cost this exists to avoid, so a timer that wakes past its deadline (a suspended laptop)
//!   stops rather than sending.
//!
//! The hour-long tier ([`crate::provider::CompatFlags::supports_long_cache_retention`]) answers the
//! idle case differently: it doubles the write premium on every delta instead of paying for reads.
//! On the same ledgers it would have cost about $27 more than it saved, so it stays opt-in too.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::provider::{Billing, Cost, Usage};

/// The least a refresh must be expected to save before it is sent, in USD.
pub const MIN_EXPECTED_SAVING: f64 = 0.05;

/// The chance a user replies before an idle cache entry expires.
///
/// Pi's figure, measured from their own usage; per-session estimates did no better than the
/// constant.
pub const IDLE_CONTINUATION: f64 = 0.15;

/// The longest a turn's cache is kept warm, from the request that wrote it.
pub const MAX_STREAMING_AGE: Duration = Duration::from_secs(60 * 60);

/// The longest an idle conversation's cache is kept warm, from the last real request.
pub const MAX_IDLE_AGE: Duration = Duration::from_secs(30 * 60);

/// When the prompt cache is kept warm (`agent.cache_warming`).
///
/// Ordered by how much it may spend, which is what a project's config is compared on: a repository
/// may turn warming down and may not turn it up, since it would be spending the user's money.
/// `Unknown` sorts last on purpose, so a value this build can't read never lowers anything.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum CacheWarming {
    /// Never
    Off,
    /// While a turn is running (default)
    #[default]
    Streaming,
    /// While a turn is running and for up to 30 minutes after it
    Idle,
    /// A value this build doesn't know, which behaves as the default rather than failing the
    /// whole config file
    #[serde(other)]
    Unknown,
}

impl crate::config::Lenient for CacheWarming {
    const UNKNOWN: Self = Self::Unknown;
    const DEFAULT: Self = Self::Streaming;
    const DEFAULT_NAME: &'static str = "streaming";
    const NAME: &'static str = "cache warming";
}

impl CacheWarming {
    /// The mode this value behaves as: itself, or the default for one this build doesn't know.
    pub fn resolved(self) -> Self {
        match self {
            Self::Unknown => Self::default(),
            known => known,
        }
    }

    /// Whether any warming happens at all.
    pub fn enabled(self) -> bool {
        self.resolved() != Self::Off
    }
}

/// Where the conversation is when a refresh comes due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// A turn is running, so another request is certain
    Streaming,
    /// The turn ended and the next request is up to the user
    Idle,
}

impl Phase {
    /// The chance a real request reads the entry before it expires.
    pub fn continuation(self) -> f64 {
        match self {
            Self::Streaming => 1.0,
            Self::Idle => IDLE_CONTINUATION,
        }
    }

    /// How long warming may go on in this phase.
    pub fn max_age(self) -> Duration {
        match self {
            Self::Streaming => MAX_STREAMING_AGE,
            Self::Idle => MAX_IDLE_AGE,
        }
    }
}

/// How long after a request its refresh is due: 90% of the lifetime, keeping at least ten seconds
/// in hand. `None` for a lifetime too short to refresh inside.
pub fn refresh_delay(ttl: Duration) -> Option<Duration> {
    let margin = Duration::from_secs(10);
    (ttl > margin).then(|| ttl.mul_f64(0.9).min(ttl - margin))
}

/// The latest a refresh due at `delay` may still go out: half the margin it was scheduled with.
///
/// A timer can wake late after a suspend or a stalled runtime. Past this point the entry has
/// probably expired, and the refresh would be a full-price write rather than a read.
pub fn refresh_deadline(ttl: Duration, delay: Duration) -> Duration {
    delay + ttl.saturating_sub(delay) / 2
}

/// What one refresh would cost and save.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Decision {
    /// The refresh itself: a read of the prompt plus one output token
    pub warm_cost: f64,
    /// What the next real request costs extra if the entry expires: a write instead of a read
    pub miss_cost: f64,
    /// The chance that request arrives before the entry expires
    pub continuation: f64,
}

impl Decision {
    /// `continuation × miss_cost − warm_cost`.
    pub fn expected_saving(&self) -> f64 {
        self.continuation * self.miss_cost - self.warm_cost
    }

    /// Whether the refresh is worth sending.
    pub fn warms(&self) -> bool {
        self.expected_saving() >= MIN_EXPECTED_SAVING
    }
}

/// Prices one refresh of a `prompt_tokens` prompt, or `None` when there is no money to reason about.
///
/// Unpriced models and subscription plans both answer `None`: the first has no rates to weigh, and
/// the second is billed per month rather than per call, so a refresh saves nobody anything and
/// spends the plan's limits.
pub fn decide(phase: Phase, prompt_tokens: u64, cost: &Cost, billing: Billing) -> Option<Decision> {
    if prompt_tokens == 0 || !cost.is_known() || billing == Billing::Subscription {
        return None;
    }
    let prompt = u32::try_from(prompt_tokens).unwrap_or(u32::MAX);
    let call = |read: u32, write: u32, output: u32| Usage {
        prompt_tokens: prompt,
        completion_tokens: output,
        total_tokens: prompt.saturating_add(output),
        cache_read_tokens: read,
        cache_write_tokens: write,
        reasoning_tokens: 0,
        cache_write_1h_tokens: 0,
    };
    let hit = cost.usd(&call(prompt, 0, 0));
    let miss = cost.usd(&call(0, prompt, 0));
    Some(Decision {
        warm_cost: cost.usd(&call(prompt, 0, 1)),
        miss_cost: (miss - hit).max(0.0),
        continuation: phase.continuation(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Claude Opus 5's published rates.
    fn opus() -> Cost {
        Cost {
            input: 5.0,
            output: 25.0,
            cache_read: 0.5,
            cache_write: 6.25,
            cache_write_1h: 0.0,
            tiers: Vec::new(),
        }
    }

    #[test]
    fn the_refresh_lands_inside_the_lifetime_with_room_to_spare() {
        assert_eq!(refresh_delay(Duration::from_secs(300)), Some(Duration::from_secs(270)));
        assert_eq!(refresh_delay(Duration::from_secs(3600)), Some(Duration::from_secs(3240)));
        assert_eq!(refresh_delay(Duration::from_secs(60)), Some(Duration::from_secs(50)));
        assert_eq!(refresh_delay(Duration::from_secs(10)), None, "nothing fits in ten seconds");
        assert_eq!(
            refresh_deadline(Duration::from_secs(300), Duration::from_secs(270)),
            Duration::from_secs(285)
        );
    }

    /// The numbers behind the policy, on real rates: mid-turn a large prompt is always worth a
    /// read, a small one never is, and idle it takes a far larger prompt to clear the bar.
    #[test]
    fn a_refresh_is_sent_only_where_it_is_expected_to_pay() {
        let decide_opus = |phase, tokens| decide(phase, tokens, &opus(), Billing::PerToken);

        let mid_turn = decide_opus(Phase::Streaming, 200_000).expect("priced");
        assert!((mid_turn.miss_cost - 1.15).abs() < 1e-9, "a write of 200k less its read");
        assert!((mid_turn.warm_cost - 0.100_025).abs() < 1e-9, "a read of 200k plus one token");
        assert!(mid_turn.warms());

        assert!(!decide_opus(Phase::Streaming, 8_000).expect("priced").warms(), "too small");

        assert!(decide_opus(Phase::Idle, 200_000).expect("priced").warms());
        assert!(
            !decide_opus(Phase::Idle, 100_000).expect("priced").warms(),
            "worth it mid-turn only"
        );
        assert!(decide_opus(Phase::Streaming, 100_000).expect("priced").warms());
    }

    #[test]
    fn nothing_is_warmed_without_a_bill_to_shrink() {
        assert_eq!(decide(Phase::Streaming, 400_000, &Cost::default(), Billing::PerToken), None);
        assert_eq!(decide(Phase::Streaming, 400_000, &opus(), Billing::Subscription), None);
        assert_eq!(decide(Phase::Streaming, 0, &opus(), Billing::PerToken), None);
    }

    /// A model with no published cache rates still prices a miss: `Cost::usd` bills an unknown
    /// write rate as input, and an unknown read rate as input too, so the gap is zero and nothing
    /// is warmed on a guess.
    #[test]
    fn unknown_cache_rates_never_justify_a_refresh() {
        let input_only = Cost { input: 5.0, output: 25.0, ..Cost::default() };
        let decision = decide(Phase::Streaming, 400_000, &input_only, Billing::PerToken);
        assert!(!decision.expect("priced").warms());
    }

    /// Ordered by spend, which is the order a project is allowed to move it in.
    #[test]
    fn the_modes_are_ordered_by_what_they_may_spend() {
        assert!(CacheWarming::Off < CacheWarming::Streaming);
        assert!(CacheWarming::Streaming < CacheWarming::Idle);
        assert_eq!(CacheWarming::default(), CacheWarming::Streaming);
        let parsed: CacheWarming = serde_json::from_str("\"off\"").expect("parses");
        assert_eq!(parsed, CacheWarming::Off);
        // A typo costs the key, never the file, and lands on the default
        let typo: CacheWarming = serde_json::from_str("\"sometimes\"").expect("parses");
        assert_eq!(typo.resolved(), CacheWarming::Streaming);
        assert!(typo.enabled());
    }
}
