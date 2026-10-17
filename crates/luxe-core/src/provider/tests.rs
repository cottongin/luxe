use super::*;

/// "Unchosen" is an empty id and nothing else.
///
/// The whole safety argument rests on it: no provider serves an empty model id, so a turn that
/// somehow reaches the wire without a choice fails at the request instead of billing something
/// plausible. A placeholder that looked like a model — `unknown`, or the default's — would
/// lose that, so neither counts as unset.
#[test]
fn only_an_empty_id_names_no_model() {
    assert!(ModelRef::new("openai_compat", "").is_unset());
    assert!(ModelRef::new("", "").is_unset());
    assert!(ModelRef::new("anthropic", "   ").is_unset(), "whitespace is not an id");

    assert!(!ModelRef::new("openai", "gpt-5.6").is_unset());
    assert!(!ModelInfo::default().r#ref.is_unset(), "the default is a stand-in, not an absence");
}

/// Opus 5 rates. Cache reads are a tenth of input and writes a quarter more, so charging
/// everything as fresh input badly overstates a long, cache-heavy session.
fn opus_5() -> Cost {
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
fn cost_carves_cache_reads_and_writes_out_of_cache_inclusive_input() {
    // Every adapter reports `prompt_tokens` cache-inclusive: 20k fresh + 180k read.
    let usage = Usage {
        prompt_tokens: 200_000,
        completion_tokens: 10_000,
        total_tokens: 210_000,
        cache_read_tokens: 180_000,
        ..Default::default()
    };
    // 20k*$5 + 180k*$0.50 + 10k*$25 = 0.10 + 0.09 + 0.25
    assert!((opus_5().usd(&usage) - 0.44).abs() < 1e-9);

    // The same tokens written to cache instead cost the write premium:
    // 20k*$5 + 180k*$6.25 + 10k*$25 = 0.10 + 1.125 + 0.25
    let writing = Usage { cache_read_tokens: 0, cache_write_tokens: 180_000, ..usage.clone() };
    assert!((opus_5().usd(&writing) - 1.475).abs() < 1e-9);
}

/// A write kept for an hour is priced at its own rate, twice input when the catalog has none,
/// and is never counted twice against the five-minute rate it is a part of.
#[test]
fn an_hour_long_cache_write_costs_twice_input() {
    let opus =
        Cost { input: 5.0, output: 25.0, cache_read: 0.5, cache_write: 6.25, ..Cost::default() };
    let usage = |write, long| Usage {
        prompt_tokens: 1_000_000,
        cache_write_tokens: write,
        cache_write_1h_tokens: long,
        ..Usage::default()
    };
    assert!((opus.usd(&usage(1_000_000, 0)) - 6.25).abs() < 1e-9);
    assert!((opus.usd(&usage(1_000_000, 1_000_000)) - 10.0).abs() < 1e-9);
    assert!((opus.usd(&usage(1_000_000, 400_000)) - (0.6 * 6.25 + 0.4 * 10.0)).abs() < 1e-9);
    let priced = Cost { cache_write_1h: 8.0, ..opus.clone() };
    assert!((priced.usd(&usage(1_000_000, 1_000_000)) - 8.0).abs() < 1e-9);
    // a count that claims more hour-long writes than writes is held to the writes
    assert!((opus.usd(&usage(500_000, 900_000)) - (0.5 * 5.0 + 0.5 * 10.0)).abs() < 1e-9);
}

/// Haiku 5.5's models.dev entry: five times every rate once the prompt passes 100k.
#[test]
fn a_prompt_past_a_tier_pays_that_tier_for_all_of_it() {
    let haiku = Cost {
        input: 0.1,
        output: 0.5,
        cache_read: 0.01,
        cache_write: 0.125,
        tiers: vec![CostTier {
            above_tokens: 100_000,
            input: 0.5,
            output: 2.5,
            cache_read: 0.05,
            cache_write: 0.625,
        }],
        ..Cost::default()
    };
    let usage = |prompt| Usage {
        prompt_tokens: prompt,
        completion_tokens: 1_000,
        cache_read_tokens: prompt / 2,
        ..Usage::default()
    };
    // 50k*$0.1 + 50k*$0.01 + 1k*$0.5
    assert!(
        (haiku.usd(&usage(100_000)) - 0.006).abs() < 1e-9,
        "the threshold itself is not past it"
    );
    // 60k*$0.5 + 60k*$0.05 + 1k*$2.5
    assert!((haiku.usd(&usage(120_000)) - 0.0355).abs() < 1e-9);
}

#[test]
fn cost_falls_back_to_the_input_rate_when_cache_rates_are_unpublished() {
    let flat = Cost { input: 5.0, output: 25.0, ..Default::default() };
    let usage = Usage {
        prompt_tokens: 100_000,
        completion_tokens: 0,
        total_tokens: 100_000,
        cache_read_tokens: 60_000,
        ..Default::default()
    };
    assert!((flat.usd(&usage) - 0.5).abs() < 1e-9, "all input billed at the input rate");
    assert!(!Cost::default().is_known(), "no rates at all reports no cost");
}

#[test]
fn cost_tolerates_cache_counts_that_exceed_the_input_total() {
    // Defensive: a provider that double-counts must not underflow the fresh-token subtraction.
    let usage = Usage {
        prompt_tokens: 1_000,
        completion_tokens: 0,
        total_tokens: 1_000,
        cache_read_tokens: 900,
        cache_write_tokens: 900,
        ..Default::default()
    };
    let cost = opus_5().usd(&usage);
    assert!(cost.is_finite() && cost > 0.0, "got {cost}");
}

#[test]
fn ultra_accepts_max_as_a_serde_alias() {
    let level: ThinkingLevel = match serde_json::from_str("\"max\"") {
        Ok(level) => level,
        Err(error) => panic!("max alias should deserialize: {error}"),
    };
    assert_eq!(level, ThinkingLevel::Ultra);
    let serialized = match serde_json::to_string(&level) {
        Ok(serialized) => serialized,
        Err(error) => panic!("ultra should serialize: {error}"),
    };
    assert_eq!(serialized, "\"ultra\"");
}

#[test]
fn model_map_clamps_unsupported_qwen_tiers_upward() {
    let model = ModelInfo {
        reasoning: true,
        thinking_level_map: HashMap::from([
            ("off".to_string(), Some("off".to_string())),
            ("minimal".to_string(), None),
            ("low".to_string(), None),
            ("medium".to_string(), None),
            ("high".to_string(), Some("on".to_string())),
            ("xhigh".to_string(), Some("on".to_string())),
        ]),
        ..Default::default()
    };

    assert_eq!(model.effective_thinking_level(ThinkingLevel::Low), ThinkingLevel::High);
    assert_eq!(model.effective_thinking_level(ThinkingLevel::Ultra), ThinkingLevel::XHigh);
    assert_eq!(model.thinking_level_value(ThinkingLevel::High), Some("on"));
}

#[test]
fn model_map_reuses_provider_max_for_ultra() {
    let model = ModelInfo {
        reasoning: true,
        thinking_level_map: HashMap::from([
            ("minimal".to_string(), Some("low".to_string())),
            ("xhigh".to_string(), Some("xhigh".to_string())),
            ("max".to_string(), Some("max".to_string())),
        ]),
        ..Default::default()
    };

    assert_eq!(model.effective_thinking_level(ThinkingLevel::Ultra), ThinkingLevel::Ultra);
    assert_eq!(model.thinking_level_value(ThinkingLevel::Minimal), Some("low"));
    assert_eq!(model.thinking_level_value(ThinkingLevel::Ultra), Some("max"));
}
