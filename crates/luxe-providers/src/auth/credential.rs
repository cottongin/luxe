//! Credentials — the typed contents of one `auth.json` entry.
//!
//! Serialization is hand-written rather than derived, deliberately: [`crate::config::ApiKey`]
//! serializes to `[REDACTED]` as a fail-safe, so a derived `Serialize` on a credential would write
//! placeholders into the user's auth file. [`Credential::to_json`] is the one place allowed to put a
//! secret on disk, and it is the only path the store uses.

use std::collections::BTreeMap;

use luxe_core::auth::{CredentialKind, StoredCredentialInfo};
use serde_json::{Map, Value};

use crate::config::ApiKey;

/// One provider's stored credential.
#[derive(Debug, Clone)]
pub enum Credential {
    /// A long-lived API key (possibly a `$ENV` / `!command` spec, resolved at request time).
    ApiKey(ApiKeyCredential),
    /// An OAuth token pair that expires and refreshes.
    OAuth(OAuthCredential),
}

/// A stored API key, plus any provider-scoped configuration values that travel with it.
#[derive(Debug, Clone, Default)]
pub struct ApiKeyCredential {
    /// The key, verbatim as stored. `$ENV`, `${ENV}` and `!command` forms are resolved on use, not
    /// here — status output must never execute a user's command to render a table.
    pub key: ApiKey,
    /// Provider-scoped values (deployment names, account ids) consulted before the process
    /// environment. Kept so a provider needing more than a key doesn't need a second config file.
    pub env: BTreeMap<String, String>,
}

/// A stored OAuth credential.
#[derive(Debug, Clone, Default)]
pub struct OAuthCredential {
    /// Bearer token used on requests.
    pub access: ApiKey,
    /// Token used to mint a new access token. For GitHub Copilot this is the long-lived GitHub
    /// token and "refresh" means re-exchanging it.
    pub refresh: ApiKey,
    /// Unix-millisecond expiry of `access`.
    pub expires_at_ms: u64,
    /// Account label the provider gave us (ChatGPT account id, GitHub login).
    pub account: Option<String>,
    /// A per-credential endpoint, when the provider hands one out (Copilot's `proxy-ep`).
    pub endpoint: Option<String>,
    /// GitHub Enterprise domain, when the login targeted one.
    pub enterprise_domain: Option<String>,
    /// Model ids this account may actually use, when the provider enumerates them.
    pub available_models: Vec<String>,
}

impl Credential {
    /// Which kind of credential this is.
    pub fn kind(&self) -> CredentialKind {
        match self {
            Self::ApiKey(_) => CredentialKind::ApiKey,
            Self::OAuth(_) => CredentialKind::OAuth,
        }
    }

    /// Non-secret metadata for status output.
    pub fn info(&self) -> StoredCredentialInfo {
        match self {
            Self::ApiKey(_) => StoredCredentialInfo {
                kind: CredentialKind::ApiKey,
                expires_at_ms: None,
                account: None,
            },
            Self::OAuth(oauth) => StoredCredentialInfo {
                kind: CredentialKind::OAuth,
                expires_at_ms: Some(oauth.expires_at_ms),
                account: oauth.account.clone(),
            },
        }
    }

    /// An API-key credential from a bare key.
    pub fn api_key(key: impl Into<String>) -> Self {
        Self::ApiKey(ApiKeyCredential { key: ApiKey::new(key.into()), env: BTreeMap::new() })
    }

    /// The stored OAuth credential, when this is one.
    pub fn as_oauth(&self) -> Option<&OAuthCredential> {
        match self {
            Self::OAuth(oauth) => Some(oauth),
            Self::ApiKey(_) => None,
        }
    }

