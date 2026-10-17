//! MCP commands — project servers must not start without the shared tooling trust grant.

use std::path::Path;
use std::process::Command;

mod list;
mod login;

fn project_server(project: &Path) {
    let config = serde_json::json!({"mcpServers": {"project-marker": {
        "command": luxe_core::paths::display(&std::env::current_exe().unwrap()),
        "args": ["--exact", "mcp_marker_child", "--ignored"],
        "enabled": true
    }}});
    std::fs::write(project.join(".mcp.json"), config.to_string()).unwrap();
}

fn tools(project: &Path, home: &Path, trust: bool) -> String {
    run_mcp(project, home, trust, "tools")
}

fn run_mcp(project: &Path, home: &Path, trust: bool, subcommand: &str) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_luxe"));
    if trust {
        command.arg("--trust-project");
    }
    let output = command
        .args(["mcp", subcommand])
        .current_dir(project)
        .env("XDG_CONFIG_HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("LUXE_MODELS_DEV_REFRESH", "0")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn tooling_preference(directory: &Path, preference: &str) {
    std::fs::create_dir_all(directory).unwrap();
    let config = serde_json::json!({"lsp": {"project_servers": preference}});
    std::fs::write(directory.join("config.json"), config.to_string()).unwrap();
}

#[test]
#[ignore = "subprocess fixture explicitly run by the MCP trust tests"]
fn mcp_marker_child() {
    std::fs::write("mcp-ran", "started").unwrap();
}

/// Unfiltered CLI discovery used to execute a command from an untrusted project's `.mcp.json`.
#[test]
fn tools_does_not_start_an_untrusted_project_server() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    project_server(project.path());
    let stdout = tools(project.path(), home.path(), false);
    assert!(!project.path().join("mcp-ran").exists(), "the project command ran: {stdout}");
    assert!(stdout.contains("project-marker") && stdout.contains("--trust-project"), "{stdout}");
}

/// Refusing every project server would silently discard the CLI's explicit trust grant.
#[test]
fn tools_starts_a_project_server_with_explicit_trust() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    project_server(project.path());
    let stdout = tools(project.path(), home.path(), true);
    assert!(project.path().join("mcp-ran").exists(), "the trusted command never ran: {stdout}");
}

/// CLI tools must honor the same global opt-in the engine uses.
#[test]
fn tools_honors_the_user_always_preference() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    project_server(project.path());
    tooling_preference(&home.path().join("luxe"), "always");
    let stdout = tools(project.path(), home.path(), false);
    assert!(project.path().join("mcp-ran").exists(), "the user opt-in was ignored: {stdout}");
}

/// Explicit trust must not override the user's global refusal to start project tooling.
#[test]
fn tools_honors_the_user_never_preference_even_with_trust() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    project_server(project.path());
    tooling_preference(&home.path().join("luxe"), "never");
    let stdout = tools(project.path(), home.path(), true);
    assert!(!project.path().join("mcp-ran").exists(), "the user refusal was ignored: {stdout}");
}

/// A repository must not grant itself permission by setting the global-only tooling preference.
#[test]
fn tools_ignores_a_project_always_preference() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    project_server(project.path());
    tooling_preference(&project.path().join(".luxe"), "always");
    let stdout = tools(project.path(), home.path(), false);
    assert!(!project.path().join("mcp-ran").exists(), "the project granted itself trust: {stdout}");
}

/// The trust filter must not block a server declared by the user, outside the project.
#[test]
fn tools_starts_a_user_server_without_project_trust() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    project_server(project.path());
    std::fs::create_dir_all(home.path().join("luxe")).unwrap();
    std::fs::rename(project.path().join(".mcp.json"), home.path().join("luxe/mcp.json")).unwrap();
    let stdout = tools(project.path(), home.path(), false);
    assert!(project.path().join("mcp-ran").exists(), "the user command never ran: {stdout}");
}
