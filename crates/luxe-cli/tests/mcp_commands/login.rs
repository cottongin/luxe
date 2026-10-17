use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::process::Command;

async fn discovery_probe(listener: TcpListener, requests: Arc<AtomicUsize>) {
    // A mismatched resource ends discovery before any browser prompt or token exchange
    let body = r#"{"resource":"https://wrong.example/mcp","authorization_servers":["https://wrong.example"]}"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    loop {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        assert!(socket.read(&mut request).await.unwrap() > 0, "client sent no request");
        requests.fetch_add(1, Ordering::SeqCst);
        socket.write_all(response.as_bytes()).await.unwrap();
    }
}

fn oauth_server(path: &Path, url: &str) {
    let config = serde_json::json!({"mcpServers": {"project-oauth": {
        "url": url, "auth": {"kind": "oauth"}
    }}});
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, config.to_string()).unwrap();
}

fn login_command(project: &Path, home: &Path, trust: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_luxe"));
    if trust {
        command.arg("--trust-project");
    }
    command
        .args(["mcp", "login", "--id", "project-oauth"])
        .current_dir(project)
        .env("XDG_CONFIG_HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env_remove("LUXE_CONFIG_DIR")
        .env_remove("LUXE_STATE_DIR")
        .env("LUXE_MODELS_DEV_REFRESH", "0")
        .env("NO_PROXY", "127.0.0.1")
        .kill_on_drop(true);
    command
}

async fn probe_login(trust: bool, user_server: bool) -> (String, String, usize) {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let path = if user_server {
        home.path().join("luxe/mcp.json")
    } else {
        project.path().join(".mcp.json")
    };
    oauth_server(&path, &url);
    run_probed_login(login_command(project.path(), home.path(), trust), listener).await
}

async fn run_probed_login(mut command: Command, listener: TcpListener) -> (String, String, usize) {
    let requests = Arc::new(AtomicUsize::new(0));
    let probe = tokio::spawn(discovery_probe(listener, requests.clone()));
    let outcome = tokio::time::timeout(Duration::from_secs(10), command.output()).await;
    probe.abort();
    let stopped = probe.await.unwrap_err();
    assert!(stopped.is_cancelled(), "discovery probe failed: {stopped}");
    let output = outcome.expect("login must finish without a browser").unwrap();
    assert!(!output.status.success(), "the probe must stop login: {output:?}");
    (
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
        requests.load(Ordering::SeqCst),
    )
}

/// Unfiltered login used to contact an OAuth endpoint chosen by an untrusted repository.
#[tokio::test]
async fn login_does_not_contact_an_untrusted_project_server() {
    let (stdout, stderr, requests) = probe_login(false, false).await;
    assert_eq!(requests, 0, "untrusted OAuth discovery ran: {stdout}\n{stderr}");
    assert!(stdout.contains("project-oauth") && stdout.contains("--trust-project"), "{stdout}");
    assert!(stderr.contains("not an OAuth-configured HTTP MCP server"), "{stderr}");
}

/// Filtering login must not discard explicit project trust before OAuth discovery.
#[tokio::test]
async fn login_contacts_a_project_server_with_explicit_trust() {
    let (stdout, stderr, requests) = probe_login(true, false).await;
    assert!(requests > 0, "the trusted server was not contacted: {stdout}\n{stderr}");
    assert!(stderr.contains("refusing to send a token"), "{stderr}");
}

/// A user's own OAuth server must remain available without granting the repository trust.
#[tokio::test]
async fn login_contacts_a_user_server_without_project_trust() {
    let (stdout, stderr, requests) = probe_login(false, true).await;
    assert!(requests > 0, "the user's server was not contacted: {stdout}\n{stderr}");
    assert!(stderr.contains("refusing to send a token"), "{stderr}");
}
