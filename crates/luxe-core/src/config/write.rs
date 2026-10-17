//! Config writes — the small, surgical edits a settings surface makes to `config.json`.
//!
//! The file belongs to the reader. [`super::Config`] has no catch-all field, so the tempting
//! write — deserialize, change, serialize — would delete every key this build does not know: a
//! setting from a newer version, a key somebody added by hand. Every write here goes through the
//! document as JSON instead, touches exactly one path, and keeps the order of everything else
//! (`serde_json` is built with `preserve_order`). Writes are atomic and locked ([`crate::json_file`]),
//! and the
//! schema beside the file is refreshed so an editor offers completion for whatever was just set.
//!
//! Only the global file is ever written. A project's `.luxe/config.json` is read to say where a
//! value comes from, and a project-scoped write would need the same trust rules as its hooks.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::json_file::{FileError, FileMode, JsonFile};

use super::Config;

/// What a write to `config.json` could not do.
#[derive(Debug, thiserror::Error)]
pub enum ConfigWriteError {
    /// The file exists and is not a JSON object, so writing would replace what the reader has
    #[error("{path} does not parse, so it was left alone: {reason}")]
    Unparseable {
        /// Where the file is
        path: PathBuf,
        /// What the parser said
        reason: String,
    },
    /// Reading or writing the file failed; the OS error is in the message, not also the `source()`
    #[error("writing {path}: {error}")]
    Io {
        /// Where the file is
        path: PathBuf,
        /// What failed
        error: std::io::Error,
    },
}

/// Which file a value was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// Nobody set it
    Default,
    /// `~/.config/luxe/config.json`
    Global,
    /// The project's `.luxe/config.json`, which wins over the global file
    Project,
}

/// The raw documents behind a merged [`Config`], for saying where each value came from.
#[derive(Debug, Clone, Default)]
pub struct ConfigLayers {
    /// The global file, when it exists and parses
    pub global: Option<Value>,
    /// The project file, when it exists and parses
    pub project: Option<Value>,
}

impl ConfigLayers {
    /// Read both files under `repo_root` as they are on disk.
    pub fn read(repo_root: &Path) -> Self {
        Self {
            global: read_document(&Config::global_path()),
            project: read_document(&repo_root.join(".luxe").join("config.json")),
        }
    }

    /// The value at `path` and the layer it came from — the project's when it sets one, or the
    /// global file's, or `None` for the built-in default.
    ///
    /// The precedence of a plain setting. A few keys merge instead (`tools.disabled` is a union,
    /// `agent.on_ask` is tighten-only), and a surface showing one of those reads both documents.
    pub fn lookup(&self, path: &[&str]) -> (Layer, Option<&Value>) {
        if let Some(value) = self.project.as_ref().and_then(|doc| value_at(doc, path)) {
            return (Layer::Project, Some(value));
        }
        match self.global.as_ref().and_then(|doc| value_at(doc, path)) {
            Some(value) => (Layer::Global, Some(value)),
            None => (Layer::Default, None),
        }
    }
}

/// The value at `path` inside `document`.
pub fn value_at<'a>(document: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(document, |node, key| node.get(key)).filter(|value| !value.is_null())
}

fn read_document(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<Value>(&text).ok().filter(Value::is_object)
}

impl Config {
    /// Sets `path` to `value` in the global config, leaving every other key exactly as it was.
    ///
    /// Creates the file (pointing at its schema) when there is none. Refuses, rather than
    /// overwrites, a file that does not parse: the reader may be halfway through editing it.
    pub fn set_global(path: &[&str], value: Value) -> Result<PathBuf, ConfigWriteError> {
        edit_global(|document| set(document, path, value))
    }

    /// Removes `path` from the global config, so the built-in default applies again, and drops any
    /// object the removal leaves empty.
    pub fn unset_global(path: &[&str]) -> Result<PathBuf, ConfigWriteError> {
        edit_global(|document| unset(document, path))
    }

    /// Turns one tool on or off in the global `tools.disabled`, keeping the rest of the list in its
    /// order and removing the key once nothing is left in it.
    ///
    /// A project's own list is a union on top of this one, so turning a tool on here does not
    /// override a project that turned it off; a surface says so beside the toggle.
    pub fn set_tool_enabled_globally(
        name: &str,
        enabled: bool,
    ) -> Result<PathBuf, ConfigWriteError> {
        edit_global(|document| {
            let mut disabled: Vec<Value> =
                match value_at(&Value::Object(document.clone()), &["tools", "disabled"]) {
                    Some(Value::Array(list)) => list.clone(),
                    _ => Vec::new(),
                };
            disabled.retain(|entry| entry.as_str() != Some(name));
            if !enabled {
                disabled.push(Value::String(name.to_string()));
            }
            match disabled.is_empty() {
                true => unset(document, &["tools", "disabled"]),
                false => set(document, &["tools", "disabled"], Value::Array(disabled)),
            }
        })
    }
}

/// Edit the global config under [`JsonFile`]'s lock. Called from the UI thread; the wait for the
/// lock is bounded (a few seconds), and only another Luxe writing the same file at that moment, or
/// one that crashed holding the lock, makes it wait at all.
fn edit_global(edit: impl FnOnce(&mut Map<String, Value>)) -> Result<PathBuf, ConfigWriteError> {
    let path = Config::global_path();
    let file = JsonFile::new(path.clone(), FileMode::PreserveExisting);
    file.modify(|document| {
        if document.is_empty() {
            *document = fresh();
        }
        edit(document);
        Ok(())
    })
    .map_err(|error| match error {
        FileError::Unparseable { reason, .. } => {
            ConfigWriteError::Unparseable { path: path.clone(), reason }
        }
        FileError::Io { error, .. } => ConfigWriteError::Io { path: path.clone(), error },
        other => ConfigWriteError::Io { path: path.clone(), error: std::io::Error::other(other) },
    })?;
    let schema = path.with_file_name("config.schema.json");
    crate::fs_atomic::replace(&schema, Config::schema_json().as_bytes())
        .map_err(|error| ConfigWriteError::Io { path: schema, error })?;
    Ok(path)
}

/// A new file: nothing in it but the pointer to its schema.
fn fresh() -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("$schema".into(), Value::String("./config.schema.json".into()));
    map
}

fn set(document: &mut Map<String, Value>, path: &[&str], value: Value) {
    let Some((last, parents)) = path.split_last() else { return };
    let mut node = document;
    for key in parents {
        let entry = node.entry((*key).to_string()).or_insert_with(|| Value::Object(Map::new()));
        // A scalar where an object belongs is replaced: the path names an object
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        node = entry.as_object_mut().expect("just made an object");
    }
    node.insert((*last).to_string(), value);
}

fn unset(document: &mut Map<String, Value>, path: &[&str]) {
    let Some((first, rest)) = path.split_first() else { return };
    if rest.is_empty() {
        document.remove(*first);
        return;
    }
    if let Some(Value::Object(child)) = document.get_mut(*first) {
        unset(child, rest);
        if child.is_empty() {
            document.remove(*first);
        }
    }
}

#[cfg(test)]
mod tests;