    /// Serialize for the auth file. The only sanctioned path from a secret to disk.
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        match self {
            Self::ApiKey(api) => {
                map.insert("type".into(), Value::String("api_key".into()));
                map.insert("key".into(), Value::String(api.key.expose().to_string()));
                if !api.env.is_empty() {
                    let env: Map<String, Value> = api
                        .env
                        .iter()
                        .map(|(name, value)| (name.clone(), Value::String(value.clone())))
                        .collect();
                    map.insert("env".into(), Value::Object(env));
                }
            }
            Self::OAuth(oauth) => {
                map.insert("type".into(), Value::String("oauth".into()));
                map.insert("access".into(), Value::String(oauth.access.expose().to_string()));
                map.insert("refresh".into(), Value::String(oauth.refresh.expose().to_string()));
                map.insert("expires".into(), Value::Number(oauth.expires_at_ms.into()));
                insert_opt(&mut map, "account", oauth.account.as_deref());
                insert_opt(&mut map, "endpoint", oauth.endpoint.as_deref());
                insert_opt(&mut map, "enterprise_domain", oauth.enterprise_domain.as_deref());
                if !oauth.available_models.is_empty() {
                    map.insert(
                        "available_models".into(),
                        Value::Array(
                            oauth
                                .available_models
                                .iter()
                                .map(|id| Value::String(id.clone()))
                                .collect(),
                        ),
                    );
                }
            }
        }
        Value::Object(map)
    }

    /// Parse one auth-file entry, returning `None` for anything that isn't a credential.
    ///
    /// Tolerant on purpose: an entry missing its `type` but carrying a `key` is an api-key
    /// credential (that is how a hand-written file usually looks), and an unrecognized shape is
    /// skipped rather than allowed to sink the whole file.
    pub fn from_json(value: &Value) -> Option<Self> {
        let map = value.as_object()?;
        let tag = map.get("type").and_then(Value::as_str).map(str::to_ascii_lowercase);
        match tag.as_deref() {
            Some("oauth") => {
                let access = map.get("access").and_then(Value::as_str)?;
                let refresh = map.get("refresh").and_then(Value::as_str).unwrap_or_default();
                // the refresh token never rides a header, so this is where it becomes known
                for token in [access, refresh] {
                    luxe_core::redact::process_secrets().add(token);
                }
                Some(Self::OAuth(OAuthCredential {
                    access: ApiKey::new(access),
                    refresh: ApiKey::new(refresh),
                    expires_at_ms: map.get("expires").and_then(Value::as_u64).unwrap_or(0),
                    account: string_field(map, "account"),
                    endpoint: string_field(map, "endpoint"),
                    enterprise_domain: string_field(map, "enterprise_domain"),
                    available_models: map
                        .get("available_models")
                        .and_then(Value::as_array)
                        .map(|ids| {
                            ids.iter().filter_map(Value::as_str).map(str::to_string).collect()
                        })
                        .unwrap_or_default(),
                }))
            }
            Some("api_key") | None => {
                let key = map.get("key").and_then(Value::as_str)?;
                // a `$VAR` or `!command` spec is not the key; it is remembered once resolved
                if !key.starts_with(['$', '!']) {
                    luxe_core::redact::process_secrets().add(key);
                }
                let env = map
                    .get("env")
                    .and_then(Value::as_object)
                    .map(|env| {
                        env.iter()
                            .filter_map(|(name, value)| {
                                value.as_str().map(|value| (name.clone(), value.to_string()))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Some(Self::ApiKey(ApiKeyCredential { key: ApiKey::new(key), env }))
            }
            Some(_) => None,
        }
    }
}

fn insert_opt(map: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        map.insert(key.to_string(), Value::String(value.to_string()));
    }
}

fn string_field(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(Value::as_str).filter(|value| !value.is_empty()).map(str::to_string)
}

/// Unix milliseconds now — the clock every expiry is measured against.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both tokens of a stored OAuth login, and a stored key, are remembered on load: the refresh
    /// token never rides a header, and `cat auth.json` must not hand any of them to the model.
    #[test]
    fn a_loaded_oauth_login_is_remembered_for_redaction() {
        let json = serde_json::json!({"type": "oauth", "access": "access-plain-8812",
            "refresh": "refresh-plain-8812", "expires": 1});
        Credential::from_json(&json).unwrap();
        let seen =
            luxe_core::redact::process_secrets().redact("access-plain-8812 refresh-plain-8812");
        assert_eq!(seen, "[REDACTED] [REDACTED]");
        // a stored key is too, before any request uses it; a spec for one is not the key
        Credential::from_json(&serde_json::json!({"type": "api_key", "key": "stored-plain-8812"}));
        Credential::from_json(&serde_json::json!({"type": "api_key", "key": "$OPENAI_API_KEY"}));
        let seen = luxe_core::redact::process_secrets().redact("stored-plain-8812 $OPENAI_API_KEY");
        assert_eq!(seen, "[REDACTED] $OPENAI_API_KEY");
    }

    #[test]
    fn api_key_round_trips_with_the_real_secret() {
        let mut env = BTreeMap::new();
        env.insert("AZURE_DEPLOYMENT".to_string(), "gpt5".to_string());
        let credential =
            Credential::ApiKey(ApiKeyCredential { key: ApiKey::new("sk-live"), env: env.clone() });

        let json = credential.to_json();
        assert_eq!(json["type"], "api_key");
        assert_eq!(json["key"], "sk-live", "the store must write the real key, not [REDACTED]");
        assert_eq!(json["env"]["AZURE_DEPLOYMENT"], "gpt5");

        let back = Credential::from_json(&json).expect("parses back");
        match back {
            Credential::ApiKey(api) => {
                assert_eq!(api.key.expose(), "sk-live");
                assert_eq!(api.env, env);
            }
            Credential::OAuth(_) => panic!("kind changed"),
        }
    }

    #[test]
    fn oauth_round_trips_every_field() {
        let credential = Credential::OAuth(OAuthCredential {
            access: ApiKey::new("tid=abc;proxy-ep=proxy.individual.githubcopilot.com"),
            refresh: ApiKey::new("gho_refresh"),
            expires_at_ms: 1_785_964_482_483,
            account: Some("octocat".into()),
            endpoint: Some("https://api.individual.githubcopilot.com".into()),
            enterprise_domain: None,
            available_models: vec!["gpt-5.6-sol".into(), "claude-sonnet-5".into()],
        });
        let json = credential.to_json();
        assert_eq!(json["expires"], 1_785_964_482_483u64);
        assert!(json.get("enterprise_domain").is_none(), "absent fields stay absent");

        let back = Credential::from_json(&json).expect("parses back");
        let oauth = back.as_oauth().expect("still oauth");
        assert_eq!(oauth.refresh.expose(), "gho_refresh");
        assert_eq!(oauth.account.as_deref(), Some("octocat"));
        assert_eq!(oauth.available_models.len(), 2);
        assert_eq!(back.info().kind, CredentialKind::OAuth);
        assert_eq!(back.info().expires_at_ms, Some(1_785_964_482_483));
    }

    #[test]
    fn a_bare_key_entry_parses_as_an_api_key() {
        // How a hand-written file usually looks: no `type` tag at all.
        let credential = Credential::from_json(&serde_json::json!({ "key": "sk-hand-written" }))
            .expect("tolerated");
        assert_eq!(credential.kind(), CredentialKind::ApiKey);
    }

    #[test]
    fn unrecognizable_entries_are_skipped_not_guessed() {
        assert!(Credential::from_json(&serde_json::json!({ "type": "webauthn" })).is_none());
        assert!(Credential::from_json(&serde_json::json!({ "type": "oauth" })).is_none());
        assert!(Credential::from_json(&serde_json::json!({ "unrelated": true })).is_none());
        assert!(Credential::from_json(&serde_json::json!("a string")).is_none());
    }

    #[test]
    fn debug_never_prints_a_secret() {
        let credential = Credential::api_key("sk-super-secret");
        let rendered = format!("{credential:?}");
        assert!(!rendered.contains("sk-super-secret"), "leaked in Debug: {rendered}");
        assert!(rendered.contains("REDACTED"));
    }
}
