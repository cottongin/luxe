//! Codex — plan headroom and the ChatGPT backend flavour.

use super::*;

#[test]
fn plan_headroom_is_read_from_a_real_codex_response() {
    let headers = codex_headers(&[
        ("x-codex-plan-type", "plus"),
        ("x-codex-primary-used-percent", "12"),
        ("x-codex-primary-window-minutes", "10080"),
        ("x-codex-primary-reset-after-seconds", "603553"),
        ("x-codex-secondary-used-percent", "0"),
        ("x-codex-secondary-window-minutes", "0"),
        ("x-codex-credits-has-credits", "False"),
        ("x-codex-credits-balance", "0"),
    ]);
    let plan = plan_usage_from_headers(&headers).expect("headers carry a plan");

    assert_eq!(plan.plan.as_deref(), Some("plus"));
    let primary = plan.primary.expect("weekly window");
    assert_eq!(primary.used_percent, 12.0);
    assert_eq!(primary.window_label(), "weekly");
    assert_eq!(primary.reset_label().as_deref(), Some("6d 23h"));
    // A zero-length window is the backend saying the plan has no such limit.
    assert!(plan.secondary.is_none(), "a 0-minute window is not a window");
    // Credits are only meaningful when the account actually has them.
    assert_eq!(plan.credits_balance, None);
    assert_eq!(plan.summary().as_deref(), Some("plus · 12% of weekly (resets in 6d 23h)"));
}

#[test]
fn a_response_without_plan_headers_reports_nothing_rather_than_zero() {
    assert!(plan_usage_from_headers(&codex_headers(&[])).is_none());
    assert!(plan_usage_from_headers(&codex_headers(&[("x-request-id", "abc")])).is_none());
    // Unparseable values are absent, not zero — a fake "0% used" is worse than no number.
    let garbled = codex_headers(&[
        ("x-codex-primary-used-percent", "many"),
        ("x-codex-primary-window-minutes", "10080"),
    ]);
    assert!(plan_usage_from_headers(&garbled).is_none());
}

/// The status line shows one window, and it should be the one about to bite.
#[test]
fn the_tightest_window_is_the_one_reported() {
    let headers = codex_headers(&[
        ("x-codex-plan-type", "pro"),
        ("x-codex-primary-used-percent", "20"),
        ("x-codex-primary-window-minutes", "10080"),
        ("x-codex-secondary-used-percent", "83"),
        ("x-codex-secondary-window-minutes", "300"),
        ("x-codex-secondary-reset-after-seconds", "1800"),
        ("x-codex-credits-has-credits", "true"),
        ("x-codex-credits-balance", "12.5"),
    ]);
    let plan = plan_usage_from_headers(&headers).expect("plan");
    assert_eq!(plan.summary().as_deref(), Some("pro · 83% of 5h (resets in 30m)"));
    assert_eq!(plan.credits_balance, Some(12.5));
}

/// The ChatGPT backend lives at a different path, and a base URL that already names it must not
/// be doubled up.
#[test]
fn the_codex_flavour_targets_the_chatgpt_backend_path() {
    let codex = OpenaiResponsesProvider::new(ProviderConfig::default()).codex();
    assert_eq!(codex.effective_base_url(), "https://chatgpt.com/backend-api");
    assert_eq!(
        codex.request_url("https://chatgpt.com/backend-api"),
        "https://chatgpt.com/backend-api/codex/responses"
    );
    assert_eq!(
        codex.request_url("https://proxy.local/codex/responses"),
        "https://proxy.local/codex/responses"
    );
    assert_eq!(
        codex.request_url("https://proxy.local/codex"),
        "https://proxy.local/codex/responses"
    );
    assert_eq!(codex.request_url("https://proxy.local/"), "https://proxy.local/codex/responses");

    // The standard flavour is untouched.
    let standard = make_provider();
    assert_eq!(
        standard.request_url("https://api.openai.com/v1"),
        "https://api.openai.com/v1/responses"
    );
}

/// Codex refuses an output cap and never stores a response; both are wire-level requirements
/// rather than preferences.
#[test]
fn the_codex_flavour_omits_the_output_cap_and_forces_stateless_requests() {
    let codex = OpenaiResponsesProvider::new(ProviderConfig::default()).codex();
    let mut req = make_req("gpt-5.3-codex");
    req.max_tokens = Some(4096);
    let body = codex.build_request_body(&req);

    assert!(body.get("max_output_tokens").is_none(), "Codex rejects an output cap");
    assert_eq!(body["store"], false);
    assert_eq!(body["include"][0], "reasoning.encrypted_content");
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["parallel_tool_calls"], true);
    // Codex requires instructions; a turn without a system prompt still gets some.
    assert!(body["instructions"].as_str().is_some_and(|text| !text.is_empty()));

    // The standard flavour keeps sending the cap.
    let standard = make_provider();
    assert_eq!(standard.build_request_body(&req)["max_output_tokens"], 4096);
}

// -----------------------------------------------------------------------
// Request-body building
// -----------------------------------------------------------------------

#[test]
fn request_body_basic_fields() {
    let p = make_provider();
    let req = make_req("gpt-5");
    let body = p.build_request_body(&req);

    assert_eq!(body["model"], "gpt-5");
    assert_eq!(body["stream"], true);
    assert!(body["input"].is_array());
    // No instructions when system is None.
    assert!(body.get("instructions").is_none());
    // No max_output_tokens when max_tokens is None.
    assert!(body.get("max_output_tokens").is_none());
}
