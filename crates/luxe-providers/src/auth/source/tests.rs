#![allow(clippy::unwrap_used)]

use super::*;
use crate::auth::credential::{ApiKeyCredential, OAuthCredential};

fn store() -> (tempfile::TempDir, AuthStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = AuthStore::new(dir.path().join("auth.json"));
    (dir, store)
}

fn header<'a>(auth: &'a RequestAuth, name: &str) -> Option<&'a str> {
    auth.headers
        .iter()
        .find(|(header, _)| header.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.expose())
}

#[tokio::test]
async fn static_auth_reproduces_the_per_api_header_style() {
    let bearer = StaticAuth::for_api(ApiKind::OpenaiCompletions, Some(ApiKey::new("sk-1")))
        .request_auth()
        .await
        .unwrap();
    assert_eq!(header(&bearer, "authorization"), Some("Bearer sk-1"));

    // Anthropic rejects `Authorization: Bearer`; this is the regression that would break it.
    let anthropic = StaticAuth::for_api(ApiKind::AnthropicMessages, Some(ApiKey::new("sk-ant")))
        .request_auth()
        .await
        .unwrap();
    assert_eq!(header(&anthropic, "x-api-key"), Some("sk-ant"));
    assert!(header(&anthropic, "authorization").is_none());

    let none = StaticAuth::for_api(ApiKind::OpenaiCompletions, None).request_auth().await.unwrap();
    assert!(none.is_empty());
}

/// Foundry's Anthropic passthrough authenticates on `x-api-key` and its portal samples show
/// `api-key`; Anthropic's own Foundry client sends both, so a 401 can't turn on which one this
/// endpoint happened to want today.
#[tokio::test]
async fn the_foundry_style_sends_both_key_headers_and_never_a_bearer() {
    let auth = StaticAuth::for_style(KeyStyle::FoundryKey, Some(ApiKey::new("fk-1")))
        .request_auth()
        .await
        .unwrap();
    assert_eq!(header(&auth, "x-api-key"), Some("fk-1"));
    assert_eq!(header(&auth, "api-key"), Some("fk-1"));
    assert!(header(&auth, "authorization").is_none());

    let azure = StaticAuth::for_style(KeyStyle::AzureKey, Some(ApiKey::new("az-1")))
        .request_auth()
        .await
        .unwrap();
    assert_eq!(azure.headers.len(), 1);
    assert_eq!(header(&azure, "api-key"), Some("az-1"));
}

/// The catalog decides the style, not the API — which is the whole point for a host that
/// re-serves someone else's wire format.
#[tokio::test]
async fn a_stored_key_takes_the_style_its_provider_asked_for() {
    let (_dir, store) = store();
    store.put("foundry-anthropic", &Credential::api_key("fk-stored")).unwrap();
    let source = StoreAuth::new(store, "foundry-anthropic", ApiKind::AnthropicMessages);
    let auth = source.request_auth().await.unwrap();
    assert_eq!(header(&auth, "x-api-key"), Some("fk-stored"));
    assert_eq!(header(&auth, "api-key"), Some("fk-stored"));
}

/// A Microsoft Entra ID token is a bearer token over the same credential machinery, so the
/// override is what makes keyless auth a one-line config change rather than a feature.
#[tokio::test]
async fn a_configured_style_overrides_the_catalog() {
    let (_dir, store) = store();
    store.put("foundry-anthropic", &Credential::api_key("!echo entra-token")).unwrap();
    let source = StoreAuth::new(store, "foundry-anthropic", ApiKind::AnthropicMessages)
        .with_key_style(Some(KeyStyle::Bearer));
    let auth = source.request_auth().await.unwrap();
    assert_eq!(header(&auth, "authorization"), Some("Bearer entra-token"));
    assert!(header(&auth, "x-api-key").is_none());
}

#[test]
fn key_styles_parse_by_the_header_a_401_shows_you() {
    assert_eq!(KeyStyle::parse("bearer"), Some(KeyStyle::Bearer));
    assert_eq!(KeyStyle::parse("x-api-key"), Some(KeyStyle::AnthropicKey));
    assert_eq!(KeyStyle::parse("  API-KEY "), Some(KeyStyle::AzureKey));
    assert_eq!(KeyStyle::parse("foundry"), Some(KeyStyle::FoundryKey));
    assert_eq!(KeyStyle::parse("x_api_key"), Some(KeyStyle::AnthropicKey));
    assert_eq!(KeyStyle::parse("telepathy"), None);
}

