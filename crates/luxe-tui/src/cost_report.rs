//! `/cost` — what this session has spent, and on what.
//!
//! The audit surface for the ledger. A cost readout that can only be believed or disbelieved is
//! not much of a readout: this one shows the calls behind the figure, splits the spend by the part
//! of the harness that caused it (so "what does the review cost me?" has an answer), and says
//! plainly when part of the total is unpriced rather than quietly reporting a smaller number.
//!
//! Subscription usage gets its own line. Tokens on a ChatGPT or Copilot plan are already paid for,
//! so their published rates are reported as *value* (`≈$x, covered by your plan`) and never folded
//! into a spend figure.

use luxe_core::usage::{format_tokens, SessionUsage, UsageTotals};

/// Renders the session's spend as a system block, with the plan's remaining allowance when the
/// provider reports one.
pub(crate) fn render_with_plan(
    usage: &SessionUsage,
    plan: Option<&luxe_core::provider::PlanUsage>,
) -> String {
    let mut out = render(usage);
    if let Some(summary) = plan.and_then(luxe_core::provider::PlanUsage::summary) {
        out.push_str(&format!("\n\nallowance     {summary}"));
        if let Some(credits) = plan.and_then(|plan| plan.credits_balance) {
            out.push_str(&format!(
                "\ncredits       ${credits:.2} available once the plan is spent"
            ));
        }
    }
    out
}

/// Renders the session's spend as a system block.
pub(crate) fn render(usage: &SessionUsage) -> String {
    if usage.is_empty() {
        return "No provider calls yet this session.".to_string();
    }
    let total = &usage.total;
    let mut out = String::from("Session cost\n");
    out.push_str(&format!("  {}\n", headline(total)));
    out.push_str(&format!("  {}\n", token_line(total)));
    // Reported even at zero, and especially at zero. A provider block written for a base URL once
    // turned Anthropic prompt caching off for everyone who configured it by hand — no error, just a
    // tenfold input bill — and a line that hides itself when the number is bad is the one shape
    // that cannot surface that. One call has nothing to reuse, so the line waits for a second.
    if let Some(hit) = total.cache_hit_pct().filter(|_| total.calls > 1) {
        out.push_str(&format!("  cache        {}\n", cache_line(total, hit)));
    }

    // The breakdown earns its place only when more than one part of the harness spent something.
    if usage.by_source.len() > 1 {
        out.push_str("\nBy source\n");
        for entry in &usage.by_source {
            let cost = entry.totals.cost_label_exact().unwrap_or_else(|| "unpriced".to_string());
            out.push_str(&format!(
                "  {:<12} {:>10}   {} call{}   ↑{} ↓{}\n",
                entry.source.label(),
                cost,
                entry.totals.calls,
                if entry.totals.calls == 1 { "" } else { "s" },
                format_tokens(entry.totals.prompt_tokens),
                format_tokens(entry.totals.completion_tokens),
            ));
        }
    }

    if let Some(plan) = total.plan_label_exact() {
        let calls = total.plan_calls;
        out.push_str(&format!(
            "\nplan         {plan} of usage over {calls} call{}, covered by your subscription \
             (not billed per token)\n",
            if calls == 1 { "" } else { "s" }
        ));
    }

    if total.unpriced_calls > 0 {
        out.push_str(&format!(
            "\n{} of {} calls ran on a model with no published rates, so the total is a floor.\n",
            total.unpriced_calls, total.calls
        ));
    }
    out.trim_end().to_string()
}

fn headline(total: &UsageTotals) -> String {
    let calls = format!("{} call{}", total.calls, if total.calls == 1 { "" } else { "s" });
    match (total.cost_label_exact(), total.is_all_plan()) {
        (Some(cost), _) => format!("total        {cost}   over {calls}"),
        // Everything ran on a plan: there is no bill to headline, and saying "$0.00" would be as
        // misleading as an invented charge.
        (None, true) => format!("total        billed to your subscription   over {calls}"),
        (None, false) => format!("total        no published rates for this model   over {calls}"),
    }
}

/// What the cache did for this session, in the terms it is billed in.
///
/// The hit rate alone is the wrong half of the story and reads as reassurance: a prefix rewritten
/// on every turn is still read back on every turn, so a session can sit at 73% cached while four
/// fifths of its input bill is writes at roughly twelve times the read rate. So the written share
/// is named too, as soon as it stops being a rounding error.
fn cache_line(total: &UsageTotals, hit: f32) -> String {
    let served = format!(
        "{hit:.0}% of input served from cache ({} fresh)",
        format_tokens(total.fresh_prompt_tokens())
    );
    match total.cache_write_pct() {
        Some(written) if written >= 1.0 => format!(
            "{served}, {written:.0}% re-written ({})",
            format_tokens(total.cache_write_tokens)
        ),
        _ => served,
    }
}

fn token_line(total: &UsageTotals) -> String {
    format!(
        "tokens       ↑{} in · ↓{} out{}",
        format_tokens(total.prompt_tokens),
        format_tokens(total.completion_tokens),
        match total.reasoning_tokens {
            0 => String::new(),
            n => format!(" ({} reasoning)", format_tokens(n)),
        }
    )
}

/// The `/cost` line for a session that never reached a provider — `format_usd` is exercised in
/// `luxe-core`, so these cover the report's own shape.
#[cfg(test)]
mod tests;
