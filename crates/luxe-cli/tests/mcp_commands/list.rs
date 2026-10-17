use std::path::Path;

use super::{project_server, run_mcp};

fn user_server(home: &Path, id: &str) {
    let config = serde_json::json!({"mcpServers": {id: {
        "command": luxe_core::paths::display(&std::env::current_exe().unwrap()),
        "args": ["--exact", "mcp_marker_child", "--ignored"]
    }}});
    std::fs::create_dir_all(home.join("luxe")).unwrap();
    std::fs::write(home.join("luxe/mcp.json"), config.to_string()).unwrap();
}

/// Filtering the list would hide project entries; omitting the label would hide their origin.
#[test]
fn list_keeps_and_marks_an_untrusted_project_server() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    project_server(project.path());
    let stdout = run_mcp(project.path(), home.path(), false, "list");
    assert!(stdout.contains("project-marker"), "{stdout}");
    assert!(stdout.contains("enabled not started (project)"), "{stdout}");
    assert!(!project.path().join("mcp-ran").exists(), "listing must never start a server");
}

/// The project label must not be attached to entries supplied by the user.
#[test]
fn list_does_not_mark_a_user_server_as_project_supplied() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    user_server(home.path(), "user-marker");
    let stdout = run_mcp(project.path(), home.path(), false, "list");
    assert!(stdout.contains("user-marker") && stdout.contains("enabled"), "{stdout}");
    assert!(!stdout.contains("(project)"), "{stdout}");
    assert!(!project.path().join("mcp-ran").exists(), "listing must never start a server");
}

/// Trust changes which commands may run, not who declared an entry or what a listing executes.
#[test]
fn list_still_marks_and_does_not_start_a_trusted_project_server() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    project_server(project.path());
    let stdout = run_mcp(project.path(), home.path(), true, "list");
    assert!(
        stdout.contains("project-marker") && stdout.contains("not started (project)"),
        "{stdout}"
    );
    assert!(!project.path().join("mcp-ran").exists(), "trusted listing must not execute a server");
}

/// Disabled entries must retain both their state and their project attribution.
#[test]
fn list_keeps_disabled_project_entries() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let config = serde_json::json!({"mcpServers": {"disabled-project": {
        "command": "unused-disabled-command", "enabled": false
    }}});
    std::fs::write(project.path().join(".mcp.json"), config.to_string()).unwrap();
    let stdout = run_mcp(project.path(), home.path(), false, "list");
    assert!(stdout.contains("disabled-project"), "{stdout}");
    assert!(stdout.contains("disabled not started (project)"), "{stdout}");
}

/// A project override must not inherit the user's attribution from the shadowed entry.
#[test]
fn list_marks_a_project_override_of_a_user_server() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    user_server(home.path(), "project-marker");
    project_server(project.path());
    let stdout = run_mcp(project.path(), home.path(), false, "list");
    assert!(stdout.contains("Configured MCP servers (1)"), "{stdout}");
    assert!(
        stdout.contains("project-marker") && stdout.contains("not started (project)"),
        "{stdout}"
    );
}
