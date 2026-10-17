//! LSP server registry — language id → server command and config.
//!
//! Detects a file's language from its extension and finds its workspace root by marker files
//! (`Cargo.toml`, `package.json`, …).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One way to launch a server for a language.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerCommand {
    /// Program name, resolved against `PATH` and the well-known bin directories.
    pub command: String,
    /// Arguments to pass it.
    #[serde(default)]
    pub args: Vec<String>,
}

/// Configuration for a language server entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerEntry {
    /// Command to launch the language server (e.g., "rust-analyzer").
    pub command: String,

    /// Arguments to pass to the language server.
    #[serde(default)]
    pub args: Vec<String>,

    /// Further ways to launch this language, tried in order after [`Self::command`].
    ///
    /// A language is not a server, and the two drift apart. TypeScript 7 is the native port: it
    /// ships no `tsserver.js`, so `typescript-language-server` — at every version — fails its
    /// handshake with "Could not find a valid TypeScript installation", and the language service
    /// moved to `tsc --lsp`. One hardcoded command turns that into "Luxe has no TypeScript
    /// support"; a list turns it into a fallback nobody has to notice.
    ///
    /// Ordered most-preferred first, and tried on a *failed handshake*, not merely a missing
    /// binary — `tsc` is on `PATH` for both TypeScript 6 and 7 and only one of them answers.
    #[serde(default)]
    pub alternatives: Vec<ServerCommand>,

    /// Root marker files/directories to detect workspace root (e.g., "Cargo.toml").
    #[serde(default)]
    pub root_markers: Vec<String>,

    /// File extensions (no leading dot) this server handles, e.g. `["rs"]`. Drives language
    /// detection, so a config-added server auto-routes by extension.
    #[serde(default)]
    pub extensions: Vec<String>,

    /// Additional initialization options.
    #[serde(default)]
    pub init_options: serde_json::Value,

    /// How to install the server, surfaced when it isn't found (e.g. `"gem install ruby-lsp"`).
    #[serde(default)]
    pub install_hint: Option<String>,
}

impl ServerEntry {
    /// Every way to launch this language, most preferred first.
    pub fn candidates(&self) -> Vec<ServerCommand> {
        std::iter::once(ServerCommand { command: self.command.clone(), args: self.args.clone() })
            .chain(self.alternatives.iter().cloned())
            .collect()
    }
}

/// A user's `lsp-servers.json` entry: every field optional.
///
/// The distinction that matters is *absent* versus *empty*. [`ServerEntry`] cannot express it —
/// its fields are `#[serde(default)]`, so `{"rust": {"command": "rust-analyzer-nightly"}}`
/// deserializes to an entry with no extensions and no root markers, and merging that over the
/// built-in would silently remove Rust support: `.rs` would stop detecting as a language. A patch
/// inherits every field the user did not mention, which also means a field added to `ServerEntry`
/// later starts life inherited rather than blanked out of every existing config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerEntryPatch {
    /// Program to launch; required when the language has no built-in to inherit from.
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub alternatives: Option<Vec<ServerCommand>>,
    pub root_markers: Option<Vec<String>>,
    pub extensions: Option<Vec<String>>,
    pub init_options: Option<serde_json::Value>,
    pub install_hint: Option<String>,
}

impl ServerEntryPatch {
    /// Apply this patch over the shipped entry for the same language, if there is one.
    ///
    /// Returns `None` for a patch that names a language Luxe does not ship *and* omits `command`,
    /// which is the one combination that cannot produce a launchable server.
    pub fn apply(self, base: Option<&ServerEntry>) -> Option<ServerEntry> {
        let base = base.cloned().unwrap_or_default();
        let command =
            self.command.or_else(|| (!base.command.is_empty()).then_some(base.command))?;
        Some(ServerEntry {
            command,
            args: self.args.unwrap_or(base.args),
            alternatives: self.alternatives.unwrap_or(base.alternatives),
            root_markers: self.root_markers.unwrap_or(base.root_markers),
            extensions: self.extensions.unwrap_or(base.extensions),
            init_options: self.init_options.unwrap_or(base.init_options),
            install_hint: self.install_hint.or(base.install_hint),
        })
    }
}

