//! Copilot plan usage — how much of the subscription is left, as GitHub reports it.

use luxe_core::provider::{PlanUsage, PlanWindow};
use serde_json::Value;

use super::{EDITOR_HEADERS, GITHUB_COM};

/// How much of the plan's monthly allowance this account has used, from GitHub's own account
/// record. `None` when the record cannot be had, or the plan has no limit to report against.
///
/// Read with the GitHub token, the same one the sign-in exchanges for Copilot tokens, from the
/// same host. Nothing about the machine goes with it.
pub async fn plan_usage(github_token: &str, enterprise_domain: Option<&str>) -> Option<PlanUsage> {
    let domain = enterprise_domain.unwrap_or(GITHUB_COM);
    let url = format!("https://api.{domain}/copilot_internal/user");
    let mut request = crate::http::shared_client()
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {github_token}"));
    for (name, value) in EDITOR_HEADERS {
        request = request.header(*name, *value);
    }
    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        tracing::debug!(status = %response.status(), "copilot plan usage refused");
        return None;
    }
    let body: Value = response.json().await.ok()?;
    parse_plan_usage(&body, super::super::super::credential::now_ms() / 1000)
}

/// The plan usage in an account record, as of `now_secs`.
///
/// The allowance is `premium_interactions`, which on a plan billed by the token is drawn down by
/// every metered request's cost; `chat` and `completions` are unlimited on paid plans and say
/// nothing. An unlimited allowance reports no window rather than a meaningless 0%.
pub(crate) fn parse_plan_usage(body: &Value, now_secs: u64) -> Option<PlanUsage> {
    let allowance = body.pointer("/quota_snapshots/premium_interactions")?;
    if allowance.get("unlimited").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let remaining = allowance.get("percent_remaining").and_then(Value::as_f64)?;
    let resets_in_secs = body
        .get("quota_reset_date_utc")
        .and_then(Value::as_str)
        .and_then(unix_seconds)
        .map(|reset| reset.saturating_sub(now_secs));
    let window = PlanWindow {
        used_percent: (100.0 - remaining).clamp(0.0, 100.0) as f32,
        window_minutes: luxe_core::provider::MONTH_MINUTES,
        resets_in_secs,
    };
    Some(PlanUsage {
        plan: body.get("copilot_plan").and_then(Value::as_str).map(str::to_string),
        primary: Some(window),
        ..PlanUsage::default()
    })
}

/// Unix seconds of an RFC 3339 UTC timestamp (`2026-11-01T00:00:00.000Z`), to the second.
pub(super) fn unix_seconds(timestamp: &str) -> Option<u64> {
    let (date, time) = timestamp.split_once('T')?;
    let mut date = date.splitn(3, '-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let mut clock = time.trim_end_matches('Z').splitn(3, ':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    let second: i64 = clock.next()?.split('.').next()?.parse().ok()?;
    // Howard Hinnant's days-from-civil, so a date needs no calendar library
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second).ok()
}
