use super::*;

fn write_gate_config(root: &Path, folder: &str, commands: &[&str]) {
    let path = root.join(folder);
    std::fs::create_dir_all(&path).unwrap();
    let commands = if commands.is_empty() { vec![] } else { vec![commands] };
    std::fs::write(
        path.join("config.json"),
        serde_json::json!({"gate": {"commands": commands, "timeout_secs": 90}}).to_string(),
    )
    .unwrap();
}

/// A cloned project's commands must not replace the user's acceptance commands.
#[test]
fn a_project_cannot_choose_the_gate_commands_without_trust() {
    let _lock = XDG_CONFIG_HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path());
    std::fs::create_dir_all(dir.path().join("luxe")).unwrap();
    std::fs::create_dir_all(dir.path().join(".luxe")).unwrap();
    std::fs::write(
        dir.path().join("luxe/config.json"),
        r#"{"gate":{"commands":[["global-check"]]}}"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join(".luxe/config.json"),
        r#"{"gate":{"commands":[["project-check"]],"timeout_secs":17},"theme":"light"}"#,
    )
    .unwrap();
    let config = Config::load(dir.path());
    std::env::remove_var("XDG_CONFIG_HOME");
    assert_eq!(config.gate.commands(), vec![vec!["global-check".to_string()]]);
    assert_eq!(config.gate.timeout(), std::time::Duration::from_secs(17));
    assert_eq!(config.theme.as_deref(), Some("light"));
}

/// A fresh install must not take a project's commands just because no global file exists.
#[test]
fn project_gate_commands_are_ignored_without_a_global_config() {
    let _lock = XDG_CONFIG_HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path());
    std::fs::create_dir_all(dir.path().join(".luxe")).unwrap();
    std::fs::write(
        dir.path().join(".luxe/config.json"),
        r#"{"gate":{"commands":[["project-check"]]}}"#,
    )
    .unwrap();
    let config = Config::load(dir.path());
    std::env::remove_var("XDG_CONFIG_HOME");
    assert!(config.gate.commands().is_empty());
}

/// Explicit trust lets a project's commands replace, rather than append to, the global list.
#[test]
fn gate_commands_are_read_and_a_trusted_project_replaces_them() {
    let _lock = XDG_CONFIG_HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path());
    write_gate_config(dir.path(), "luxe", &["global-check"]);
    write_gate_config(dir.path(), ".luxe", &["project-check"]);
    let config = Config::load_trusted(dir.path(), true);
    std::fs::remove_file(dir.path().join("luxe/config.json")).unwrap();
    let project_only = Config::load_trusted(dir.path(), true);
    std::env::remove_var("XDG_CONFIG_HOME");
    assert_eq!(config.gate.commands(), vec![vec!["project-check".to_string()]]);
    assert_eq!(project_only.gate.commands(), config.gate.commands());
}

/// Dropping the global timeout fallback must fail when the project leaves its timeout unset.
#[test]
fn an_unset_project_gate_timeout_uses_the_global_value() {
    let _lock = XDG_CONFIG_HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path());
    write_gate_config(dir.path(), "luxe", &["global-check"]);
    std::fs::create_dir_all(dir.path().join(".luxe")).unwrap();
    std::fs::write(
        dir.path().join(".luxe/config.json"),
        r#"{"gate":{"commands":[["project-check"]]}}"#,
    )
    .unwrap();
    let untrusted = Config::load(dir.path());
    let trusted = Config::load_trusted(dir.path(), true);
    std::env::remove_var("XDG_CONFIG_HOME");
    assert_eq!(untrusted.gate.timeout(), std::time::Duration::from_secs(90));
    assert_eq!(trusted.gate.timeout(), std::time::Duration::from_secs(90));
}

/// An absent or empty project list leaves the global commands intact, even with trust.
#[test]
fn global_gate_commands_do_not_need_project_trust() {
    let _lock = XDG_CONFIG_HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path());
    write_gate_config(dir.path(), "luxe", &["global-check"]);
    for project_present in [false, true] {
        if project_present {
            write_gate_config(dir.path(), ".luxe", &[]);
        }
        for trust in [false, true] {
            let config = Config::load_trusted(dir.path(), trust);
            assert_eq!(config.gate.commands(), vec![vec!["global-check".to_string()]]);
        }
    }
    std::env::remove_var("XDG_CONFIG_HOME");
}
