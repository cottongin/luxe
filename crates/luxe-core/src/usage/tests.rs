use super::*;

fn usage(prompt: u32, completion: u32, read: u32, write: u32) -> Usage {
    Usage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        total_tokens: prompt + completion,
        cache_read_tokens: read,
        cache_write_tokens: write,
        reasoning_tokens: 0,
        cache_write_1h_tokens: 0,
    }
}

/// `claude-sonnet-5`'s published rates, per million tokens.
fn sonnet() -> Cost {
    Cost {
        input: 2.0,
        output: 10.0,
        cache_read: 0.2,
        cache_write: 2.5,
        cache_write_1h: 0.0,
        tiers: Vec::new(),
    }
}

/// The bug this module exists to prevent: a turn's calls are independent, so the session total
/// is their **sum**. The old reducer added `max(cost - previous, 0)` per event, which on a
/// long turn's monotonically growing context collapses to roughly the largest single call.
#[test]
fn a_multi_call_turn_totals_the_sum_of_its_calls() {
    let ledger = UsageLedger::new();
    let pricing = sonnet();
    // 47 calls over a growing context, the shape of a real feature-sized turn.
    let mut expected = 0.0;
    let mut delta_accumulated: f64 = 0.0;
    let mut previous = 0.0;
    for step in 0..47u32 {
        let prompt = 10_000 + step * 700;
        let call = usage(prompt, 250, prompt.saturating_sub(600), 600);
        let recorded = ledger.record(UsageSource::Agent, "test-model", &call, &pricing);
        let cost = recorded.cost_usd.expect("sonnet is priced");
        expected += cost;
        delta_accumulated += (cost - previous).max(0.0);
        previous = cost;
    }
    let total = ledger.snapshot().total;
    assert_eq!(total.calls, 47);
    assert!(
        (total.cost_usd - expected).abs() < 1e-9,
        "ledger must sum every call: {} vs {expected}",
        total.cost_usd
    );
    // And the old arithmetic really was off by multiples, not rounding.
    assert!(
        total.cost_usd > delta_accumulated * 3.0,
        "delta accumulation understates by design: {} vs {delta_accumulated}",
        total.cost_usd
    );
}

#[test]
fn totals_split_by_source_and_roll_up() {
    let ledger = UsageLedger::new();
    let pricing = sonnet();
    ledger.record(UsageSource::Agent, "test-model", &usage(1_000, 100, 0, 0), &pricing);
    ledger.record(UsageSource::Review, "test-model", &usage(6_000, 800, 0, 0), &pricing);
    ledger.record(UsageSource::Compaction, "test-model", &usage(20_000, 900, 0, 0), &pricing);

    let snapshot = ledger.snapshot();
    assert_eq!(snapshot.total.calls, 3);
    assert_eq!(snapshot.by_source.len(), 3);
    assert_eq!(
        snapshot.by_source[0].source,
        UsageSource::Agent,
        "display order is declaration order"
    );
    let review = snapshot.source(UsageSource::Review).expect("review spent");
    assert_eq!(review.prompt_tokens, 6_000);
    let summed: u64 = snapshot.by_source.iter().map(|entry| entry.totals.prompt_tokens).sum();
    assert_eq!(summed, snapshot.total.prompt_tokens, "the roll-up is the sum of its parts");
}

#[test]
fn an_unpriced_model_reports_no_cost_rather_than_zero() {
    let ledger = UsageLedger::new();
    let unpriced = Cost::default();
    ledger.record(UsageSource::Agent, "test-model", &usage(5_000, 500, 0, 0), &unpriced);
    let total = ledger.snapshot().total;
    assert_eq!(total.unpriced_calls, 1);
    assert_eq!(total.cost_label(), None, "a local model shows no cost at all");

    // Mixing one priced call in makes the figure a floor, not an exact number.
    ledger.record(UsageSource::Agent, "test-model", &usage(5_000, 500, 0, 0), &sonnet());
    let label = ledger.snapshot().total.cost_label().expect("something is priced now");
    assert!(label.starts_with('≥'), "a partial total must not pass as exact: {label}");
}

