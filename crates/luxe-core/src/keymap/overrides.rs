//! Keybinding overrides — the user's and the project's `keybindings.json`, layered.

use super::UiAction;
use crate::user_dirs::luxe_config_dir;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The user-level `keybindings.json`.
pub fn keybindings_path() -> PathBuf {
    luxe_config_dir().join("keybindings.json")
}

/// Load keybindings overrides from `keybindings.json` (global + project layers).
///
/// Merges the global and project (`.luxe/keybindings.json`) layers, project winning. Returns an
/// empty map if no file is found or parsing fails.
pub fn load_keybindings(repo_root: &Path) -> HashMap<String, Vec<String>> {
    let global = load_keybindings_file(keybindings_path());
    let project = load_keybindings_file(repo_root.join(".luxe").join("keybindings.json"));
    merge_keybindings(global, project)
}

/// Load a keybindings file, returning an empty map on any error.
fn load_keybindings_file(path: PathBuf) -> HashMap<String, Vec<String>> {
    if !path.exists() {
        return HashMap::new();
    }
    match std::fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str(&contents) {
            Ok(map) => map,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "malformed keybindings; ignoring");
                HashMap::new()
            }
        },
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "failed to read keybindings; ignoring");
            HashMap::new()
        }
    }
}

/// Merge two keybindings maps: project keys override global (replace, not append).
///
/// By action rather than by spelling, so a project naming an action by a retired id still
/// overrides the user's binding under the current one.
fn merge_keybindings(
    global: HashMap<String, Vec<String>>,
    project: HashMap<String, Vec<String>>,
) -> HashMap<String, Vec<String>> {
    let mut merged = canonical(global);
    for (k, v) in canonical(project) {
        merged.insert(k, v);
    }
    merged
}

/// Renames every retired action id to its current one, so a layer names each action once.
///
/// Where one layer names an action both ways, the current id's entry wins and the retired one is
/// dropped whole. An id no action has passes through, for the keymap to warn about.
pub(super) fn canonical(bindings: HashMap<String, Vec<String>>) -> HashMap<String, Vec<String>> {
    let mut named: HashMap<String, (bool, Vec<String>)> = HashMap::new();
    for (id, chords) in bindings {
        let (key, current) = match UiAction::from_id(&id) {
            Some(action) => (action.id().to_string(), action.id() == id),
            None => (id, true),
        };
        match named.entry(key) {
            Entry::Occupied(mut entry) if current => {
                entry.insert((current, chords));
            }
            Entry::Occupied(_) => {}
            Entry::Vacant(entry) => {
                entry.insert((current, chords));
            }
        }
    }
    named.into_iter().map(|(id, (_, chords))| (id, chords)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::XDG_CONFIG_HOME_LOCK;

    /// Keybindings merge: project keys override global (replace, not append).
    #[test]
    fn keybindings_merge_project_overrides_global() {
        let mut global = HashMap::new();
        global.insert("quit".to_string(), vec!["Ctrl+Q".to_string()]);
        global.insert("save".to_string(), vec!["Ctrl+S".to_string()]);

        let mut project = HashMap::new();
        project.insert("quit".to_string(), vec!["Ctrl+W".to_string()]); // override

        let merged = merge_keybindings(global, project);

        assert_eq!(merged.get("quit").unwrap(), &vec!["Ctrl+W".to_string()]); // overridden
        assert_eq!(merged.get("save").unwrap(), &vec!["Ctrl+S".to_string()]); // preserved from global
    }

    /// Keybindings merge: project adds keys without removing global ones.
    #[test]
    fn keybindings_merge_project_adds_keys() {
        let mut global = HashMap::new();
        global.insert("quit".to_string(), vec!["Ctrl+Q".to_string()]);

        let mut project = HashMap::new();
        project.insert("copy".to_string(), vec!["Ctrl+C".to_string()]); // new key

        let merged = merge_keybindings(global, project);

        assert_eq!(merged.get("quit").unwrap(), &vec!["Ctrl+Q".to_string()]); // preserved
        assert_eq!(merged.get("copy").unwrap(), &vec!["Ctrl+C".to_string()]); // added from project
    }

    /// Keybindings merge: empty project returns global as-is.
    #[test]
    fn keybindings_merge_empty_project_returns_global() {
        let mut global = HashMap::new();
        global.insert("quit".to_string(), vec!["Ctrl+Q".to_string()]);

        let project = HashMap::new();
        let merged = merge_keybindings(global, project);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged.get("quit").unwrap(), &vec!["Ctrl+Q".to_string()]);
    }

    /// Keybindings load: malformed JSON file returns empty map (never panics).
    #[test]
    fn keybindings_load_malformed_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybindings.json");
        std::fs::write(&path, "{invalid json}").unwrap();

        let result = load_keybindings_file(path);
        assert!(result.is_empty());
    }

    /// Keybindings load: missing file returns empty map.
    #[test]
    fn keybindings_load_missing_file_returns_empty() {
        let path = PathBuf::from("/nonexistent/path/keybindings.json");
        let result = load_keybindings_file(path);
        assert!(result.is_empty());
    }

    /// Keybindings load: valid JSON file parses correctly.
    #[test]
    fn keybindings_load_valid_file_parses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybindings.json");
        std::fs::write(&path, r#"{"quit": ["Ctrl+Q"], "save": ["Ctrl+S"]}"#).unwrap();

        let result = load_keybindings_file(path);
        assert_eq!(result.len(), 2);
        assert_eq!(result.get("quit").unwrap(), &vec!["Ctrl+Q".to_string()]);
        assert_eq!(result.get("save").unwrap(), &vec!["Ctrl+S".to_string()]);
    }

    /// Keybindings load: layering global + project, project wins.
    #[test]
    fn keybindings_load_layering_project_wins() {
        let _xdg_config_home =
            XDG_CONFIG_HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let dir = tempfile::tempdir().unwrap();

        // Global keybindings
        let global_dir = dir.path().join("config").join("luxe");
        std::fs::create_dir_all(&global_dir).unwrap();
        std::fs::write(
            global_dir.join("keybindings.json"),
            r#"{"quit": ["Ctrl+Q"], "save": ["Ctrl+S"]}"#,
        )
        .unwrap();

        // Project keybindings (overrides quit)
        let project_dir = dir.path().join(".luxe");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(project_dir.join("keybindings.json"), r#"{"quit": ["Ctrl+W"]}"#).unwrap();

        // Point config discovery at the temp dir.
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        let result = load_keybindings(dir.path());

        assert_eq!(result.get("quit").unwrap(), &vec!["Ctrl+W".to_string()]); // overridden
        assert_eq!(result.get("save").unwrap(), &vec!["Ctrl+S".to_string()]); // preserved from global

        std::env::remove_var("XDG_CONFIG_HOME");
    }

    /// A project that names an action by its retired id still overrides the user's binding under
    /// the current one.
    ///
    /// The break this catches: the layers merged by the literal key, so both survived, and the
    /// project's lost to the global one.
    #[test]
    fn a_project_using_a_retired_id_still_wins() {
        let global = HashMap::from([("tui.preview.toggle".to_string(), vec!["alt+g".to_string()])]);
        let project =
            HashMap::from([("tui.changes.toggle".to_string(), vec!["alt+p".to_string()])]);
        let merged = merge_keybindings(global, project);
        assert_eq!(merged, HashMap::from([("tui.preview.toggle".into(), vec!["alt+p".into()])]));
    }
}
