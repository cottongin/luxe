//! Plan headroom — what a ChatGPT subscription reports about its own quota.

use luxe_core::provider::{PlanUsage, PlanWindow};

/// Read a ChatGPT plan's remaining headroom out of a Codex response's headers.
///
/// The backend publishes `x-codex-*` headers on every response: which plan, how much of the primary
/// (weekly) and secondary windows is spent, when each resets, and any pay-as-you-go credit balance.
/// Header names and shapes were read off a live response rather than documentation, so every field
/// is optional and a missing or unparseable one is simply absent — this is a display signal, and a
/// wrong guess must never look like a real number.
pub fn plan_usage_from_headers(headers: &reqwest::header::HeaderMap) -> Option<PlanUsage> {
    let text = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let number = |name: &str| text(name).and_then(|value| value.parse::<f64>().ok());

    let window = |prefix: &str| {
        let used = number(&format!("x-codex-{prefix}-used-percent"))?;
        let minutes = number(&format!("x-codex-{prefix}-window-minutes")).unwrap_or(0.0) as u64;
        // A zero-length window is the backend saying "this one doesn't apply to your plan".
        (minutes > 0).then(|| PlanWindow {
            used_percent: used as f32,
            window_minutes: minutes,
            resets_in_secs: number(&format!("x-codex-{prefix}-reset-after-seconds"))
                .map(|secs| secs.max(0.0) as u64),
        })
    };

    let usage = PlanUsage {
        plan: text("x-codex-plan-type").map(str::to_string),
        primary: window("primary"),
        secondary: window("secondary"),
        // Only meaningful when the account actually has credits enabled.
        credits_balance: text("x-codex-credits-has-credits")
            .filter(|has| has.eq_ignore_ascii_case("true"))
            .and_then(|_| number("x-codex-credits-balance")),
    };
    (!usage.is_empty()).then_some(usage)
}
