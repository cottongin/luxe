#![allow(clippy::unwrap_used)]

use super::*;
use crate::auth::credential::{ApiKeyCredential, OAuthCredential};
use crate::config::ApiKey;
use serde_json::Value;

fn store() -> (tempfile::TempDir, AuthStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = AuthStore::new(dir.path().join("auth.json"));
    (dir, store)
}

#[test]
fn writes_and_reads_a_credential() {
    let (_dir, store) = store();
    assert!(store.is_empty());
    assert!(store.credential("openai").is_none());

    store.put("openai", &Credential::api_key("sk-test")).unwrap();
    match store.credential("openai").expect("stored") {
        Credential::ApiKey(api) => assert_eq!(api.key.expose(), "sk-test"),
        Credential::OAuth(_) => panic!("wrong kind"),
    }
    assert_eq!(store.summaries().len(), 1);
    assert!(store.remove("openai").unwrap());
    assert!(!store.remove("openai").unwrap(), "a second remove is a no-op");
}

#[cfg(unix)]
#[test]
fn the_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, store) = store();
    store.put("openai", &Credential::api_key("sk-test")).unwrap();
    let mode = std::fs::metadata(store.path()).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "credentials must not be group/world readable");
}

/// The one regression that would actually hurt someone: clobbering config they already had.
#[test]
fn legacy_config_and_unknown_keys_survive_a_write() {
    let (_dir, store) = store();
    std::fs::write(
        store.path(),
        r#"{
                "$schema": "https://luxe.dev/auth.schema.json",
                "providers": { "lmstudio": { "base_url": "http://localhost:1234/v1" } },
                "default_provider": "lmstudio",
                "default_model": "qwen3",
                "future_thing": { "type": "webauthn", "handle": "keep me" }
            }"#,
    )
    .unwrap();

    store.put("openai", &Credential::api_key("sk-new")).unwrap();

    let raw: Value = serde_json::from_str(&std::fs::read_to_string(store.path()).unwrap())
        .expect("still valid JSON");
    assert_eq!(raw["default_provider"], "lmstudio");
    assert_eq!(raw["providers"]["lmstudio"]["base_url"], "http://localhost:1234/v1");
    assert_eq!(raw["future_thing"]["handle"], "keep me", "unknown entries are preserved");
    assert_eq!(raw["openai"]["key"], "sk-new");

    // Reserved keys are config, never credentials; the unparseable entry is skipped, not eaten.
    assert_eq!(store.credentials().keys().collect::<Vec<_>>(), vec!["openai"]);
}

/// The nested legacy shape (`providers.<id>.api_key`) is still read by the config loader, so the
/// store must not treat it as a credential entry or offer to overwrite it.
#[test]
fn reserved_keys_are_not_addressable_as_providers() {
    let (_dir, store) = store();
    assert!(store.credential("providers").is_none());
    let error = store.put("models", &Credential::api_key("x")).expect_err("must refuse");
    assert!(format!("{error}").contains("reserved"), "got: {error}");
}

#[test]
fn a_malformed_file_fails_a_write_instead_of_erasing_it() {
    let (_dir, store) = store();
    std::fs::write(store.path(), "{ this is not json").unwrap();
    let error = store.put("openai", &Credential::api_key("sk")).expect_err("must refuse");
    assert!(format!("{error}").contains("not valid JSON"), "got: {error}");
    assert_eq!(std::fs::read_to_string(store.path()).unwrap(), "{ this is not json");
}

#[test]
fn modify_sees_the_current_value_and_can_delete() {
    let (_dir, store) = store();
    store
        .put(
            "github-copilot",
            &Credential::OAuth(OAuthCredential {
                access: ApiKey::new("old"),
                refresh: ApiKey::new("gho"),
                expires_at_ms: 1,
                ..Default::default()
            }),
        )
        .unwrap();

    let updated = store
        .modify("github-copilot", |current| {
            let mut oauth = current.as_ref().and_then(Credential::as_oauth).cloned().unwrap();
            assert_eq!(oauth.access.expose(), "old");
            oauth.access = ApiKey::new("fresh");
            oauth.expires_at_ms = 99;
            Ok(Some(Credential::OAuth(oauth)))
        })
        .unwrap();
    assert_eq!(updated.unwrap().as_oauth().unwrap().access.expose(), "fresh");
    assert_eq!(
        store.credential("github-copilot").unwrap().as_oauth().unwrap().refresh.expose(),
        "gho",
        "an untouched field survives the rewrite"
    );

    store.modify("github-copilot", |_| Ok(None)).unwrap();
    assert!(store.credential("github-copilot").is_none());
}

#[test]
fn creates_the_parent_directory() {
    let dir = tempfile::tempdir().unwrap();
    let store = AuthStore::new(dir.path().join("nested").join("deeper").join("auth.json"));
    store.put("openai", &Credential::api_key("sk")).unwrap();
    assert!(store.path().exists());
}

#[test]
fn env_scoped_values_round_trip() {
    let (_dir, store) = store();
    let mut env = BTreeMap::new();
    env.insert("CLOUDFLARE_ACCOUNT_ID".to_string(), "acct".to_string());
    store
        .put("cloudflare", &Credential::ApiKey(ApiKeyCredential { key: ApiKey::new("cf"), env }))
        .unwrap();
    match store.credential("cloudflare").unwrap() {
        Credential::ApiKey(api) => {
            assert_eq!(api.env.get("CLOUDFLARE_ACCOUNT_ID").map(String::as_str), Some("acct"));
        }
        Credential::OAuth(_) => panic!("wrong kind"),
    }
}

#[test]
fn successive_writes_release_the_lock() {
    let (_dir, store) = store();
    for index in 0..3 {
        store.put("openai", &Credential::api_key(format!("sk-{index}"))).unwrap();
    }
    assert!(!store.path().with_extension("lock").exists(), "lock must not leak");
}
