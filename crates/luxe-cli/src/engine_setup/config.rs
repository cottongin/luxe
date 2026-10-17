//! Configuration loading — auth/models, permissions, command hooks, and env resolution.
//!
//! Every reader treats its file as read-if-present: a missing or malformed config degrades to
//! defaults rather than failing, so Luxe runs the same with or without user config.

use std::path::PathBuf;

use luxe_providers::config::ModelsConfig;

/// The API-key environment variables a provider answers to, most specific first.
///
/// The provider catalog is the source of truth; an unrecognized provider falls back to
/// `OPENAI_API_KEY`, which is what an unknown OpenAI-compatible endpoint is most likely
/// authenticated by.
pub(crate) fn provider_key_env(provider: &str) -> &'static [&'static str] {
    match luxe_providers::auth::provider_descriptor(provider) {
        Some(descriptor) => descriptor.env,
        None => &["OPENAI_API_KEY"],
    }
}

/// A non-empty environment variable value, or `None`.
pub(crate) fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.trim().is_empty())
}

/// Path to the on-disk models.dev catalog cache (`~/.cache/luxe/models-dev.json`).
///
/// Derived data, so it lives in the cache dir rather than next to the user's config; deleting it
/// only costs a refresh. `LUXE_MODELS_DEV_PATH` points at a specific file instead (a mirror, an
/// air-gapped copy, or a pinned catalog for reproducible runs).
pub(crate) fn models_dev_cache_path() -> PathBuf {
    if let Some(path) = env_nonempty("LUXE_MODELS_DEV_PATH") {
        return PathBuf::from(path);
    }
    luxe_core::user_dirs::luxe_cache_dir().join("models-dev.json")
}

/// Load and merge `~/.config/luxe/auth.json` (nested schema) with `models.json`.
///
/// `models.json` overrides win: its `models`/`providers` entries replace matching keys and
/// its `default_*` fields override when set.
pub(crate) fn load_luxe_config() -> ModelsConfig {
    let auth_path = luxe_core::user_dirs::luxe_config_file("auth.json");
    let models_path = luxe_core::user_dirs::luxe_config_file("models.json");
    // Nudge (once) if a secrets file is group/world-readable — parity with ssh/aws-cli.
    static WARN_ONCE: std::sync::Once = std::sync::Once::new();
    WARN_ONCE.call_once(|| {
        warn_if_loose_perms(&auth_path);
        warn_if_loose_perms(&models_path);
    });
    let mut config = read_json_config(&auth_path).unwrap_or_default();
    if let Some(models) = read_json_config(&models_path) {
        config.models.extend(models.models);
        config.providers.extend(models.providers);
        if models.default_provider.is_some() {
            config.default_provider = models.default_provider;
        }
        if models.default_model.is_some() {
            config.default_model = models.default_model;
        }
    }
    config
}

/// Warn if a secrets file is readable by group or other.
///
/// Luxe writes its own secret files `0600` (MCP token store, review stores), but `auth.json`/
/// `models.json` are user-authored, so we can only nudge: ssh and aws-cli take the same stance
/// (warn/refuse on loose credential permissions). Unix-only; a no-op elsewhere.
#[cfg(unix)]
pub(crate) fn warn_if_loose_perms(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::metadata(path) else { return };
    let mode = meta.permissions().mode() & 0o777;
    if perms_are_loose(mode) {
        tracing::warn!(
            path = %path.display(),
            mode = format!("{mode:#o}"),
            "secrets file is readable by group/other; run `chmod 600` on it"
        );
    }
}

#[cfg(not(unix))]
pub(crate) fn warn_if_loose_perms(_path: &std::path::Path) {}

/// A file mode is "loose" for a secret if any group/other bit is set.
#[cfg(unix)]
pub(crate) fn perms_are_loose(mode: u32) -> bool {
    mode & 0o077 != 0
}