#[test]
fn restore_adopts_a_persisted_tally_and_reset_clears_it() {
    let ledger = UsageLedger::new();
    ledger.record(UsageSource::Agent, "test-model", &usage(1_000, 100, 0, 0), &sonnet());
    let persisted = ledger.snapshot();

    let resumed = UsageLedger::new();
    resumed.restore(&persisted);
    assert_eq!(resumed.snapshot(), persisted, "resume continues the same tally");

    // And a later call keeps adding to it rather than starting over.
    resumed.record(UsageSource::Agent, "test-model", &usage(1_000, 100, 0, 0), &sonnet());
    assert_eq!(resumed.snapshot().total.calls, 2);

    resumed.reset();
    assert!(resumed.snapshot().is_empty(), "a new session starts its own tally");
}

#[test]
fn cache_counts_are_subsets_of_the_input_total() {
    let ledger = UsageLedger::new();
    ledger.record(UsageSource::Agent, "test-model", &usage(10_000, 200, 9_000, 400), &sonnet());
    let total = ledger.snapshot().total;
    assert_eq!(total.fresh_prompt_tokens(), 600);
    let hit = total.cache_hit_pct().expect("input was recorded");
    assert!((hit - 90.0).abs() < 0.01, "cache hit rate: {hit}");
}

/// A healthy-looking hit rate is not evidence of a healthy bill, which is how a four-fold
/// overcharge went unnoticed for the length of a whole session.
#[test]
fn a_high_hit_rate_can_still_hide_a_prefix_being_re_written() {
    // The real shape, scaled down: 73% read, 27% re-written every call.
    let mut totals = UsageTotals::default();
    totals.add_call(&usage(74_300, 220, 54_306, 19_997), Some(0.0));

    let hit = totals.cache_hit_pct().expect("input was recorded");
    let written = totals.cache_write_pct().expect("input was recorded");
    assert!(hit > 70.0, "the hit rate looks fine: {hit}");
    assert!(written > 25.0, "and the writes are what the bill was: {written}");

    // Writing one turn's delta is what healthy looks like.
    let mut healthy = UsageTotals::default();
    healthy.add_call(&usage(74_300, 220, 72_000, 2_100), Some(0.0));
    assert!(healthy.cache_write_pct().expect("recorded") < 5.0);
}

/// The figure people are billed in is cents, so that is what a glance surface shows — with one
/// exception, which is the whole point of the function: a spend that rounds away says so.
#[test]
fn money_reads_in_cents_and_never_as_free() {
    assert_eq!(format_usd(0.0), "$0.00");
    assert_eq!(format_usd(0.0127), "$0.01");
    assert_eq!(format_usd(0.5), "$0.50");
    assert_eq!(format_usd(0.47), "$0.47");
    assert_eq!(format_usd(0.09256), "$0.09");
    assert_eq!(format_usd(1.5), "$1.50");
    assert_eq!(format_usd(123.456), "$123.46");
    assert_eq!(format_usd(0.005), "$0.01", "half a cent is where rounding starts working");
    for spend in [0.00001, 0.0001, 0.0049, 0.003] {
        assert_eq!(format_usd(spend), "<$0.01", "{spend} is real money, not free");
    }
    for spend in [0.00001, 0.0001, 0.009, 0.999, 1.0] {
        assert_ne!(format_usd(spend), "$0.00", "{spend} is real money");
    }
}

/// `/cost` attributes spend, so it has to be able to tell two sub-cent sources apart — which is
/// exactly what rounding to the cent takes away.
#[test]
fn the_exact_figure_keeps_enough_precision_to_attribute_spend() {
    assert_eq!(format_usd_exact(0.0), "$0.00");
    assert_eq!(format_usd_exact(0.0127), "$0.0127");
    assert_eq!(format_usd_exact(0.5), "$0.50");
    assert_eq!(format_usd_exact(0.09256), "$0.0926");
    assert_eq!(format_usd_exact(123.456), "$123.46");
    assert_eq!(format_usd_exact(0.00003), "<$0.0001", "smaller than the last step, but not free");
    assert_ne!(
        format_usd_exact(0.0012),
        format_usd_exact(0.0039),
        "two sub-cent sources have to stay distinguishable"
    );
}

