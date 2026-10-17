//! `luxe wire` — audit a `LUXE_WIRE_LOG` capture.
//!
//! Answers the questions you cannot answer by reading source: what exactly did we send, how big was
//! each part of it, does the cacheable prefix actually stay byte-identical between turns, is
//! anything sent twice, and did a credential leak into the payload.
//!
//! Section sizes are reported in characters and in tokens *calibrated against the run itself* — the
//! provider tells us `prompt_tokens` for a body we can measure, so the chars-per-token ratio is
//! measured rather than guessed.

use super::args::WireArgs;

mod cache;
mod call;
mod secrets;
mod sections;

use cache::{cache_stability, transitions};
use call::{load, Call};
use sections::{chars_per_token, composition, duplication, overview, streaming};

/// Run `luxe wire`.
pub(crate) fn run_wire_command(args: WireArgs) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(&args.log)
        .map_err(|error| anyhow::anyhow!("cannot read {}: {error}", args.log.display()))?;
    let calls = load(&text);
    if calls.is_empty() {
        anyhow::bail!(
            "no provider calls in {} — run a turn with LUXE_WIRE_LOG set to that path first",
            args.log.display()
        );
    }
    let mut out = String::new();
    let ratio = chars_per_token(&calls);

    let churn = transitions(&calls);
    overview(&mut out, &calls, ratio, &warnings(&calls, &churn));
    composition(&mut out, &calls, ratio, args.tools);
    cache_stability(&mut out, &calls, &churn);
    duplication(&mut out, &calls);
    streaming(&mut out, &calls);
    secrets::report(&mut out, &text);

    crate::output::write(&out);
    Ok(())
}

/// The audit's verdict: the problems worth acting on, in one place at the top.
fn warnings(calls: &[Call], churn: &[Vec<String>]) -> Vec<String> {
    let mut warnings = Vec::new();
    let unstable = churn.iter().filter(|reasons| !reasons.is_empty()).count();
    if unstable > 0 {
        warnings.push(format!(
            "▲ the cacheable prefix changes on {unstable} of {} transition(s)",
            calls.len().saturating_sub(1)
        ));
    }
    if let Some(call) = calls.last() {
        let total = call.context_chars().max(1);
        let tools: usize = call.tool_entries().iter().map(|(_, _, j)| j.chars().count()).sum();
        let system = call.system().unwrap_or_default().chars().count();
        let overhead = (tools + system) as f64 / total as f64 * 100.0;
        if overhead > 80.0 {
            warnings.push(format!(
                "▲ {overhead:.0}% of the last request is fixed overhead (tools + system), not conversation"
            ));
        }
    }
    if warnings.is_empty() {
        warnings.push("no structural problems found".to_string());
    }
    warnings
}

#[cfg(test)]
mod tests;