/// Load command hooks (external commands run at agent-loop hook points).
///
/// Trust: the *global* `~/.config/luxe/hooks.json` is always honored; a project's
/// `.luxe/hooks.json` is loaded ONLY under `--trust-project`. A hook runs an arbitrary command,
/// so honoring project hooks by default would let a cloned repo execute code on the next tool
/// call — the same footgun as malicious git hooks. Returns `None` when no hooks are configured.
pub(crate) fn load_command_hooks(
    repo_root: &std::path::Path,
    trust_project: bool,
) -> Option<luxe_core::command_hooks::CommandHook> {
    use luxe_core::command_hooks::{CommandHook, HooksFile};

    let parse = |path: std::path::PathBuf| -> HooksFile {
        let file = match std::fs::read_to_string(&path) {
            Ok(text) => HooksFile::from_json(&text).unwrap_or_else(|e| {
                tracing::warn!(path = %path.display(), error = %e, "ignoring malformed hooks.json");
                HooksFile::default()
            }),
            Err(_) => HooksFile::default(),
        };
        // A key naming no hook point is the one failure a user cannot see: the hook is simply
        // never called, on the machine where it matters. Say so, and say what they probably meant.
        for key in &file.unknown_points {
            match luxe_core::hook::HookPoint::closest(key) {
                Some(nearest) => tracing::warn!(
                    path = %path.display(),
                    point = %key,
                    did_you_mean = %nearest,
                    "ignoring hook at an unknown hook point"
                ),
                None => tracing::warn!(
                    path = %path.display(),
                    point = %key,
                    "ignoring hook at an unknown hook point"
                ),
            }
        }
        file
    };

    let mut merged = parse(luxe_config_dir().join("hooks.json")).hooks;
    if trust_project {
        for (point, defs) in parse(repo_root.join(".luxe").join("hooks.json")).hooks {
            merged.entry(point).or_default().extend(defs);
        }
    }
    let hook =
        CommandHook::new(HooksFile { hooks: merged, unknown_points: Vec::new() }.into_specs());
    (!hook.is_empty()).then_some(hook)
}

/// Assemble the permission engine from config + CLI intent.
///
/// Layers, trusted-first: built-in defaults + the mode overlay + the global
/// `~/.config/luxe/permissions.json`; then the project-local `.luxe/permissions.json` as a
/// **tighten-only** overlay (it can raise strictness but not weaken the baseline) unless
/// `--trust-project`, which merges it freely. Mode precedence: CLI override > global file >
/// (project file only when trusted) > `Default`. A missing/malformed file is skipped.
pub(crate) fn load_permission_engine(
    repo_root: &std::path::Path,
    mode_override: Option<luxe_core::permission::PermissionMode>,
    trust_project: bool,
) -> luxe_core::permission::PermissionEngine {
    use luxe_core::permission::PermissionEngine;

    let (global, project) = permission_files(repo_root);
    let mode = resolve_mode(mode_override, &global, &project, trust_project);
    PermissionEngine::assemble(mode, global.ruleset(), project.ruleset(), trust_project)
}

/// The permission mode this run is in, by the same precedence [`load_permission_engine`] uses.
///
/// For a decision the permission engine does not make itself: `--rpc` refusing GitHub writes in a
/// read-only session, which has to see a `"mode": "read-only"` in the global file exactly as the
/// engine does.
pub(crate) fn effective_permission_mode(
    repo_root: &std::path::Path,
    mode_override: Option<luxe_core::permission::PermissionMode>,
    trust_project: bool,
) -> luxe_core::permission::PermissionMode {
    let (global, project) = permission_files(repo_root);
    resolve_mode(mode_override, &global, &project, trust_project)
}

/// The global and project permissions files, each empty when missing or malformed.
fn permission_files(
    repo_root: &std::path::Path,
) -> (luxe_core::permission::PermissionsFile, luxe_core::permission::PermissionsFile) {
    use luxe_core::permission::PermissionsFile;
    let read_file = |path: std::path::PathBuf| -> PermissionsFile {
        match std::fs::read_to_string(&path) {
            Ok(text) => PermissionsFile::from_json(&text).unwrap_or_else(|e| {
                tracing::warn!(path = %path.display(), error = %e, "ignoring malformed permissions.json");
                PermissionsFile::default()
            }),
            Err(_) => PermissionsFile::default(),
        }
    };
    (
        read_file(global_permissions_path()),
        read_file(repo_root.join(".luxe").join("permissions.json")),
    )
}

/// CLI override, then the global file, then the project file when trusted, then `Default`.
fn resolve_mode(
    mode_override: Option<luxe_core::permission::PermissionMode>,
    global: &luxe_core::permission::PermissionsFile,
    project: &luxe_core::permission::PermissionsFile,
    trust_project: bool,
) -> luxe_core::permission::PermissionMode {
    mode_override
        .or_else(|| global.mode())
        .or_else(|| trust_project.then(|| project.mode()).flatten())
        .unwrap_or_default()
}

/// Parse a JSON config file into [`ModelsConfig`], returning `None` when absent/invalid.
pub(crate) fn read_json_config(path: &std::path::Path) -> Option<ModelsConfig> {
    if !path.exists() {
        return None;
    }
    match luxe_providers::config::load_models_config(path) {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "ignoring malformed config");
            None
        }
    }
}

/// The global permissions file (`~/.config/luxe/permissions.json`).
///
/// The only file a "remember this" approval may be written to: a project-local ruleset is
/// tighten-only, so an approval stored there would never take effect.
pub(crate) fn global_permissions_path() -> PathBuf {
    luxe_config_dir().join("permissions.json")
}

/// Luxe's own config directory. See [`luxe_core::user_dirs::luxe_config_dir`], the one owner.
pub(crate) fn luxe_config_dir() -> PathBuf {
    luxe_core::user_dirs::luxe_config_dir()
}

