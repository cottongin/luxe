use super::*;
use luxe_core::usage::{SourceUsage, UsageSource};

fn totals(calls: u64, cost: f64) -> UsageTotals {
    UsageTotals {
        calls,
        prompt_tokens: 120_000,
        completion_tokens: 4_000,
        cache_read_tokens: 108_000,
        cost_usd: cost,
        ..Default::default()
    }
}

#[test]
fn an_untouched_session_says_so() {
    assert_eq!(render(&SessionUsage::default()), "No provider calls yet this session.");
}

/// A cache rate of zero is the interesting one: it means every request is re-billing a prefix
/// the provider was supposed to be reusing. Hiding it was how that stayed invisible.
#[test]
fn a_cold_cache_is_reported_rather_than_hidden() {
    let cold = UsageTotals { cache_read_tokens: 0, ..totals(12, 0.9) };
    let report = render(&SessionUsage::single(UsageSource::Agent, cold));
    assert!(report.contains("cache        0% of input served from cache"), "{report}");

    // One call has nothing to reuse yet, so there is no rate worth reporting.
    let first = UsageTotals { cache_read_tokens: 0, ..totals(1, 0.1) };
    assert!(!render(&SessionUsage::single(UsageSource::Agent, first)).contains("cache"));
}

#[test]
fn the_report_names_the_total_the_calls_and_the_cache() {
    let report = render(&SessionUsage::single(UsageSource::Agent, totals(47, 0.4381)));
    assert!(report.contains("$0.4381"), "the exact figure, not a rounded one: {report}");
    assert!(report.contains("47 calls"), "the calls behind it: {report}");
    assert!(report.contains("90%"), "cache hit rate: {report}");
    assert!(!report.contains("By source"), "one source needs no breakdown: {report}");
    assert!(!report.contains("re-written"), "nothing was: {report}");
}

/// The shape a real session hid behind a healthy-looking hit rate: three quarters of the input
/// read back from cache, and a quarter of it re-written at twelve times the price.
#[test]
fn a_prefix_being_re_written_is_named_beside_the_hit_rate() {
    let churning =
        UsageTotals { cache_read_tokens: 87_000, cache_write_tokens: 33_000, ..totals(47, 12.50) };
    let report = render(&SessionUsage::single(UsageSource::Agent, churning));
    assert!(report.contains("72% of input served from cache"), "{report}");
    assert!(report.contains("28% re-written (33.0k)"), "{report}");
}

/// The question the review has to be able to answer about itself.
#[test]
fn a_review_pass_appears_as_its_own_line() {
    let agent = totals(20, 0.30);
    let review = UsageTotals { calls: 3, cost_usd: 0.04, ..totals(3, 0.04) };
    let mut total = agent;
    total.merge(&review);
    let report = render(&SessionUsage {
        total,
        by_source: vec![
            SourceUsage { source: UsageSource::Agent, totals: agent },
            SourceUsage { source: UsageSource::Review, totals: review },
        ],
    });
    assert!(report.contains("By source"));
    assert!(report.contains("review"), "the review's own share is named: {report}");
    assert!(report.contains("$0.04"));
}

#[test]
fn a_partly_unpriced_total_says_it_is_a_floor() {
    let mixed = UsageTotals { unpriced_calls: 5, ..totals(20, 0.12) };
    let report = render(&SessionUsage::single(UsageSource::Agent, mixed));
    assert!(report.contains('≥'), "the figure is marked as a floor: {report}");
    assert!(report.contains("5 of 20 calls"), "and the reason is stated: {report}");
}
/// A plan session must never headline a dollar figure — and must not read as "unpriced" either.
#[test]
fn a_subscription_session_reports_value_not_spend() {
    let mut plan = UsageTotals::default();
    plan.add_billed_call(
        &luxe_core::provider::Usage {
            prompt_tokens: 12_000,
            completion_tokens: 900,
            total_tokens: 12_900,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 300,
            cache_write_1h_tokens: 0,
        },
        Some(0.42),
        luxe_core::provider::Billing::Subscription,
    );
    let report = render(&SessionUsage::single(luxe_core::usage::UsageSource::Agent, plan));

    assert!(report.contains("billed to your subscription"), "{report}");
    assert!(report.contains("≈$0.42"), "the value is still worth reporting: {report}");
    assert!(report.contains("covered by your subscription"), "{report}");
    assert!(!report.contains("no published rates"), "a plan model is priced: {report}");
    // Tokens remain the primary number.
    assert!(report.contains("12.0k") || report.contains("12k"), "{report}");
}

/// A session that used both must show the charge and the plan value separately.
#[test]
fn a_mixed_session_separates_the_bill_from_the_plan() {
    let call = luxe_core::provider::Usage {
        prompt_tokens: 1_000,
        completion_tokens: 100,
        total_tokens: 1_100,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        cache_write_1h_tokens: 0,
    };
    let mut mixed = UsageTotals::default();
    mixed.add_billed_call(&call, Some(0.10), luxe_core::provider::Billing::PerToken);
    mixed.add_billed_call(&call, Some(0.25), luxe_core::provider::Billing::Subscription);
    let report = render(&SessionUsage::single(luxe_core::usage::UsageSource::Agent, mixed));

    assert!(report.contains("total        $0.10"), "only the API half is spend: {report}");
    assert!(report.contains("≈$0.25"), "{report}");
}
/// On a plan the binding constraint is the allowance, so `/cost` has to name it.
#[test]
fn the_report_names_the_plans_remaining_allowance() {
    let mut plan = UsageTotals::default();
    plan.add_billed_call(
        &luxe_core::provider::Usage {
            prompt_tokens: 5_000,
            completion_tokens: 100,
            total_tokens: 5_100,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            cache_write_1h_tokens: 0,
        },
        Some(0.01),
        luxe_core::provider::Billing::Subscription,
    );
    let usage = SessionUsage::single(luxe_core::usage::UsageSource::Agent, plan);
    let headroom = luxe_core::provider::PlanUsage {
        plan: Some("plus".into()),
        primary: Some(luxe_core::provider::PlanWindow {
            used_percent: 12.0,
            window_minutes: 10_080,
            resets_in_secs: Some(603_553),
        }),
        secondary: None,
        credits_balance: Some(4.25),
    };

    let report = render_with_plan(&usage, Some(&headroom));
    assert!(report.contains("allowance"), "{report}");
    assert!(report.contains("12% of weekly"), "{report}");
    assert!(report.contains("resets in 6d 23h"), "{report}");
    assert!(report.contains("$4.25"), "credits are worth naming: {report}");

    // An API-key session says nothing about allowances, because it has none.
    let plain = render_with_plan(&usage, None);
    assert!(!plain.contains("allowance"), "{plain}");
}
