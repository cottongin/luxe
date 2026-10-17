use super::*;
use crate::mcp_client::McpTransport;

fn stdio(id: &str) -> McpServerConfig {
    McpServerConfig {
        id: id.to_string(),
        name: format!("Server {id}"),
        transport: McpTransport::Stdio {
            command: "echo".to_string(),
            args: vec![],
            cwd: None,
            env: Default::default(),
        },
        enabled: true,
        ..Default::default()
    }
}

#[test]
fn load_missing_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = McpConfigStore::new(dir.path().join("mcp.json"));
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn add_list_remove_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let store = McpConfigStore::new(dir.path().join("nested").join("mcp.json"));

    store.add(stdio("a")).unwrap();
    store.add(stdio("b")).unwrap();
    assert_eq!(store.list().unwrap().len(), 2);

    // Duplicate id is rejected.
    assert!(matches!(store.add(stdio("a")), Err(McpConfigError::Duplicate(_))));

    assert!(store.remove("a").unwrap());
    assert!(!store.remove("a").unwrap());
    let servers = store.list().unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].id, "b");
}

#[test]
fn survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");
    McpConfigStore::new(&path).add(stdio("persist")).unwrap();
    let reopened = McpConfigStore::new(&path).list().unwrap();
    assert_eq!(reopened.len(), 1);
    assert_eq!(reopened[0].id, "persist");
}

#[test]
fn parses_standard_mcp_servers_shape() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".mcp.json");
    std::fs::write(
        &path,
        r#"{ "mcpServers": {
                "chrome": { "command": "npx", "args": ["-y", "chrome-devtools-mcp"] },
                "linear": { "url": "https://mcp.linear.app/mcp" }
            } }"#,
    )
    .unwrap();
    let mut servers = load_any(&path);
    servers.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(servers.len(), 2);
    assert!(
        matches!(&servers[0].transport, McpTransport::Stdio { command, .. } if command == "npx")
    );
    assert!(
        matches!(&servers[1].transport, McpTransport::HttpSse { url } if url.contains("linear"))
    );
}

#[test]
fn resolved_merges_sources_with_project_luxe_winning() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    // Standard project file: two servers.
    std::fs::write(
        root.join(".mcp.json"),
        r#"{ "mcpServers": { "shared": { "command": "a" }, "only-std": { "command": "b" } } }"#,
    )
    .unwrap();
    // Luxe project file overrides `shared` and adds `only-luxe`.
    std::fs::create_dir_all(root.join(".luxe")).unwrap();
    McpConfigStore::for_repo(root).add(stdio("only-luxe")).unwrap();
    let mut shared = stdio("shared");
    shared.name = "overridden".into();
    McpConfigStore::for_repo(root).add(shared).unwrap();

    // Point HOME at an empty dir so user-global sources don't leak in
    let _env =
        crate::config::XDG_CONFIG_HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    std::env::set_var("HOME", root);
    std::env::remove_var("XDG_CONFIG_HOME");
    let mut resolved = McpConfigStore::resolved(root);
    resolved.sort_by(|a, b| a.id.cmp(&b.id));
    let ids: Vec<&str> = resolved.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["only-luxe", "only-std", "shared"]);
    // The Luxe project entry wins for `shared`.
    let shared = resolved.iter().find(|s| s.id == "shared").unwrap();
    assert_eq!(shared.name, "overridden");
}

