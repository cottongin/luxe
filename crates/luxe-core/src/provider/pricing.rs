//! Pricing — what a model charges, and the tokens a call used.

use super::*;

/// Cost per million tokens.
///
/// Cache rates matter for real spend, not just completeness: with prompt caching on, most of a
/// long session's input tokens are cache hits billed at roughly a tenth of the input rate, so
/// charging them as fresh input overstates cost several-fold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Cost {
    /// Input cost per million tokens (USD).
    pub input: f64,
    /// Output cost per million tokens (USD).
    pub output: f64,
    /// Cache-hit read cost per million tokens (USD); `0` means unknown.
    #[serde(default)]
    pub cache_read: f64,
    /// Cache-write cost per million tokens (USD); `0` means unknown.
    #[serde(default)]
    pub cache_write: f64,
    /// Cost per million tokens of a cache write kept for an hour (USD); `0` means unknown, which
    /// prices it at twice the input rate, Anthropic's published multiplier and the only provider
    /// that reports these writes.
    #[serde(default, skip_serializing_if = "is_unpriced")]
    pub cache_write_1h: f64,
    /// Dearer rates for a long prompt, cheapest first. A request whose prompt passes a tier's
    /// threshold pays that tier's rates for all of it, which is how Anthropic and Google bill.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tiers: Vec<CostTier>,
}

/// The rates a request pays once its prompt, cache reads and writes included, passes
/// `above_tokens`. A cache rate of `0` falls back to this tier's input rate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CostTier {
    /// Prompt tokens past which this tier applies
    pub above_tokens: u64,
    /// Input cost per million tokens (USD)
    pub input: f64,
    /// Output cost per million tokens (USD)
    pub output: f64,
    /// Cache-hit read cost per million tokens (USD)
    #[serde(default)]
    pub cache_read: f64,
    /// Cache-write cost per million tokens (USD)
    #[serde(default)]
    pub cache_write: f64,
}

fn is_unpriced(rate: &f64) -> bool {
    *rate == 0.0
}

impl Cost {
    /// Whether any rate is known. A model with no pricing reports no cost rather than `$0.00`.
    pub fn is_known(&self) -> bool {
        self.input > 0.0 || self.output > 0.0
    }

    /// USD for one turn's usage.
    ///
    /// Every adapter normalizes `prompt_tokens` to be cache-inclusive, so the cache-read and
    /// cache-write counts are carved back out and billed at their own rates (roughly 0.1x and
    /// 1.25x of input) before the remainder is charged as fresh input. A rate of `0` means the
    /// catalog didn't publish one, in which case that slice falls back to the input rate.
    ///
    /// A prompt past one of [`Cost::tiers`]' thresholds is billed at that tier's rates instead.
    pub fn usd(&self, usage: &Usage) -> f64 {
        let input = usage.prompt_tokens as u64;
        let tier = self
            .tiers
            .iter()
            .filter(|tier| input > tier.above_tokens)
            .max_by_key(|tier| tier.above_tokens);
        let (input_rate, output_rate, read_rate, write_rate, long_rate) = match tier {
            // The hour-long write keeps Anthropic's multiplier, since no catalog tiers it
            Some(tier) => {
                (tier.input, tier.output, tier.cache_read, tier.cache_write, tier.input * 2.0)
            }
            None => (
                self.input,
                self.output,
                self.cache_read,
                self.cache_write,
                if self.cache_write_1h > 0.0 { self.cache_write_1h } else { self.input * 2.0 },
            ),
        };
        let rate_or_input = |rate: f64| if rate > 0.0 { rate } else { input_rate };
        let read = (usage.cache_read_tokens as u64).min(input);
        let write = (usage.cache_write_tokens as u64).min(input - read);
        let long = (usage.cache_write_1h_tokens as u64).min(write);
        let fresh = input - read - write;
        (fresh as f64 * input_rate
            + read as f64 * rate_or_input(read_rate)
            + (write - long) as f64 * rate_or_input(write_rate)
            + long as f64 * long_rate
            + usage.completion_tokens as f64 * output_rate)
            / 1_000_000.0
    }
}

/// Usage statistics from a completed turn.
///
/// Normalized so a field means the same thing across providers: `prompt_tokens` is *all*
/// input tokens (cached + uncached), matching OpenAI's convention. Anthropic reports uncached
/// input separately, so its adapter folds cache read/write back in. `cache_read_tokens` and
/// `cache_write_tokens` are the cache breakdown *within* `prompt_tokens`; `reasoning_tokens`
/// is the reasoning subset *within* `completion_tokens`. New fields default to 0 so older
/// session transcripts still deserialize.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Usage {
    /// All input tokens (cached + uncached).
    pub prompt_tokens: u32,
    /// Output tokens (including any reasoning tokens).
    pub completion_tokens: u32,
    /// Total tokens (`prompt_tokens + completion_tokens`).
    pub total_tokens: u32,
    /// Cached input tokens read (a subset of `prompt_tokens`), billed at a discount.
    #[serde(default)]
    pub cache_read_tokens: u32,
    /// Input tokens written to the cache (a subset of `prompt_tokens`), billed at a premium.
    #[serde(default)]
    pub cache_write_tokens: u32,
    /// Reasoning/thinking tokens (a subset of `completion_tokens`).
    #[serde(default)]
    pub reasoning_tokens: u32,
    /// The cache writes kept for an hour rather than five minutes (a subset of
    /// `cache_write_tokens`), which Anthropic bills at twice the input rate rather than 1.25x.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_write_1h_tokens: u32,
}

fn is_zero(count: &u32) -> bool {
    *count == 0
}

/// How a model's tokens are paid for.
///
/// Not cosmetic: a subscription provider's published rates are what the same tokens would have cost
/// on the API, and presenting that as *spend* tells a user they were charged money they weren't.
/// Luxe keeps the number — it's a useful measure of what a turn was worth — and reports it in its
/// own column so a plan's usage is never mistaken for a bill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Billing {
    /// Charged per token against an API key.
    #[default]
    PerToken,
    /// Included in a subscription (ChatGPT Plus/Pro, GitHub Copilot).
    Subscription,
}

impl Billing {
    /// Whether tokens are billed per token against an account balance.
    pub fn is_per_token(self) -> bool {
        matches!(self, Self::PerToken)
    }
}