/// Whether an env var is explicitly set to a falsy value (`0`/`false`/`no`/`off`). Used for
/// default-on features that can be turned off.
pub(crate) fn env_disabled(key: &str) -> bool {
    std::env::var(key)
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off"))
        .unwrap_or(false)
}

/// A short, stable hash of the repo path for session directory naming.
pub(crate) fn project_hash(path: &std::path::Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every file the guide's table says lives in the config directory resolves under
    /// `LUXE_CONFIG_DIR`, through the code that actually reads it. A new row has to join the map,
    /// so a file added to the guide cannot be left behind by the override.
    ///
    /// The break this catches: three resolvers, and core's ignored the variable, so `auth.json`
    /// and `models.json` moved while `config.json`, `themes/`, `skills/` and the rest stayed put.
    #[test]
    fn the_override_moves_every_file_in_the_config_directory() {
        let _env =
            crate::engine_setup::test_env::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var_os("LUXE_CONFIG_DIR");
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("profile");
        std::fs::create_dir_all(&root).expect("profile dir");
        std::env::set_var("LUXE_CONFIG_DIR", &root);
        let cwd = PathBuf::from("/tmp/luxe-config-all-project");
        let contains = |paths: Vec<PathBuf>, name: &str| paths.contains(&root.join(name));
        let resolved = |name: &str| -> bool {
            match name {
                "config.json" => luxe_core::config::Config::global_path() == root.join(name),
                "keybindings.json" => luxe_core::keymap::keybindings_path() == root.join(name),
                "models.json" => {
                    luxe_providers::auth::config_file::ConfigFile::default_path().path()
                        == root.join(name)
                }
                "auth.json" => {
                    luxe_providers::auth::AuthStore::default_path().path() == root.join(name)
                }
                "permissions.json" => global_permissions_path() == root.join(name),
                "hooks.json" => {
                    let hooks = r#"{ "BeforeToolUse": [ { "command": ["true"] } ] }"#;
                    std::fs::write(root.join(name), hooks).expect("hooks.json");
                    let read = load_command_hooks(&cwd, false).is_some();
                    // and only that file: gone, there is nothing, whatever the machine's own says
                    std::fs::remove_file(root.join(name)).expect("hooks.json removed");
                    read && load_command_hooks(&cwd, false).is_none()
                }
                "mcp.json" => contains(luxe_core::mcp_config::McpConfigStore::sources(&cwd), name),
                "lsp-servers.json" => {
                    luxe_core::config::Config::lsp_servers_path() == root.join(name)
                }
                "debug-adapters.json" => {
                    luxe_core::config::Config::debug_adapters_path() == root.join(name)
                }
                "agents.md" => luxe_core::instructions::global_context_file() == root.join(name),
                "themes/" => luxe_core::theme::user_themes_dir() == root.join("themes"),
                "skills/" => contains(luxe_core::skills::skill_dirs(&cwd), "skills"),
                other => panic!("the guide lists `{other}`; add how it resolves to this map"),
            }
        };
        let guide = include_str!("../guide.md");
        let table = guide.split("| File | Holds | In a project |").nth(1).expect("the files table");
        let names: Vec<&str> = table
            .lines()
            .skip(2)
            .take_while(|line| line.starts_with('|'))
            .filter_map(|line| line.split('`').nth(1))
            .collect();
        assert!(names.len() >= 12, "the table parsed: {names:?}");
        let left_behind: Vec<&str> = names.iter().copied().filter(|name| !resolved(name)).collect();
        let shown = luxe_core::user_dirs::config_path_display("x.json");
        assert_eq!(shown, luxe_core::paths::home_relative(&root.join("x.json")));

        match previous {
            Some(value) => std::env::set_var("LUXE_CONFIG_DIR", value),
            None => std::env::remove_var("LUXE_CONFIG_DIR"),
        }
        assert!(left_behind.is_empty(), "these stayed behind: {left_behind:?}");
    }

    /// `LUXE_STATE_DIR` moves the state record and the MCP tokens together, not just the record.
    #[test]
    fn the_state_override_moves_every_state_file() {
        let _env =
            crate::engine_setup::test_env::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var_os("LUXE_STATE_DIR");
        let root = PathBuf::from("/tmp/luxe-state-all");
        std::env::set_var("LUXE_STATE_DIR", &root);
        let record = luxe_core::runtime_state::RuntimeState::path();
        let tokens = luxe_mcp::token_store::TokenStore::default_path().path().to_path_buf();
        match previous {
            Some(value) => std::env::set_var("LUXE_STATE_DIR", value),
            None => std::env::remove_var("LUXE_STATE_DIR"),
        }
        assert_eq!(record, root.join("state.json"));
        assert_eq!(tokens, root.join("mcp-auth.json"));
    }
}