#[tokio::test]
async fn a_stored_key_is_used_and_beats_the_environment() {
    let (_dir, store) = store();
    store.put("openai", &Credential::api_key("sk-stored")).unwrap();
    let source = StoreAuth::new(store, "openai", ApiKind::OpenaiCompletions);
    let auth = source.request_auth().await.unwrap();
    assert_eq!(header(&auth, "authorization"), Some("Bearer sk-stored"));
}

/// `echo`, not `printf`: the spec is run through whatever shell the platform has, and on a
/// Windows box with no Git for Windows that is `cmd`, which has `echo` and not `printf`. The
/// resolver trims, so the trailing `\r\n` cmd adds costs nothing.
#[tokio::test]
async fn a_command_key_spec_is_resolved_at_request_time() {
    let (_dir, store) = store();
    store
        .put(
            "openai",
            &Credential::ApiKey(ApiKeyCredential {
                key: ApiKey::new("!echo sk-from-command"),
                ..Default::default()
            }),
        )
        .unwrap();
    let source = StoreAuth::new(store, "openai", ApiKind::OpenaiCompletions);
    let auth = source.request_auth().await.unwrap();
    assert_eq!(header(&auth, "authorization"), Some("Bearer sk-from-command"));
}

/// A key command that hangs is stopped at its deadline and says why, rather than holding the turn
/// (and the runtime thread under it) for as long as it likes.
#[cfg(unix)]
#[tokio::test]
async fn a_hanging_key_command_does_not_hold_the_turn() {
    let (_dir, store) = store();
    store.put("openai", &Credential::api_key("!sleep 10")).unwrap();
    let source = StoreAuth::new(store, "openai", ApiKind::OpenaiCompletions)
        .with_command_deadline(std::time::Duration::from_millis(300));
    let started = std::time::Instant::now();
    let error = source.request_auth().await.unwrap_err().to_string();
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{error}");
    assert!(error.contains("did not finish"), "{error}");
}

/// A local server needs no key, so a configured key command that fails still leaves it reachable.
#[tokio::test]
async fn a_keyless_provider_survives_a_failing_fallback_command() {
    let (_dir, store) = store();
    let source = StoreAuth::new(store, "openai", ApiKind::OpenaiCompletions)
        .with_fallback(Some(ApiKey::new("!exit 1")))
        .keyless(true);
    assert!(source.request_auth().await.unwrap().is_empty());
}

#[tokio::test]
async fn an_override_beats_a_stored_credential() {
    let (_dir, store) = store();
    store.put("openai", &Credential::api_key("sk-stored")).unwrap();
    let source = StoreAuth::new(store, "openai", ApiKind::OpenaiCompletions)
        .with_override(Some(ApiKey::new("sk-cli")));
    let auth = source.request_auth().await.unwrap();
    assert_eq!(header(&auth, "authorization"), Some("Bearer sk-cli"));
}

/// Signing in cannot rescue a key that outranks the store, so a failing override names itself
/// rather than sending the reader to `luxe auth login`.
#[tokio::test]
async fn a_failing_override_says_it_was_the_key_given_for_this_run() {
    let (_dir, store) = store();
    let source = StoreAuth::new(store, "openai", ApiKind::OpenaiCompletions)
        .with_override(Some(ApiKey::new("!exit 1")));
    let error = source.request_auth().await.unwrap_err().to_string();
    assert!(error.contains("the key given for 'openai' on this run"), "{error}");
    assert!(!error.contains("auth login"), "{error}");
}

/// An override that expands to nothing leaves the stored sign-in in charge, for the account
/// details read with it as well as for the request.
#[tokio::test]
async fn an_override_that_expands_to_nothing_leaves_the_stored_sign_in() {
    let (_dir, store) = store();
    let sign_in = OAuthCredential {
        access: ApiKey::new("oauth-access"),
        expires_at_ms: now_ms() + 86_400_000,
        ..Default::default()
    };
    store.put("openai", &Credential::OAuth(sign_in)).unwrap();
    let source = StoreAuth::new(store, "openai", ApiKind::OpenaiCompletions)
        .with_override(Some(ApiKey::new("${LUXE_TEST_UNSET_OVERRIDE_KEY}")));
    let auth = source.request_auth().await.unwrap();
    assert_eq!(header(&auth, "authorization"), Some("Bearer oauth-access"));
    assert!(source.oauth_credential().await.is_some());
}