/// The server registry: language_id → ServerEntry.
#[derive(Debug, Clone, Default)]
pub struct ServerRegistry {
    entries: HashMap<String, ServerEntry>,
}

impl ServerRegistry {
    /// Create a new registry with built-in entries (rust-analyzer, etc.).
    pub fn new() -> Self {
        let mut registry = Self::default();
        registry.add_defaults();
        registry
    }

    /// Add default server entries for the common languages from the bundled `default-lsp-servers
    /// .json`. Shipping them as DATA (not hardcoded) lets users inspect + copy the defaults, and
    /// built-in + user config share one schema and code path. Servers are external binaries the
    /// user installs; Luxe only knows how to launch + detect them.
    fn add_defaults(&mut self) {
        // Compile-time-embedded and covered by `default_servers_bundle_parses` — a genuine
        // invariant, so a parse failure is a build-time bug, never a runtime path.
        let defaults: HashMap<String, ServerEntry> = serde_json::from_str(DEFAULT_SERVERS_JSON)
            .expect("bundled default-lsp-servers.json must be valid");
        self.entries.extend(defaults);
    }

    /// Register a custom server entry.
    pub fn register(&mut self, language_id: String, entry: ServerEntry) {
        self.entries.insert(language_id, entry);
    }

    /// Merge user-configured servers (from `lsp-servers.json`), overriding built-ins by language id.
    ///
    /// Whole-entry replacement: use it for an entry you built programmatically and mean in full.
    /// User config goes through [`Self::merge_patches`], which inherits what it does not mention.
    pub fn merge_custom(&mut self, custom: HashMap<String, ServerEntry>) {
        self.entries.extend(custom);
    }

    /// Merge user config, inheriting every field a patch leaves out from the shipped entry.
    ///
    /// Returns the language ids of patches that could not be applied — a new language with no
    /// `command` — so a caller can say so rather than drop them silently.
    pub fn merge_patches(&mut self, custom: HashMap<String, ServerEntryPatch>) -> Vec<String> {
        let mut rejected = Vec::new();
        for (language_id, patch) in custom {
            match patch.apply(self.entries.get(&language_id)) {
                Some(entry) => {
                    self.entries.insert(language_id, entry);
                }
                None => rejected.push(language_id),
            }
        }
        rejected.sort();
        rejected
    }

    /// The install hint for a language's server, when known.
    pub fn install_hint(&self, language_id: &str) -> Option<&str> {
        self.entries.get(language_id).and_then(|e| e.install_hint.as_deref())
    }

    /// Detect the language ID for a file from its extension, driven by the registered entries so
    /// config-added servers route automatically.
    pub fn detect_language(&self, path: &Path) -> Option<String> {
        let ext = path.extension()?.to_str()?;
        self.entries
            .iter()
            .find(|(_, entry)| entry.extensions.iter().any(|candidate| candidate == ext))
            .map(|(lang, _)| lang.clone())
    }

    /// Find the workspace root for a file by searching up the directory tree.
    pub fn find_root(&self, path: &Path, markers: &[String]) -> Option<PathBuf> {
        let mut current = path.parent()?;

        loop {
            for marker in markers {
                if current.join(marker).exists() {
                    return Some(current.to_path_buf());
                }
            }

            match current.parent() {
                Some(parent) => current = parent,
                None => break,
            }
        }

        None
    }

    /// Get the server entry for a language ID.
    pub fn get(&self, language_id: &str) -> Option<&ServerEntry> {
        self.entries.get(language_id)
    }

    /// Check if a language is supported.
    pub fn is_supported(&self, language_id: &str) -> bool {
        self.entries.contains_key(language_id)
    }

    /// Iterate over `(language_id, entry)` for every registered server (built-in + merged config),
    /// for status/introspection surfaces like the `/lsp` command.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &ServerEntry)> {
        self.entries.iter()
    }
}

/// The shipped default LSP-server registry (common languages), merged first in every registry.
const DEFAULT_SERVERS_JSON: &str = include_str!("default-lsp-servers.json");

#[cfg(test)]
mod tests;
