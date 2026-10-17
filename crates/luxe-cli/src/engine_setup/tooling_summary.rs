//! Tooling status collection for the `/lsp`, `/debug`, and `/mcp` views — built-in defaults
//! merged with user config, a live PATH check, warm-server state, and active debug sessions.

use super::services::{command_on_path, load_debug_adapters, load_lsp_servers, SharedLspService};

/// Assemble the merged LSP-server + debug-adapter status for the `/lsp` and `/debug` views: built-in
/// defaults plus the user's config files, a live PATH installed-check, the warm servers running
/// under `root`, and any active debug sessions. Rebuilt by the collector each snapshot so the views
/// stay fresh (config edits, mid-session installs, live sessions) without a restart.
pub(crate) async fn collect_tooling_summary(
    root: &std::path::Path,
    lsp_service: &SharedLspService,
    dap_service: &luxe_dap::DapService,
) -> luxe_core::ToolingSummary {
    use luxe_core::{DebugAdapterStatus, DebugSessionStatus, LanguageServerStatus, ToolingSummary};

    // Warm servers under the active project root → the set of "running" languages.
    let running_langs: std::collections::HashSet<String> = lsp_service
        .running_servers()
        .into_iter()
        .filter(|(_, server_root)| server_root.starts_with(root) || root.starts_with(server_root))
        .map(|(lang, _)| lang)
        .collect();

    // LSP catalog: built-ins merged with lsp-servers.json (a config id overrides a built-in).
    let user_servers = load_lsp_servers();
    let mut registry = luxe_lsp::ServerRegistry::new();
    registry.merge_patches(user_servers.clone());
    let mut servers: Vec<LanguageServerStatus> = registry
        .iter()
        .map(|(language, entry)| LanguageServerStatus {
            language: language.clone(),
            command: entry.command.clone(),
            extensions: entry.extensions.clone(),
            from_config: user_servers.contains_key(language),
            install_hint: entry.install_hint.clone(),
            installed: command_on_path(&entry.command),
            running: running_langs.contains(language),
        })
        .collect();
    servers.sort_by(|a, b| a.language.cmp(&b.language));

    // Debug adapters: built-ins, minus any shadowed by a user id, plus the user's own.
    let adapters: Vec<DebugAdapterStatus> =
        super::services::merged_debug_adapters(&load_debug_adapters())
            .into_iter()
            .map(|(adapter, from_config)| DebugAdapterStatus {
                installed: command_on_path(adapter.spec.command.first().map_or("", String::as_str)),
                id: adapter.spec.id.clone(),
                command: adapter.spec.command.join(" "),
                extensions: adapter.extensions,
                from_config,
            })
            .collect();

    let debug_sessions = dap_service
        .list()
        .await
        .into_iter()
        .map(|s| DebugSessionStatus {
            id: s.id,
            adapter: s.adapter,
            program: s.program,
            terminated: s.terminated,
            stopped: s.stopped,
            stop_reason: s.stop_reason,
        })
        .collect();

    ToolingSummary {
        servers,
        adapters,
        debug_sessions,
        mcp_servers: collect_mcp_servers(root),
        tools: super::services::tool_statuses(),
        lsp_config_path: Some(luxe_core::config::Config::lsp_servers_path()),
        adapters_config_path: Some(luxe_core::config::Config::debug_adapters_path()),
    }
}

/// MCP servers, their cached tools and whether each is signed in, for `/mcp` and the `@` picker.
///
/// Reads the servers this run may start, the persisted metadata cache (`.luxe/mcp-metadata.json`)
/// and — only when a server signs in with OAuth — the names in the token store. Nothing is
/// connected and nothing is written, since this runs on every snapshot.
pub(crate) fn collect_mcp_servers(root: &std::path::Path) -> Vec<luxe_core::McpServerStatus> {
    use luxe_core::mcp_client::McpAuth;

    // What this run would start, not every file on disk: a refused project server listed as
    // configured is the "why doesn't it work" hunt all over again
    let servers = super::tools::mcp_servers_for_run(root).0;
    let signed_in = match servers.iter().any(|s| matches!(s.auth, Some(McpAuth::OAuth(_)))) {
        true => luxe_mcp::token_store::TokenStore::default_path().signed_in_ids(),
        false => Default::default(),
    };
    mcp_statuses(servers, &read_metadata_cache(root), &signed_in)
}