#[tokio::test]
async fn a_config_fallback_applies_only_when_nothing_is_stored() {
    let (_dir, store) = store();
    let source = StoreAuth::new(store.clone(), "openai", ApiKind::OpenaiCompletions)
        .with_fallback(Some(ApiKey::new("sk-config")));
    assert_eq!(
        header(&source.request_auth().await.unwrap(), "authorization"),
        Some("Bearer sk-config")
    );

    store.put("openai", &Credential::api_key("sk-stored")).unwrap();
    assert_eq!(
        header(&source.request_auth().await.unwrap(), "authorization"),
        Some("Bearer sk-stored"),
        "a stored credential outranks config"
    );
}

#[tokio::test]
async fn a_missing_credential_says_what_to_run() {
    let (_dir, store) = store();
    let source = StoreAuth::new(store, "anthropic", ApiKind::AnthropicMessages);
    let error = source.request_auth().await.expect_err("no credential");
    assert!(matches!(error, ProviderError::Auth(_)), "must classify as auth: {error:?}");
    let message = error.to_string();
    assert!(message.contains("luxe auth login anthropic"), "unhelpful: {message}");
    assert!(message.contains("/login anthropic"), "unhelpful: {message}");
}

#[tokio::test]
async fn a_keyless_local_server_needs_nothing() {
    let (_dir, store) = store();
    let source = StoreAuth::new(store, "lmstudio", ApiKind::OpenaiCompletions);
    assert!(source.request_auth().await.unwrap().is_empty());
}

#[tokio::test]
async fn copilot_headers_and_per_account_base_url_are_derived() {
    let (_dir, store) = store();
    store
        .put(
            "github-copilot",
            &Credential::OAuth(OAuthCredential {
                access: ApiKey::new("tid=x;proxy-ep=proxy.business.githubcopilot.com;exp=1"),
                refresh: ApiKey::new("gho_x"),
                // Far future so no refresh is attempted (which would need the network).
                expires_at_ms: now_ms() + 86_400_000,
                ..Default::default()
            }),
        )
        .unwrap();
    let source = StoreAuth::new(store, "github-copilot", ApiKind::OpenaiCompletions);
    let auth = source.request_auth().await.unwrap();
    assert!(header(&auth, "authorization").unwrap().starts_with("Bearer tid=x"));
    assert_eq!(header(&auth, "Copilot-Integration-Id"), Some("vscode-chat"));
    assert_eq!(
        auth.base_url.as_deref(),
        Some("https://api.business.githubcopilot.com"),
        "proxy-ep decides the endpoint for this account"
    );
}

#[tokio::test]
async fn codex_headers_carry_the_account_id() {
    let (_dir, store) = store();
    store
        .put(
            "openai-codex",
            &Credential::OAuth(OAuthCredential {
                access: ApiKey::new("header.payload.sig"),
                refresh: ApiKey::new("r"),
                expires_at_ms: now_ms() + 86_400_000,
                account: Some("acct-123".into()),
                ..Default::default()
            }),
        )
        .unwrap();
    let source = StoreAuth::new(store, "openai-codex", ApiKind::OpenaiCodex);
    let auth = source.request_auth().await.unwrap();
    assert_eq!(header(&auth, "chatgpt-account-id"), Some("acct-123"));
    assert_eq!(header(&auth, "originator"), Some("luxe"));
}

#[test]
fn debug_output_never_carries_a_header_value() {
    let auth = RequestAuth::bearer(&ApiKey::new("sk-super-secret"));
    let rendered = format!("{auth:?}");
    assert!(!rendered.contains("sk-super-secret"), "leaked: {rendered}");
}

/// A key that reaches a request header is remembered, so a tool printing it later is scrubbed even
/// when it has no shape any pattern knows.
#[test]
fn a_key_put_on_a_request_is_remembered_for_redaction() {
    RequestAuth::bearer(&ApiKey::new("bearer-plain-key-4417"));
    RequestAuth::anthropic_key(&ApiKey::new("anthropic-plain-key-4417"));
    RequestAuth::azure_key(&ApiKey::new("azure-plain-key-4417"));
    RequestAuth::foundry_key(&ApiKey::new("foundry-plain-key-4417"));
    for key in ["bearer", "anthropic", "azure", "foundry"] {
        let literal = format!("{key}-plain-key-4417");
        let seen = luxe_core::redact::process_secrets().redact(&format!("seen {literal}"));
        assert_eq!(seen, "seen [REDACTED]", "{key}");
    }
    // a header that is not a credential is not remembered
    let _ = RequestAuth::none().with_header("anthropic-version", "2023-06-01");
    assert!(luxe_core::redact::process_secrets().redact("2023-06-01").contains("2023-06-01"));
}
