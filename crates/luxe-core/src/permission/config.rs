//! Permission config file — the on-disk `permissions.json` shape.
//!
//! One schema for both the global (`~/.config/luxe/permissions.json`) and project-local
//! (`.luxe/permissions.json`) files. Path resolution and the trust decision live in the CLI; this
//! module just parses the file and turns it into a [`Ruleset`] the engine can assemble.

use std::path::Path;

use serde::Deserialize;

use super::defaults::PermissionMode;
use super::{PermissionAction, PermissionConfig, Rule, Ruleset};
use crate::json_file::{FileError, FileMode, JsonFile};

/// Parsed `permissions.json`.
///
/// ```json
/// {
///   "mode": "default",
///   "permission": { "bash": { "*": "ask", "git *": "allow" }, "edit": "allow" }
/// }
/// ```
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PermissionsFile {
    /// Optional preset (`default`/`strict`/`read-only`/`yolo`). Ignored from an untrusted project.
    #[serde(default)]
    pub mode: Option<String>,
    /// Per-tool rules (OpenCode-style scalar-or-map).
    #[serde(default)]
    pub permission: Option<PermissionConfig>,
}

impl PermissionsFile {
    /// Parse from JSON text.
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Flatten the file's rules into a ruleset (empty when the file has none).
    pub fn ruleset(&self) -> Ruleset {
        self.permission.as_ref().map(PermissionConfig::to_ruleset).unwrap_or_default()
    }

    /// The parsed mode, if the file set a valid one.
    pub fn mode(&self) -> Option<PermissionMode> {
        self.mode.as_deref().and_then(PermissionMode::parse)
    }
}

/// Merge `rules` into the permissions file at `path`, creating it if needed.
///
/// This is how a "remember this" approval outlives the session. Only the named `permission →
/// pattern` entries are touched — every other key in the file (including ones this build doesn't
/// know about) is preserved, and the write is atomic and locked ([`crate::json_file`]), so a crash
/// can't leave a half-written policy behind and a file that does not parse is refused, not replaced.
///
/// Callers must point this at the *global* file: a project-local one is tighten-only, so an
/// approval written there would be ignored.
pub fn persist_rules(path: &Path, rules: &[Rule]) -> Result<(), FileError> {
    use serde_json::{Map, Value};

    if rules.is_empty() {
        return Ok(());
    }
    let file = JsonFile::new(path.to_path_buf(), FileMode::OwnerOnly);
    file.modify(|root| {
        let permission = root
            .entry("permission")
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| {
                FileError::Rejected(format!("{}: \"permission\" is not an object", path.display()))
            })?;
        for rule in rules {
            // A scalar value (`"edit": "allow"`) can't hold per-pattern entries, so promote it to
            // a map with the original action as the catch-all before adding this one.
            let entry =
                permission.entry(&rule.permission).or_insert_with(|| Value::Object(Map::new()));
            if let Some(scalar) = entry.as_str().map(str::to_string) {
                let mut promoted = Map::new();
                promoted.insert("*".to_string(), Value::String(scalar));
                *entry = Value::Object(promoted);
            }
            let Some(patterns) = entry.as_object_mut() else { continue };
            let action = Value::String(action_name(rule.action).to_string());
            patterns.insert(rule.pattern.clone(), action);
        }
        Ok(())
    })
}

fn action_name(action: PermissionAction) -> &'static str {
    match action {
        PermissionAction::Allow => "allow",
        PermissionAction::Ask => "ask",
        PermissionAction::Deny => "deny",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::{evaluate, PermissionAction};

    #[test]
    fn parses_mode_and_rules() {
        let file = PermissionsFile::from_json(
            r#"{ "mode": "strict", "permission": { "bash": { "*": "ask", "git *": "allow" } } }"#,
        )
        .unwrap();
        assert_eq!(file.mode(), Some(PermissionMode::Strict));
        let rules = file.ruleset();
        assert_eq!(evaluate("bash", "git status", &rules), Some(PermissionAction::Allow));
        assert_eq!(evaluate("bash", "curl x", &rules), Some(PermissionAction::Ask));
    }

    #[test]
    fn empty_or_missing_fields_are_fine() {
        let file = PermissionsFile::from_json("{}").unwrap();
        assert_eq!(file.mode(), None);
        assert!(file.ruleset().is_empty());
    }

    #[test]
    fn unknown_mode_is_none() {
        let file = PermissionsFile::from_json(r#"{ "mode": "bananas" }"#).unwrap();
        assert_eq!(file.mode(), None);
    }

    /// A permissions file that cannot be read as text is an error, not an empty file to write the
    /// approval over.
    #[test]
    fn persisting_refuses_a_file_it_cannot_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("permissions.json");
        let unreadable = b"{\"permission\":{\"bash\":\"\xff\"}}";
        std::fs::write(&path, unreadable).expect("seed");
        let rule = Rule {
            permission: "bash".into(),
            pattern: "cargo test".into(),
            action: PermissionAction::Allow,
        };
        let error = persist_rules(&path, std::slice::from_ref(&rule)).expect_err("refused");
        assert!(matches!(error, FileError::Io { .. }), "{error}");
        assert_eq!(std::fs::read(&path).expect("read"), unreadable);
    }

    /// A permissions file that does not parse is refused and left as it is, not replaced by one
    /// holding only the new approval.
    #[test]
    fn persisting_refuses_a_file_that_does_not_parse() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("permissions.json");
        std::fs::write(&path, "{ \"permission\": ").expect("seed");
        let rule = Rule {
            permission: "bash".into(),
            pattern: "cargo test".into(),
            action: PermissionAction::Allow,
        };
        let error = persist_rules(&path, std::slice::from_ref(&rule)).expect_err("refused");
        assert!(matches!(error, FileError::Unparseable { .. }), "{error}");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "{ \"permission\": ");
    }

    #[test]
    fn persisting_a_rule_creates_the_file_and_survives_a_reload() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("nested").join("permissions.json");
        let rule = crate::permission::Rule {
            permission: "external_directory".into(),
            pattern: "/home/dev/notes/*".into(),
            action: PermissionAction::Allow,
        };

        persist_rules(&path, std::slice::from_ref(&rule)).expect("write");
        let reloaded =
            PermissionsFile::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            evaluate("external_directory", "/home/dev/notes/one.md", &reloaded.ruleset()),
            Some(PermissionAction::Allow)
        );
    }

    #[test]
    fn persisting_preserves_unrelated_keys_and_promotes_a_scalar_entry() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("permissions.json");
        std::fs::write(
            &path,
            r#"{ "mode": "strict", "permission": { "edit": "ask", "bash": { "git *": "allow" } } }"#,
        )
        .unwrap();

        persist_rules(
            &path,
            &[crate::permission::Rule {
                permission: "edit".into(),
                pattern: "/tmp/scratch/*".into(),
                action: PermissionAction::Allow,
            }],
        )
        .expect("write");

        let text = std::fs::read_to_string(&path).unwrap();
        let file = PermissionsFile::from_json(&text).unwrap();
        assert_eq!(file.mode(), Some(PermissionMode::Strict), "unrelated keys survive");
        let rules = file.ruleset();
        assert_eq!(evaluate("bash", "git status", &rules), Some(PermissionAction::Allow));
        assert_eq!(
            evaluate("edit", "/tmp/scratch/x.rs", &rules),
            Some(PermissionAction::Allow),
            "the new rule applies"
        );
        assert_eq!(
            evaluate("edit", "src/other.rs", &rules),
            Some(PermissionAction::Ask),
            "the promoted scalar keeps its catch-all meaning"
        );
    }
}