/// A subscription turn must never report spend: the user paid for a plan, not for tokens.
#[test]
fn plan_billed_calls_are_value_not_spend() {
    let mut totals = UsageTotals::default();
    totals.add_billed_call(&usage(1_000, 100, 0, 0), Some(0.25), Billing::Subscription);

    assert_eq!(totals.cost_usd, 0.0, "nothing was charged");
    assert_eq!(totals.plan_usd, 0.25, "but the turn was worth something");
    assert_eq!(totals.plan_calls, 1);
    assert_eq!(totals.unpriced_calls, 0, "a plan call is priced, just not charged");
    assert_eq!(totals.cost_label(), None, "no dollar figure may read as a bill");
    assert_eq!(totals.plan_label().as_deref(), Some("≈$0.25"));
    assert!(totals.is_all_plan());
    // Tokens are still counted in full — the number a plan user actually watches.
    assert_eq!(totals.prompt_tokens, 1_000);
    assert_eq!(totals.completion_tokens, 100);
}

/// Switching providers mid-session reports each half honestly rather than summing them.
#[test]
fn a_session_that_mixes_a_plan_and_an_api_key_reports_both() {
    let mut totals = UsageTotals::default();
    totals.add_billed_call(&usage(1_000, 100, 0, 0), Some(0.25), Billing::Subscription);
    totals.add_billed_call(&usage(1_000, 100, 0, 0), Some(0.10), Billing::PerToken);

    assert_eq!(totals.cost_label().as_deref(), Some("$0.10"), "only the API half is spend");
    assert_eq!(totals.plan_label().as_deref(), Some("≈$0.25"));
    assert!(!totals.is_all_plan());
    assert_eq!(totals.calls, 2);
}

/// The `≥` marker means "spend we cannot name", and a plan call is not that.
#[test]
fn plan_calls_do_not_make_a_priced_total_look_partial() {
    let mut totals = UsageTotals::default();
    totals.add_billed_call(&usage(1_000, 100, 0, 0), Some(0.10), Billing::PerToken);
    totals.add_billed_call(&usage(1_000, 100, 0, 0), Some(0.25), Billing::Subscription);
    assert_eq!(totals.cost_label().as_deref(), Some("$0.10"), "no ≥: nothing is missing");

    // An actually-unpriced API call still earns the marker.
    totals.add_billed_call(&usage(500, 50, 0, 0), None, Billing::PerToken);
    assert_eq!(totals.cost_label().as_deref(), Some("≥$0.10"));
}

#[test]
fn merging_carries_the_plan_columns() {
    let mut left = UsageTotals::default();
    left.add_billed_call(&usage(100, 10, 0, 0), Some(0.20), Billing::Subscription);
    let mut right = UsageTotals::default();
    right.add_billed_call(&usage(100, 10, 0, 0), Some(0.30), Billing::Subscription);
    left.merge(&right);
    assert_eq!(left.plan_calls, 2);
    assert!((left.plan_usd - 0.50).abs() < 1e-9);
}

/// The per-turn event drives the "this turn cost X" display; a plan turn has no such number.
#[test]
fn a_recorded_plan_call_reports_no_per_turn_charge() {
    let ledger = UsageLedger::new();
    let recorded = ledger.record_billed(
        UsageSource::Agent,
        "gpt-5.3-codex",
        &usage(1_000, 100, 0, 0),
        &sonnet(),
        Billing::Subscription,
    );
    assert_eq!(recorded.cost_usd, None, "no charge to report for a plan turn");
    assert!(recorded.session.total.plan_usd > 0.0, "but the ledger still knows its value");
}

#[test]
fn token_counts_scale() {
    assert_eq!(format_tokens(0), "0");
    assert_eq!(format_tokens(999), "999");
    assert_eq!(format_tokens(1_200), "1.2k");
    assert_eq!(format_tokens(1_240_000), "1.2M");
}
