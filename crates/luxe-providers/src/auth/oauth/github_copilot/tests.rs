#![allow(clippy::unwrap_used)]

use super::models::parse_models;
use super::usage::unix_seconds;
use super::*;

#[test]
fn the_api_base_follows_the_tokens_proxy_endpoint() {
    assert_eq!(
        api_base_url("tid=1;proxy-ep=proxy.individual.githubcopilot.com;exp=2", None),
        "https://api.individual.githubcopilot.com"
    );
    assert_eq!(
        api_base_url("proxy-ep=proxy.business.githubcopilot.com", None),
        "https://api.business.githubcopilot.com"
    );
    // A host that isn't prefixed `proxy.` is used verbatim rather than mangled.
    assert_eq!(api_base_url("proxy-ep=gateway.example.com", None), "https://gateway.example.com");
    // No proxy-ep: fall back per account type.
    assert_eq!(api_base_url("tid=1", None), DEFAULT_API_BASE);
    assert_eq!(api_base_url("tid=1", Some("ghe.acme.com")), "https://copilot-api.ghe.acme.com");
}

#[test]
fn token_fields_are_parsed_out_of_the_semicolon_soup() {
    let token = "tid=abc; exp=1785964482;proxy-ep=proxy.x.githubcopilot.com ";
    assert_eq!(token_field(token, "tid").as_deref(), Some("abc"));
    assert_eq!(token_field(token, "exp").as_deref(), Some("1785964482"));
    assert!(token_field(token, "missing").is_none());
}

#[test]
fn enterprise_domains_are_normalized_and_bad_input_is_rejected() {
    assert_eq!(normalize_domain("").unwrap(), None);
    assert_eq!(normalize_domain("   ").unwrap(), None);
    assert_eq!(normalize_domain("company.ghe.com").unwrap().as_deref(), Some("company.ghe.com"));
    assert_eq!(
        normalize_domain("https://company.ghe.com/path").unwrap().as_deref(),
        Some("company.ghe.com")
    );
    assert!(normalize_domain("not a domain").is_err());
    assert!(normalize_domain("localhost").is_err(), "a bare word is a typo, not a domain");
}

#[test]
fn a_pending_device_poll_is_not_a_failure() {
    assert!(matches!(
        classify(&serde_json::json!({ "error": "authorization_pending" })),
        Poll::Pending
    ));
    assert!(matches!(
        classify(&serde_json::json!({ "error": "slow_down", "interval": 10 })),
        Poll::SlowDown(Some(10))
    ));
    assert!(matches!(
        classify(&serde_json::json!({ "error": "expired_token", "error_description": "gone" })),
        Poll::Failed(message) if message == "gone"
    ));
    assert!(matches!(classify(&serde_json::json!({})), Poll::Failed(_)));
}

#[test]
fn model_rows_are_filtered_the_way_an_agent_needs() {
    let body = serde_json::json!({ "data": [
        { "id": "gpt-5.6-sol", "model_picker_enabled": true },
        { "id": "claude-sonnet-5", "model_picker_enabled": true,
          "policy": { "state": "unconfigured" } },
        { "id": "banned", "model_picker_enabled": true, "policy": { "state": "disabled" } },
        { "id": "embeddings", "model_picker_enabled": false },
        { "id": "no-tools", "model_picker_enabled": true,
          "capabilities": { "supports": { "tool_calls": false } } },
    ]});
    let models = parse_models(&body);

    let selectable: Vec<&str> =
        models.iter().filter(|m| m.selectable).map(|m| m.id.as_str()).collect();
    assert_eq!(selectable, vec!["gpt-5.6-sol"], "only usable models are offered");

    let pending: Vec<&str> =
        models.iter().filter(|m| m.needs_enabling).map(|m| m.id.as_str()).collect();
    assert_eq!(pending, vec!["claude-sonnet-5"], "unconfigured models get enabled");

    assert!(
        !models.iter().any(|m| m.id == "banned" && m.needs_enabling),
        "a model the user disabled on purpose must stay disabled"
    );
}

#[test]
fn no_models_payload_yields_no_models_rather_than_a_panic() {
    assert!(parse_models(&serde_json::json!({})).is_empty());
    assert!(parse_models(&serde_json::json!({ "data": "nope" })).is_empty());
    assert!(parse_models(&serde_json::json!({ "data": [{}] })).is_empty());
}

#[test]
fn editor_headers_include_the_integration_id_copilot_requires() {
    let names: Vec<&str> = EDITOR_HEADERS.iter().map(|(name, _)| *name).collect();
    assert!(names.contains(&"Copilot-Integration-Id"));
    assert!(names.contains(&"Editor-Version"));
}

/// The record of a Copilot Pro account billed by the token, copied from a live one with its
/// ids taken out: 18.6% of the month's allowance spent, renewing on the first.
#[test]
fn the_months_allowance_is_read_off_the_account_record() {
    let record = serde_json::json!({
        "copilot_plan": "individual",
        "quota_reset_date_utc": "2026-11-01T00:00:00.000Z",
        "token_based_billing": true,
        "quota_snapshots": {
            "chat": { "unlimited": true, "percent_remaining": 100.0 },
            "premium_interactions": {
                "entitlement": 1500, "remaining": 1221, "percent_remaining": 81.4,
                "overage_permitted": false, "unlimited": false, "token_based_billing": true
            }
        }
    });
    let october_8th = unix_seconds("2026-10-08T17:45:50.393Z").expect("a time");
    let plan = parse_plan_usage(&record, october_8th).expect("a plan");
    let window = plan.primary.expect("the monthly window");
    assert!((window.used_percent - 18.6).abs() < 0.01, "{}", window.used_percent);
    assert_eq!(window.window_label(), "monthly");
    assert_eq!(window.reset_label().as_deref(), Some("23d 6h"));
    assert_eq!(plan.plan.as_deref(), Some("individual"));

    let unlimited = serde_json::json!({ "quota_snapshots": {
        "premium_interactions": { "unlimited": true, "percent_remaining": 100.0 } } });
    assert!(parse_plan_usage(&unlimited, october_8th).is_none(), "no limit, no meter");
    assert!(parse_plan_usage(&serde_json::json!({}), october_8th).is_none());
}

#[test]
fn a_utc_timestamp_reads_as_unix_seconds() {
    assert_eq!(unix_seconds("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(unix_seconds("2026-11-01T00:00:00.000Z"), Some(1_793_491_200));
    assert_eq!(unix_seconds("2000-03-01T12:30:15Z"), Some(951_913_815));
    assert_eq!(unix_seconds("not a date"), None);
}
