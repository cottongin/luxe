//! Config files — where they live, how one is read, and how the user's is first written.

use super::Config;
use crate::user_dirs::luxe_config_dir;
use std::io::Write;
use std::path::{Path, PathBuf};

impl Config {
    /// The user-level `config.json`.
    pub fn global_path() -> PathBuf {
        luxe_config_dir().join("config.json")
    }

    /// Path to the optional custom debug-adapter registry (`debug-adapters.json`).
    pub fn debug_adapters_path() -> PathBuf {
        luxe_config_dir().join("debug-adapters.json")
    }

    /// Path to the optional custom LSP-server registry (`lsp-servers.json`).
    pub fn lsp_servers_path() -> PathBuf {
        luxe_config_dir().join("lsp-servers.json")
    }

    /// Creates the user config and its JSON schema when missing.
    ///
    /// Existing config is never overwritten. The generated schema is refreshed so opening
    /// the file after an upgrade immediately exposes new settings to editor completion.
    pub fn ensure_global_file() -> std::io::Result<PathBuf> {
        let path = Self::global_path();
        let directory = path.parent().unwrap_or_else(|| Path::new("."));
        ensure_config_file(directory)
    }
}

/// Load the global config from `~/.config/luxe/config.json`.
pub(super) fn load_global_config() -> Option<Config> {
    read_config_file(&Config::global_path())
}

/// Load the project config from `<repo>/.luxe/config.json`.
pub(super) fn load_project_config(repo_root: &Path) -> Option<Config> {
    let path = repo_root.join(".luxe").join("config.json");
    read_config_file(&path)
}

/// Read and parse a config file, returning `None` on any error (with a warning).
pub(super) fn read_config_file(path: &Path) -> Option<Config> {
    if !path.exists() {
        return None;
    }
    match std::fs::read_to_string(path) {
        Ok(contents) => match serde_json::from_str(&contents) {
            Ok(config) => Some(config),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "malformed UI config; using defaults");
                None
            }
        },
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "failed to read UI config; using defaults");
            None
        }
    }
}

pub(super) fn ensure_config_file(directory: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(directory)?;
    std::fs::write(directory.join("config.schema.json"), Config::schema_json())?;

    let path = directory.join("config.json");
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => file.write_all(b"{\n  \"$schema\": \"./config.schema.json\"\n}\n")?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    Ok(path)
}