/// The metadata cache as JSON, or `Null` when there is none yet.
fn read_metadata_cache(root: &std::path::Path) -> serde_json::Value {
    std::fs::read_to_string(root.join(".luxe").join("mcp-metadata.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// One status row per server, from what is already on disk.
///
/// Args: `(servers, metadata cache, ids with a stored token)`.
fn mcp_statuses(
    servers: Vec<luxe_core::mcp_client::McpServerConfig>,
    cache: &serde_json::Value,
    signed_in: &std::collections::HashSet<String>,
) -> Vec<luxe_core::McpServerStatus> {
    use luxe_core::mcp_client::{McpAuth, McpTransport};

    servers
        .into_iter()
        .map(|server| {
            let cached = cache
                .get(&server.id)
                .and_then(|entry| entry.get("tools"))
                .and_then(|tools| tools.as_array());
            // `tool_count` keeps counting what the server lists; `tools` is what `mcp` will call,
            // the same exclusions the registry applies
            let tools = cached
                .into_iter()
                .flatten()
                .filter_map(|tool| tool.get("name")?.as_str())
                .filter(|name| !server.exclude_tools.iter().any(|excluded| excluded == name))
                .map(str::to_owned)
                .collect();
            luxe_core::McpServerStatus {
                transport: match server.transport {
                    McpTransport::Stdio { .. } => "stdio".into(),
                    McpTransport::HttpSse { .. } => "http".into(),
                },
                auth: match server.auth {
                    None => "none",
                    Some(McpAuth::Bearer { .. }) => "bearer",
                    Some(McpAuth::OAuth(_)) => "oauth",
                }
                .into(),
                signed_in: matches!(server.auth, Some(McpAuth::OAuth(_)))
                    .then(|| signed_in.contains(&server.id)),
                tool_count: cached.map(Vec::len),
                tools,
                enabled: server.enabled,
                id: server.id,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use luxe_core::mcp_client::{McpAuth, McpServerConfig, McpTransport, OAuthConfig};

    fn server(id: &str, auth: Option<McpAuth>, exclude: &[&str]) -> McpServerConfig {
        McpServerConfig {
            id: id.into(),
            transport: McpTransport::HttpSse { url: "https://example.com/mcp".into() },
            auth,
            exclude_tools: exclude.iter().map(|name| (*name).to_string()).collect(),
            enabled: true,
            ..Default::default()
        }
    }

    /// Through the real paths: the global config names the servers, the project's cache their
    /// tools, and the token store — in the state directory, both redirected here — who is signed in.
    #[test]
    fn the_status_rows_come_from_the_config_the_cache_and_the_token_store() {
        let (config, state, project) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        std::env::set_var("XDG_CONFIG_HOME", config.path());
        std::env::set_var("XDG_STATE_HOME", state.path());
        std::fs::create_dir_all(config.path().join("luxe")).unwrap();
        std::fs::write(
            config.path().join("luxe").join("mcp.json"),
            r#"{ "mcpServers": {
                "asana": { "url": "https://mcp.asana.com/v2/mcp", "auth": { "kind": "oauth" } },
                "linear": { "url": "https://mcp.linear.app/mcp", "auth": { "kind": "oauth" } }
            } }"#,
        )
        .unwrap();
        std::fs::create_dir_all(state.path().join("luxe")).unwrap();
        std::fs::write(
            state.path().join("luxe").join("mcp-auth.json"),
            r#"{ "asana": { "access_token": "t" } }"#,
        )
        .unwrap();
        std::fs::create_dir_all(project.path().join(".luxe")).unwrap();
        std::fs::write(
            project.path().join(".luxe").join("mcp-metadata.json"),
            r#"{ "asana": { "config_hash": 0, "tools": [{"name": "get_tasks"}] } }"#,
        )
        .unwrap();

        let mut rows = super::collect_mcp_servers(project.path());
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        let shape: Vec<_> =
            rows.iter().map(|row| (row.id.as_str(), row.tools.clone(), row.signed_in)).collect();
        assert_eq!(
            shape,
            vec![
                ("asana", vec!["get_tasks".to_string()], Some(true)),
                ("linear", Vec::new(), Some(false)),
            ]
        );
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::remove_var("XDG_STATE_HOME");
    }

    /// A row carries what `mcp` will call — the cache less the exclusions — and says signed in only
    /// for an OAuth server, from the ids with a token.
    #[test]
    fn a_status_row_carries_the_callable_tools_and_the_sign_in() {
        let cache = serde_json::json!({
            "asana": { "config_hash": 1, "tools": [{"name": "get_tasks"}, {"name": "delete_task"}, {"description": "nameless"}] },
            "docs": { "config_hash": 2, "tools": [] },
        });
        let signed_in = ["asana".to_string()].into_iter().collect();
        let oauth = || Some(McpAuth::OAuth(OAuthConfig::default()));
        let rows = super::mcp_statuses(
            vec![
                server("asana", oauth(), &["delete_task"]),
                server("linear", oauth(), &[]),
                server("docs", None, &[]),
            ],
            &cache,
            &signed_in,
        );
        let shape: Vec<_> = rows
            .iter()
            .map(|row| (row.id.as_str(), row.tool_count, row.tools.clone(), row.signed_in))
            .collect();
        assert_eq!(
            shape,
            vec![
                ("asana", Some(3), vec!["get_tasks".to_string()], Some(true)),
                ("linear", None, Vec::new(), Some(false)),
                ("docs", Some(0), Vec::new(), None),
            ]
        );
    }
}
