use super::*;

/// The status read names the servers with a token, falls back to the old store, and leaves
/// both files exactly as it found them.
#[test]
fn signed_in_ids_reads_names_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (path, legacy) = (dir.path().join("state.json"), dir.path().join("config.json"));
    let store = TokenStore::migrating(path.clone(), legacy.clone());
    assert!(store.signed_in_ids().is_empty(), "no store: nobody is signed in");

    std::fs::write(&legacy, r#"{"asana": {"access_token": "a"}, "linear": {"access_token": "b"}}"#)
        .unwrap();
    let ids = store.signed_in_ids();
    assert_eq!(ids, ["asana", "linear"].map(String::from).into_iter().collect());
    assert!(!path.exists() && legacy.exists(), "reading a status migrated the store");

    std::fs::write(&path, r#"{"github": {"access_token": "c"}}"#).unwrap();
    assert_eq!(store.signed_in_ids(), ["github"].map(String::from).into_iter().collect());

    std::fs::write(&path, "not json").unwrap();
    assert_eq!(store.signed_in_ids().len(), 2, "an unreadable store falls back, like `get`");
}

#[test]
fn round_trips_and_sets_owner_only_perms() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::new(dir.path().join("mcp-auth.json"));
    assert!(store.get("srv").is_none());

    store
        .put(
            "srv",
            StoredToken {
                access_token: "tok".into(),
                refresh_token: Some("refresh".into()),
                expires_at_secs: None,
                client_id: Some("cid".into()),
                client_secret: None,
                token_endpoint: None,
                resource: None,
            },
        )
        .unwrap();
    let got = store.get("srv").expect("token present");
    assert_eq!(got.access_token, "tok");
    assert_eq!(got.client_id.as_deref(), Some("cid"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode =
            std::fs::metadata(dir.path().join("mcp-auth.json")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "auth file must be owner-only");
    }

    store.remove("srv").unwrap();
    assert!(store.get("srv").is_none());
}

#[test]
fn expiry_uses_safety_margin() {
    let mut t = StoredToken {
        access_token: "x".into(),
        refresh_token: None,
        expires_at_secs: None,
        client_id: None,
        client_secret: None,
        token_endpoint: None,
        resource: None,
    };
    assert!(!t.is_expired(), "no expiry = not expired");
    // The clock is read here independently: reading it through `now_secs` would move the test
    // with the code it is testing
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    t.expires_at_secs = Some(now + 10); // within 60s margin
    assert!(t.is_expired());
    t.expires_at_secs = Some(now + 3600);
    assert!(!t.is_expired());
    t.expires_at_secs = Some(now.saturating_sub(10));
    assert!(t.is_expired(), "in the past is expired");
}

fn token(access: &str) -> StoredToken {
    StoredToken {
        access_token: access.into(),
        refresh_token: Some("refresh-secret".into()),
        expires_at_secs: None,
        client_id: Some("cid".into()),
        client_secret: Some("issued-secret".into()),
        token_endpoint: Some("https://app.asana.com/-/oauth_token".into()),
        resource: Some("https://mcp.asana.com/v2/mcp".into()),
    }
}

/// A store left in the config directory is read once, lands in the state directory, and the
/// old file is gone — after the write, so nothing is lost if it fails.
#[test]
fn a_store_in_the_old_place_moves_on_first_read() {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("config").join("luxe").join("mcp-auth.json");
    let new = dir.path().join("state").join("luxe").join("mcp-auth.json");
    TokenStore::new(old.clone()).put("asana", token("at-old")).unwrap();

    let store = TokenStore::migrating(new.clone(), old.clone());
    assert_eq!(store.get("asana").map(|t| t.access_token.clone()).as_deref(), Some("at-old"));
    assert!(new.exists(), "written to the state directory");
    assert!(!old.exists(), "and the config directory no longer holds tokens");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&new).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert_eq!(store.get("asana").map(|t| t.access_token.clone()).as_deref(), Some("at-old"));
}