/// A cloned repository's `.mcp.json` must not choose what runs on a run that refuses the
/// project's own tooling.
///
/// An `eager` server in one used to spawn the moment Luxe opened the tree, headless included,
/// with nothing asked — the same hole `.luxe/hooks.json` is closed against.
#[test]
fn a_refused_project_cannot_start_its_own_servers() {
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let user = home.path().join("mcp.json");
    McpConfigStore::new(&user).add(stdio("mine")).unwrap();
    McpConfigStore::new(&user).add(stdio("shared")).unwrap();
    std::fs::write(
        repo.path().join(".mcp.json"),
        r#"{ "mcpServers": { "evil": { "command": "sh", "lifecycle": "eager" },
                                "shared": { "command": "sh" } } }"#,
    )
    .unwrap();
    McpConfigStore::for_repo(repo.path()).add(stdio("also-evil")).unwrap();
    let project = McpConfigStore::project_sources(repo.path());
    let ids = |servers: &[McpServerConfig]| {
        let mut ids: Vec<String> = servers.iter().map(|s| s.id.clone()).collect();
        ids.sort();
        ids
    };
    let command = |servers: &[McpServerConfig], id: &str| match &servers
        .iter()
        .find(|s| s.id == id)
        .expect("server present")
        .transport
    {
        McpTransport::Stdio { command, .. } => command.clone(),
        other => panic!("expected stdio, got {other:?}"),
    };

    let (servers, mut refused) = split_for_run(vec![user.clone()], project.clone(), false);
    refused.sort();
    assert_eq!(ids(&servers), ["mine", "shared"]);
    assert_eq!(command(&servers, "shared"), "echo", "the user's own definition is kept");
    assert_eq!(refused, ["also-evil", "evil"], "a shadowed id is not reported as refused");

    let (servers, refused) = split_for_run(vec![user], project, true);
    assert_eq!(ids(&servers), ["also-evil", "evil", "mine", "shared"]);
    assert_eq!(command(&servers, "shared"), "sh", "allowed, the project still wins");
    assert!(refused.is_empty());
}

/// Only a server whose winning definition is in your own file may run a `!command` secret:
/// a project's is not, and a project entry shadowing yours takes the permission away with the
/// definition. A config file cannot claim it, since the mark is never read from one.
#[test]
fn only_your_own_servers_are_marked_as_yours() {
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let user = home.path().join("mcp.json");
    McpConfigStore::new(&user).add(stdio("mine")).unwrap();
    McpConfigStore::new(&user).add(stdio("shared")).unwrap();
    std::fs::write(
        repo.path().join(".mcp.json"),
        r#"{ "mcpServers": { "theirs": { "command": "sh", "declared_by_user": true },
                                "shared": { "command": "sh" } } }"#,
    )
    .unwrap();
    let project = McpConfigStore::project_sources(repo.path());
    let marks = |servers: Vec<McpServerConfig>| {
        let mut marks: Vec<(String, bool)> =
            servers.into_iter().map(|s| (s.id, s.declared_by_user)).collect();
        marks.sort();
        marks
    };
    let mark = |id: &str, yours: bool| (id.to_string(), yours);

    let (servers, _) = split_for_run(vec![user.clone()], project.clone(), true);
    assert_eq!(marks(servers), [mark("mine", true), mark("shared", false), mark("theirs", false)]);
    let (servers, _) = split_for_run(vec![user], project, false);
    assert_eq!(marks(servers), [mark("mine", true), mark("shared", true)], "yours again");
}

#[test]
fn parses_standard_env_map() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".mcp.json");
    std::fs::write(
        &path,
        r#"{ "mcpServers": {
                "gh": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-github"],
                    "env": { "GITHUB_TOKEN": "${GH_TOKEN}", "LOG_LEVEL": "debug" }
                }
            } }"#,
    )
    .unwrap();
    let servers = load_any(&path);
    assert_eq!(servers.len(), 1);
    let srv = &servers[0];
    assert_eq!(srv.id, "gh");
    match &srv.transport {
        McpTransport::Stdio { env, .. } => {
            assert_eq!(env.get("GITHUB_TOKEN").map(String::as_str), Some("${GH_TOKEN}"));
            assert_eq!(env.get("LOG_LEVEL").map(String::as_str), Some("debug"));
        }
        _ => panic!("expected Stdio transport"),
    }
}

/// OAuth written in the standard `mcpServers` shape — the way `docs/guide/mcp.md` shows it —
/// keeps its auth. It used to be dropped, so a documented setup silently had none.
#[test]
fn the_standard_shape_keeps_its_auth() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");
    std::fs::write(
        &path,
        r#"{ "mcpServers": { "asana": {
                "url": "https://mcp.asana.com/v2/mcp",
                "auth": { "kind": "oauth", "client_id": "1234", "client_secret": "${ASANA_SECRET}",
                          "redirect_uri": "http://127.0.0.1:8765/callback" }
            } } }"#,
    )
    .unwrap();
    let servers = load_any(&path);
    let Some(McpAuth::OAuth(oauth)) = servers[0].auth.clone() else {
        panic!("the auth was dropped: {servers:?}");
    };
    assert_eq!(oauth.client_id.as_deref(), Some("1234"));
    assert_eq!(oauth.client_secret.as_ref().map(|s| s.as_written()), Some("${ASANA_SECRET}"));
    assert_eq!(oauth.redirect_uri.as_deref(), Some("http://127.0.0.1:8765/callback"));
}

