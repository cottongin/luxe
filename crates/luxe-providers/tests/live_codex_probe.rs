//! Live smoke test: does the ChatGPT-account Codex backend still report plan headroom?
//!
//! Ignored, so it never runs in CI or on a machine without a ChatGPT plan, and reports as ignored
//! rather than passed. Run on purpose, it needs a stored credential and fails without one. It
//! exists because the `x-codex-*` headers luxe's plan meter reads are undocumented: they were read
//! off a live response, and the only way to learn they have moved is to ask. A failure here means the meter has gone quiet, not that the login is broken.
//!
//! Run it with `--no-capture` to also see the raw headers, which is what you want when it fails:
//!
//! ```text
//! cargo nextest run -p luxe-providers -E 'binary(live_codex_probe)' --run-ignored only --no-capture
//! ```

#![allow(clippy::unwrap_used)]

use luxe_providers::auth::{AuthStore, Credential};

#[tokio::test]
#[ignore = "needs a stored ChatGPT credential and a network"]
async fn codex_still_reports_plan_headroom() {
    let Some(Credential::OAuth(credential)) = AuthStore::default_path().credential("openai-codex")
    else {
        panic!("no stored openai-codex credential — run `luxe auth login codex`");
    };
    let account = credential.account.clone().expect("credential carries an account id");

    // The smallest request that still gets a real response: one word in, one word out.
    let body = serde_json::json!({
        "model": "gpt-5.6-sol",
        "stream": true,
        "store": false,
        "instructions": "Answer in one word.",
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "ping" }],
        }],
    });

    let response = reqwest::Client::new()
        .post("https://chatgpt.com/backend-api/codex/responses")
        .header("Authorization", format!("Bearer {}", credential.access.expose()))
        .header("chatgpt-account-id", account)
        .header("OpenAI-Beta", "responses=experimental")
        .header("originator", "luxe")
        .header("Accept", "text/event-stream")
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .expect("the Codex backend is reachable");

    // Print the headers unconditionally: when the assertions below fail, this is the diagnosis.
    println!("status: {}", response.status());
    for (name, value) in response.headers() {
        if name.as_str().starts_with("x-codex-") {
            println!("  {name}: {}", value.to_str().unwrap_or("<binary>"));
        }
    }

    assert!(
        response.status().is_success(),
        "Codex refused the request ({}) — the credential or the model list may have moved",
        response.status()
    );

    let plan = luxe_providers::openai_responses::plan_usage_from_headers(response.headers())
        .expect(
            "no x-codex-* plan headers on the response: the meter has gone quiet, and \
             `plan_usage_from_headers` needs the new names (see the dump above)",
        );
    println!("parsed: {:?}", plan.summary());

    assert!(plan.plan.is_some(), "the response should name the plan: {plan:?}");
    let primary = plan.primary.expect("a plan has a primary allowance window");
    assert!(primary.window_minutes > 0, "a window with no length can't be rendered: {primary:?}");
    assert!(
        (0.0..=100.0).contains(&primary.used_percent),
        "used percent out of range: {primary:?}"
    );
    assert!(plan.summary().is_some(), "a parsed plan must render");
}