/// With both present the new store wins, and the old one is left alone rather than merged or
/// deleted: it is somebody's file, and it is no longer read.
#[test]
fn the_new_store_wins_over_the_old() {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("old.json");
    let new = dir.path().join("new.json");
    TokenStore::new(old.clone()).put("asana", token("at-old")).unwrap();
    TokenStore::new(new.clone()).put("asana", token("at-new")).unwrap();
    let store = TokenStore::migrating(new, old.clone());
    assert_eq!(store.get("asana").map(|t| t.access_token.clone()).as_deref(), Some("at-new"));
    assert!(old.exists());
}

/// If the new store cannot be written the old one is read in place and kept.
#[cfg(unix)]
#[test]
fn a_failed_move_keeps_the_old_store() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("old.json");
    TokenStore::new(old.clone()).put("asana", token("at-old")).unwrap();
    let locked = dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
    let store = TokenStore::migrating(locked.join("new.json"), old.clone());
    let read = store.get("asana").map(|t| t.access_token.clone());
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    // Running as root writes anyway; the promise is only that nothing is lost
    assert_eq!(read.as_deref(), Some("at-old"));
    assert!(old.exists() || locked.join("new.json").exists(), "the tokens are somewhere");
}

#[test]
fn debug_never_shows_a_token_or_a_secret() {
    let shown = format!("{:?}", token("at-live"));
    for secret in ["at-live", "refresh-secret", "issued-secret"] {
        assert!(!shown.contains(secret), "{secret} leaked: {shown}");
    }
    assert!(shown.contains("[redacted]") && shown.contains("cid"), "{shown}");
    let empty = StoredToken::default();
    assert!(format!("{empty:?}").contains("access_token: \"none\""));
}

/// The default store is in the state directory, and takes over the one in the config directory.
#[test]
fn the_default_store_is_state_not_configuration() {
    let store = TokenStore::default_path();
    let path = luxe_core::paths::display(store.path());
    assert!(path.ends_with("luxe/mcp-auth.json"), "{path}");
    assert_eq!(store.path(), luxe_core::user_dirs::luxe_state_dir().join("mcp-auth.json"));
    assert_eq!(
        store.legacy.as_deref(),
        Some(luxe_core::user_dirs::luxe_config_dir().join("mcp-auth.json").as_path())
    );
}

/// Two servers signing in at once keep both tokens: each write reads, changes and writes under the
/// file's lock, so neither overwrites the other with what it read before.
#[test]
fn concurrent_sign_ins_keep_every_token() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp-auth.json");
    let writers: Vec<_> = (0..16)
        .map(|n| {
            let path = path.clone();
            std::thread::spawn(move || {
                TokenStore::new(path).put(&format!("server-{n}"), token("at")).unwrap()
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    let store = TokenStore::new(path);
    for n in 0..16 {
        assert!(store.get(&format!("server-{n}")).is_some(), "server-{n} was lost");
    }
}

/// A store that does not parse is not emptied by the next write: it is moved to `.bad` for the
/// reader, and the new store holds only the new token, since nothing could be read from the old.
#[test]
fn an_unreadable_store_is_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp-auth.json");
    std::fs::write(&path, "{ \"asana\": { not json").unwrap();
    TokenStore::new(path.clone()).put("linear", token("at-linear")).unwrap();
    let kept = std::fs::read_to_string(dir.path().join("mcp-auth.json.bad")).unwrap();
    assert_eq!(kept, "{ \"asana\": { not json");
    let store = TokenStore::new(path);
    assert_eq!(store.get("linear").map(|t| t.access_token.clone()).as_deref(), Some("at-linear"));
    assert_eq!(store.signed_in_ids(), ["linear".to_string()].into_iter().collect());
}

/// One entry an older or newer Luxe wrote differently does not hide the others, and survives a
/// write for another server.
#[test]
fn an_entry_that_does_not_parse_hides_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp-auth.json");
    let store = TokenStore::new(path.clone());
    store.put("asana", token("at-asana")).unwrap();
    let mut raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    raw["odd"] = serde_json::json!({"no_access_token": true});
    std::fs::write(&path, raw.to_string()).unwrap();
    assert_eq!(store.get("asana").map(|t| t.access_token.clone()).as_deref(), Some("at-asana"));
    store.put("linear", token("at-linear")).unwrap();
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(raw["odd"], serde_json::json!({"no_access_token": true}), "kept as written");
}