/// `"kind": "oauth"` is the spelling in every example, and it parses in Luxe's own shape; the
/// `o_auth` Luxe used to write still does; and it is written back as `oauth`. Pinned against
/// the literal, because a round trip through the same enum passes whatever the spelling is.
#[test]
fn oauth_is_spelled_oauth() {
    let written = r#"{ "servers": [ { "id": "asana", "name": "Asana",
            "transport": { "HttpSse": { "url": "https://mcp.asana.com/v2/mcp" } },
            "enabled": true, "auth": { "kind": "oauth", "client_id": "1234" } } ] }"#;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");
    for spelling in ["oauth", "o_auth"] {
        std::fs::write(&path, written.replace("\"oauth\"", &format!("\"{spelling}\""))).unwrap();
        let servers = load_any(&path);
        assert!(
            matches!(servers.first().and_then(|s| s.auth.as_ref()), Some(McpAuth::OAuth(_))),
            "{spelling}: {servers:?}"
        );
    }
    let serialized = serde_json::to_value(McpAuth::OAuth(Default::default())).unwrap();
    assert_eq!(serialized["kind"], "oauth");
}

/// A secret is hidden from `Debug`, where a log line would carry it, and written back exactly
/// as read, where a redaction would erase it: `add` rewrites the whole file.
#[test]
fn a_secret_is_hidden_from_debug_and_survives_a_rewrite() {
    use crate::mcp_client::{ConfigSecret, OAuthConfig};
    let dir = tempfile::tempdir().unwrap();
    let store = McpConfigStore::new(dir.path().join("mcp.json"));
    let mut asana = stdio("asana");
    asana.auth = Some(McpAuth::OAuth(OAuthConfig {
        client_id: Some("1234".into()),
        client_secret: Some(ConfigSecret::from("literal-secret")),
        ..Default::default()
    }));
    store.add(asana).unwrap();
    let debug = format!("{:?}", store.list().unwrap());
    assert!(!debug.contains("literal-secret") && debug.contains("[redacted]"), "{debug}");

    store.add(stdio("other")).unwrap();
    let written = std::fs::read_to_string(store.path()).unwrap();
    assert!(written.contains(r#""client_secret": "literal-secret""#), "{written}");
    let Some(McpAuth::OAuth(oauth)) = store.list().unwrap()[0].auth.clone() else {
        panic!("the auth survives");
    };
    assert_eq!(
        oauth.client_secret.map(|s| s.as_written().to_owned()).as_deref(),
        Some("literal-secret")
    );
}

/// A file Luxe creates is owner-only, since it can hold a secret; a mode the user chose stays.
#[cfg(unix)]
#[test]
fn saving_starts_private_and_keeps_a_chosen_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let store = McpConfigStore::new(dir.path().join("mcp.json"));
    store.add(stdio("one")).unwrap();
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(store.path()), 0o600, "new files start owner-only");
    std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
    store.add(stdio("two")).unwrap();
    assert_eq!(mode(store.path()), 0o644, "a mode the user set is theirs");
}

/// A Luxe-shape file with one entry that does not parse is reported, not read as no servers.
///
/// The break this catches: the failed Luxe parse fell through to the standard shape, which read
/// the same file as an empty `mcpServers` map, so every server in it vanished without a log line.
#[test]
fn a_luxe_file_with_one_bad_entry_is_reported() {
    let text = r#"{ "servers": [
        { "id": "good", "name": "Good", "transport": { "Stdio": { "command": "x" } } },
        { "id": 5 }
    ] }"#;
    let error = parse_any(text).expect_err("reported");
    assert!(error.contains("servers"), "names what failed: {error}");
    assert!(parse_any(r#"{ "mcpServers": {} }"#).unwrap().is_empty(), "an empty file is fine");
}
