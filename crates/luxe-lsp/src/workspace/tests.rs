use super::*;

#[tokio::test]
async fn unsupported_language_yields_no_diagnostics() {
    let client = WorkspaceLspClient::new();
    let diags = client.run_diagnostics(Path::new("notes.md")).await.unwrap();
    assert!(diags.is_empty());
}

#[tokio::test]
async fn shutdown_all_clears_state_and_is_a_noop_when_empty() {
    let client = WorkspaceLspClient::new();
    // No warm servers yet: shutdown_all must not panic and leaves nothing running.
    client.shutdown_all().await;
    assert!(client.running_servers().is_empty());
}

#[tokio::test]
async fn missing_server_degrades_gracefully() {
    // A .rs file with a bogus server binary: init fails fast, so we get no diagnostics
    // and never error, and the second call short-circuits via the unavailable set.
    let dir_guard = tempfile::tempdir().expect("temp dir");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("x.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();
    let mut client = WorkspaceLspClient::new();
    // Force the language server command to a nonexistent binary so init fails fast.
    client.registry.register(
        "rust".to_string(),
        crate::registry::ServerEntry {
            command: "definitely-not-a-real-lsp-binary".to_string(),
            root_markers: vec!["Cargo.toml".to_string()],
            extensions: vec!["rs".to_string()],
            ..Default::default()
        },
    );
    let diags = client.run_diagnostics(&file).await.unwrap();
    assert!(diags.is_empty());
    // Second call short-circuits via the unavailable set.
    let diags = client.run_diagnostics(&file).await.unwrap();
    assert!(diags.is_empty());
}

/// A fetched repository brings its own `Cargo.toml`, so a file in it would root a server there
/// and run that repository's build scripts. Nothing is even attempted.
#[tokio::test]
async fn no_server_is_started_inside_a_fetched_repository() {
    let base_guard = tempfile::tempdir().expect("temp dir");
    let base = base_guard.path().to_path_buf();
    let clone = luxe_core::fetched::cache_dir(&base).join("o").join("r");
    std::fs::create_dir_all(&clone).unwrap();
    std::fs::write(clone.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
    std::fs::write(clone.join("build.rs"), "fn main() {}\n").unwrap();

    let mut client = WorkspaceLspClient::new();
    client.registry.register(
        "rust".to_string(),
        crate::registry::ServerEntry {
            command: "definitely-not-a-real-lsp-binary".to_string(),
            root_markers: vec!["Cargo.toml".to_string()],
            extensions: vec!["rs".to_string()],
            ..Default::default()
        },
    );
    let file = clone.join("build.rs");
    assert!(client.client_for_path(&file).await.is_none());
    assert!(client.warm().unavailable.is_empty(), "refused before any launch was tried");
    assert!(client.missing_server_hint(&file).contains("fetched repository"));
}

#[tokio::test]
async fn a_failed_root_does_not_disable_the_language_everywhere() {
    // One unusable root must not take the language down with it for the whole session.
    let base_guard = tempfile::tempdir().expect("temp dir");
    let base = base_guard.path().to_path_buf();
    let bad = base.join("bad");
    let good = base.join("good");
    for root in [&bad, &good] {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        std::fs::write(root.join("x.rs"), "fn main() {}\n").unwrap();
    }

    let mut client = WorkspaceLspClient::new();
    client.registry.register(
        "rust".to_string(),
        crate::registry::ServerEntry {
            command: "definitely-not-a-real-lsp-binary".to_string(),
            root_markers: vec!["Cargo.toml".to_string()],
            extensions: vec!["rs".to_string()],
            ..Default::default()
        },
    );

    // Fail on the first root.
    assert!(client.client_for_path(&bad.join("x.rs")).await.is_none());
    assert_eq!(client.warm().unavailable.len(), 1, "the failure is recorded for that root only");
    assert!(
        client.warm().unavailable.contains_key(&("rust".to_string(), bad.clone())),
        "keyed by (language, root)"
    );

    // A different root of the same language is still attempted (it fails here too, because the
    // binary is bogus — the point is that it produced its *own* entry rather than being skipped).
    assert!(client.client_for_path(&good.join("x.rs")).await.is_none());
    assert_eq!(client.warm().unavailable.len(), 2, "each root tracks its own failure");

    // A repeat on the failed root is skipped (cooldown) and backs off further.
    let first_attempts = client.warm().unavailable[&("rust".to_string(), bad.clone())].attempts;
    assert!(client.client_for_path(&bad.join("x.rs")).await.is_none());
    assert_eq!(
        client.warm().unavailable[&("rust".to_string(), bad.clone())].attempts,
        first_attempts,
        "a cooled-down root isn't retried yet, so its attempt count holds"
    );

    // The escape hatch clears every cooldown.
    assert_eq!(client.clear_start_failures(), 2);
    assert!(client.warm().unavailable.is_empty());
}

#[tokio::test]
async fn an_expired_cooldown_lets_the_next_request_retry() {
    let dir_guard = tempfile::tempdir().expect("temp dir");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
    let file = dir.join("x.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();

    let mut client = WorkspaceLspClient::new();
    client.registry.register(
        "rust".to_string(),
        crate::registry::ServerEntry {
            command: "definitely-not-a-real-lsp-binary".to_string(),
            root_markers: vec!["Cargo.toml".to_string()],
            extensions: vec!["rs".to_string()],
            ..Default::default()
        },
    );
    assert!(client.client_for_path(&file).await.is_none());
    let key = ("rust".to_string(), dir.clone());
    assert_eq!(client.warm().unavailable[&key].attempts, 1);

    // Pretend the cooldown elapsed: the next request retries and backs off further.
    client.warm().unavailable.get_mut(&key).unwrap().retry_after = Instant::now();
    assert!(client.client_for_path(&file).await.is_none());
    assert_eq!(client.warm().unavailable[&key].attempts, 2, "a retry happened and backed off");
}

/// A failed start's gate goes once the start is done, so the map does not grow with every root a
/// session touches. A successful one is held to the same in the two-caller test below.
#[tokio::test]
async fn a_finished_start_leaves_no_gate_behind() {
    let base = tempfile::tempdir().expect("temp dir");
    let mut client = WorkspaceLspClient::new();
    client.registry.register(
        "rust".to_string(),
        crate::registry::ServerEntry {
            command: "definitely-not-a-real-lsp-binary".to_string(),
            root_markers: vec!["Cargo.toml".to_string()],
            extensions: vec!["rs".to_string()],
            ..Default::default()
        },
    );
    for name in ["one", "two", "three"] {
        let root = base.path().join(name);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        std::fs::write(root.join("x.rs"), "fn main() {}\n").unwrap();
        assert!(client.client_for_path(&root.join("x.rs")).await.is_none());
    }
    assert_eq!(gates(&client), 0, "every start is over, so no gate is left");
}

/// Callers that let go of one gate at the same moment still take it out of the map.
///
/// The break this catches: each checked the count while its own handle was still alive, so two
/// letting go together each saw the other and both left the gate behind.
#[test]
fn a_gate_let_go_of_by_several_callers_at_once_is_removed() {
    let client = WorkspaceLspClient::new();
    let key = ("rust".to_string(), PathBuf::from("/project"));
    for _ in 0..2000 {
        let leases: Vec<_> = (0..8).map(|_| client.gate(&key)).collect();
        let together = std::sync::Barrier::new(leases.len());
        std::thread::scope(|scope| {
            for lease in leases {
                let together = &together;
                scope.spawn(move || {
                    together.wait();
                    drop(lease);
                });
            }
        });
        assert_eq!(gates(&client), 0);
    }
}

/// How many workspaces have a start gate in the map.
fn gates(client: &WorkspaceLspClient) -> usize {
    client.starting.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len()
}

/// A directory of this test's own, named for what it is, gone when it is dropped.
fn project_dir(tag: &str) -> tempfile::TempDir {
    tempfile::Builder::new().prefix(&format!("luxe-lsp-{tag}-")).tempdir().expect("temp dir")
}

/// The binary name the fixtures below install into a project.
///
/// Deliberately not a real server's: the host running these tests may well have `svelteserver`
/// on `PATH`, and then the untrusted case would fall through to *that* and pass for the wrong
/// reason. A name nothing can satisfy globally makes the project copy the only way to launch.
const FIXTURE_SERVER: &str = "luxe-fixture-project-server";

/// A project that installed its own language server, as npm leaves one.
///
/// The binary is a shell script that exits immediately, so the handshake always fails — which
/// is the point: what is under test is which path Luxe *chose to launch*, and the recorded
/// failure names it.
fn project_with_its_own_server(tag: &str) -> tempfile::TempDir {
    let dir = project_dir(tag);
    let root = dir.path().to_path_buf();
    let bin = root.join("node_modules").join(".bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(root.join("package.json"), "{\"name\":\"p\"}\n").unwrap();
    std::fs::write(root.join("App.svelte"), "<p>hi</p>\n").unwrap();
    // npm's Windows shim is a `.cmd`, and a bare name is not runnable there — the same shape
    // the real dev dependency has on each platform.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let server = bin.join(FIXTURE_SERVER);
        std::fs::write(&server, "#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = std::fs::metadata(&server).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&server, perms).unwrap();
    }
    #[cfg(not(unix))]
    std::fs::write(bin.join(format!("{FIXTURE_SERVER}.cmd")), "@echo off\r\nexit /b 0\r\n")
        .unwrap();
    dir
}

/// A client whose `svelte` entry names [`FIXTURE_SERVER`] and nothing else.
fn client_for_the_fixture_server(project_servers: bool) -> WorkspaceLspClient {
    let mut client = WorkspaceLspClient::new().with_project_servers(project_servers);
    client.registry.register(
        "svelte".to_string(),
        crate::registry::ServerEntry {
            command: FIXTURE_SERVER.to_string(),
            root_markers: vec!["package.json".to_string()],
            extensions: vec!["svelte".to_string()],
            install_hint: Some("npm i -g svelte-language-server".to_string()),
            ..Default::default()
        },
    );
    client
}

/// Refused, a server the repository shipped is not launched — and the refusal says so.
///
/// The message is half the feature. "No svelte language server found" while one sits in
/// `node_modules/.bin` is true, useless, and sends the reader off to install what they already
/// have.
#[tokio::test]
async fn a_project_server_is_refused_by_name_when_it_is_not_allowed() {
    let root_dir = project_with_its_own_server("refused");
    let root = root_dir.path().to_path_buf();
    let client = client_for_the_fixture_server(false);

    assert!(client.client_for_path(&root.join("App.svelte")).await.is_none());
    let reason = client.start_failure(&root.join("App.svelte")).expect("a recorded failure");
    assert!(
        reason.contains("node_modules") && reason.contains("--trust-project"),
        "the refusal has to name what it found and how to allow it: {reason}"
    );
    assert!(
        !reason.contains("npm i -g"),
        "the install hint is the one wrong answer here — it is installed: {reason}"
    );
}

/// Allowed, the same binary is the one launched.
///
/// It fails its handshake, which is what the fixture is for: the failure quotes the command,
/// so it is evidence of *which* copy Luxe ran rather than of the server working.
#[tokio::test]
async fn an_allowed_project_server_is_the_one_launched() {
    let root_dir = project_with_its_own_server("allowed");
    let root = root_dir.path().to_path_buf();
    let client = client_for_the_fixture_server(true);

    assert!(client.client_for_path(&root.join("App.svelte")).await.is_none());
    let reason = client.start_failure(&root.join("App.svelte")).expect("a recorded failure");
    assert!(
        reason.contains("did not start"),
        "allowed, it is launched and fails its handshake rather than being refused: {reason}"
    );
    assert!(!reason.contains("--trust-project"), "nothing left to ask permission for: {reason}");
}

/// A directory with a `Cargo.toml` and one Rust file.
#[cfg(unix)]
fn rust_project(tag: &str) -> tempfile::TempDir {
    let dir = project_dir(tag);
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
    std::fs::write(dir.path().join("x.rs"), "fn main() {}\n").unwrap();
    dir
}

/// A project with a `Cargo.toml` and one Rust file, and the language server stub for it: it logs
/// each start to `starts`, and answers `initialize` after `delay_secs`.
#[cfg(unix)]
struct SlowProject {
    _dir: tempfile::TempDir,
    root: PathBuf,
    starts: PathBuf,
    patches: HashMap<String, ServerEntryPatch>,
}

#[cfg(unix)]
impl SlowProject {
    fn new(tag: &str, delay_secs: f64) -> Option<Self> {
        use std::os::unix::fs::PermissionsExt;
        std::process::Command::new("python3").arg("--version").output().ok()?;
        let root_dir = rust_project(tag);
        let root = root_dir.path().to_path_buf();
        let starts = root.join("starts.log");
        let server = root.join("slow-server.py");
        std::fs::write(
            &server,
            format!(
                r#"#!/usr/bin/env python3
import json, os, sys, time
open({starts:?}, "a").write("start\n")
def read_msg():
    header = b""
    while b"\r\n\r\n" not in header:
        b = sys.stdin.buffer.read(1)
        if not b: return None
        header += b
    n = 0
    for line in header.split(b"\r\n"):
        if line.lower().startswith(b"content-length:"):
            n = int(line.split(b":")[1].strip())
    return json.loads(sys.stdin.buffer.read(n))
def write_msg(obj):
    data = json.dumps(obj).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(data) + data)
    sys.stdout.buffer.flush()
while True:
    msg = read_msg()
    if msg is None: break
    if msg.get("method") == "initialize":
        # how long this root's server takes is written beside it, so one script serves two roots
        time.sleep(float(open(os.path.join(os.getcwd(), "delay")).read()))
        write_msg({{"jsonrpc": "2.0", "id": msg["id"], "result": {{"capabilities": {{}}}}}})
    elif msg.get("method") == "shutdown":
        write_msg({{"jsonrpc": "2.0", "id": msg["id"], "result": None}})
    elif msg.get("method") == "exit":
        break
"#,
                starts = starts.display().to_string(),
            ),
        )
        .unwrap();
        std::fs::write(root.join("delay"), delay_secs.to_string()).unwrap();
        std::fs::set_permissions(&server, std::fs::Permissions::from_mode(0o755)).unwrap();
        let patch =
            ServerEntryPatch { command: Some(server.display().to_string()), ..Default::default() };
        Some(Self {
            _dir: root_dir,
            root,
            starts,
            patches: HashMap::from([("rust".to_string(), patch)]),
        })
    }

    fn file(&self) -> PathBuf {
        self.root.join("x.rs")
    }

    fn take_client(&self) -> WorkspaceLspClient {
        WorkspaceLspClient::with_servers(self.patches.clone())
    }

    /// How many times the server has been started.
    fn started(&self) -> usize {
        std::fs::read_to_string(&self.starts).map_or(0, |log| log.lines().count())
    }

    /// Waits for the server to have been launched at least once.
    async fn until_started(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.started() == 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(self.started() > 0, "the server never started");
    }
}

#[cfg(unix)]

/// While one language server is slow to start, the rest of the service answers.
///
/// The break this catches: the service sat behind one lock held across the whole start, so `/lsp`,
/// every other language's tools and the diagnostics hook after an edit all waited out the 20 seconds
/// a cold server may take.
#[cfg(unix)]
#[tokio::test]
async fn a_slow_start_does_not_hold_up_a_status_read() {
    let Some(project) = SlowProject::new("slow", 2.0) else { return };
    let file = project.file();
    let service = Arc::new(project.take_client());
    let starting = tokio::spawn({
        let service = Arc::clone(&service);
        async move { service.client_for_path(&file).await.is_some() }
    });
    project.until_started().await;

    let asked = Instant::now();
    let running = service.running_servers();
    assert!(running.is_empty(), "nothing is warm until the handshake ends");
    assert!(asked.elapsed() < Duration::from_millis(500), "waited {:?}", asked.elapsed());
    assert!(starting.await.unwrap(), "the slow server did come up");
    service.shutdown_all().await;
}

/// Two requests for a cold workspace start one server between them, and share it.
///
/// The break this would be, without the per-workspace gate: with the service shared and no lock
/// across the start, both callers find the root cold and each launches a server for it.
#[cfg(unix)]
#[tokio::test]
async fn two_requests_for_one_cold_root_start_one_server() {
    let Some(project) = SlowProject::new("shared", 0.5) else { return };
    let file = project.file();
    let service = Arc::new(project.take_client());

    let (first, second) =
        tokio::join!(service.client_for_path(&file), service.client_for_path(&file));
    let (first, second) = (first.expect("a server"), second.expect("a server"));

    assert!(Arc::ptr_eq(&first, &second), "the second took what the first started");
    assert_eq!(project.started(), 1, "one process was launched");
    assert_eq!(service.running_servers().len(), 1);
    assert_eq!(gates(&service), 0, "the gate goes with the last caller that waited on it");
    service.shutdown_all().await;
}

/// A server that fails to start tells both of two concurrent callers so, and is tried once.
#[cfg(unix)]
#[tokio::test]
async fn two_requests_for_a_failing_root_try_it_once() {
    use std::os::unix::fs::PermissionsExt;
    let root_dir = rust_project("failing");
    let root = root_dir.path().to_path_buf();
    // exits at once, so the handshake fails; each launch is logged
    let bad = root.join("fails.sh");
    std::fs::write(&bad, "#!/bin/sh\necho start >> \"$0.log\"\nexit 0\n").unwrap();
    std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o755)).unwrap();
    let patch = ServerEntryPatch { command: Some(bad.display().to_string()), ..Default::default() };
    let service =
        Arc::new(WorkspaceLspClient::with_servers(HashMap::from([("rust".to_string(), patch)])));
    let file = root.join("x.rs");

    let (first, second) =
        tokio::join!(service.client_for_path(&file), service.client_for_path(&file));
    assert!(first.is_none() && second.is_none());
    let log = std::fs::read_to_string(root.join("fails.sh.log")).unwrap_or_default();
    assert_eq!(
        log.lines().count(),
        1,
        "the second caller saw the failure rather than repeating it"
    );
    assert_eq!(gates(&service), 0);
}

/// A slow start in one workspace does not hold up another's.
///
/// What the per-workspace gate is for: one gate for the whole service would pass every other test
/// here, and bring back the stall for every language and root behind a cold one.
#[cfg(unix)]
#[tokio::test]
async fn a_slow_start_in_one_root_does_not_hold_up_another() {
    let Some(slow) = SlowProject::new("slow-root", 2.0) else { return };
    // another root of the same language, whose server answers at once
    let quick_dir = rust_project("quick-root");
    let quick = quick_dir.path().to_path_buf();
    std::fs::write(quick.join("delay"), "0").unwrap();
    let service = Arc::new(slow.take_client());
    let slow_file = slow.file();
    let starting = tokio::spawn({
        let service = Arc::clone(&service);
        async move { service.client_for_path(&slow_file).await.is_some() }
    });
    slow.until_started().await;

    let asked = Instant::now();
    let there = service.client_for_path(&quick.join("x.rs")).await;
    assert!(there.is_some(), "the quick root came up");
    assert!(asked.elapsed() < Duration::from_secs(1), "waited {:?}", asked.elapsed());
    assert!(starting.await.unwrap());
    service.shutdown_all().await;
}

/// Shutting the service down while a server is still starting stops that server too.
///
/// The break this catches: the start used to hold the lock the shutdown waited on, so it finished
/// first and was shut down with the rest; once starts stopped holding it, the late server went into
/// the emptied map and lived on into the next project.
#[cfg(unix)]
#[tokio::test]
async fn a_server_that_finishes_starting_after_shutdown_is_not_kept() {
    let Some(project) = SlowProject::new("late", 1.0) else { return };
    let file = project.file();
    let service = Arc::new(project.take_client());
    let starting = tokio::spawn({
        let service = Arc::clone(&service);
        async move { service.client_for_path(&file).await.is_some() }
    });
    project.until_started().await;

    service.shutdown_all().await;
    assert!(!starting.await.unwrap(), "the late server is not handed out");
    assert!(service.running_servers().is_empty(), "and is not kept");
}

/// A second caller waiting on a start that shutdown overtook does not launch a server of its own.
#[cfg(unix)]
#[tokio::test]
async fn a_caller_queued_behind_a_start_does_not_outlive_a_shutdown() {
    let Some(project) = SlowProject::new("queued", 1.0) else { return };
    let service = Arc::new(project.take_client());
    let callers: Vec<_> = (0..2)
        .map(|_| {
            let (service, file) = (Arc::clone(&service), project.file());
            tokio::spawn(async move { service.client_for_path(&file).await.is_some() })
        })
        .collect();
    project.until_started().await;

    service.shutdown_all().await;
    for caller in callers {
        assert!(!caller.await.unwrap(), "neither is handed a server from before the shutdown");
    }
    assert!(service.running_servers().is_empty());
    assert_eq!(project.started(), 1, "the queued caller launched nothing");
}

/// A start that fails after shutdown leaves no cooldown behind: the next request tries again.
#[cfg(unix)]
#[tokio::test]
async fn a_start_that_fails_after_shutdown_records_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let root_dir = rust_project("fails-late");
    let root = root_dir.path().to_path_buf();
    // outlasts the shutdown, then dies without answering
    let slow_failure = root.join("dies.sh");
    std::fs::write(&slow_failure, "#!/bin/sh\necho start >> \"$0.log\"\nsleep 1\nexit 0\n")
        .unwrap();
    std::fs::set_permissions(&slow_failure, std::fs::Permissions::from_mode(0o755)).unwrap();
    let patch = ServerEntryPatch {
        command: Some(slow_failure.display().to_string()),
        ..Default::default()
    };
    let service =
        Arc::new(WorkspaceLspClient::with_servers(HashMap::from([("rust".to_string(), patch)])));
    let file = root.join("x.rs");
    let log =
        || std::fs::read_to_string(root.join("dies.sh.log")).unwrap_or_default().lines().count();
    let starting = tokio::spawn({
        let (service, file) = (Arc::clone(&service), file.clone());
        async move { service.client_for_path(&file).await.is_some() }
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while log() == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    service.shutdown_all().await;
    assert!(!starting.await.unwrap());
    assert!(service.warm().unavailable.is_empty(), "no cooldown for a project that was left");

    assert!(service.client_for_path(&file).await.is_none());
    assert_eq!(log(), 2, "the next request launched the server again");
}

/// The LSP's integers become the core's severities in the spec's order: a swap here would relabel
/// every error as a warning.
#[test]
fn the_lsp_severity_numbers_map_to_the_core_severities_in_the_specs_order() {
    let decoded = |wire: &str| {
        severity_of(serde_json::from_str::<crate::protocol::DiagnosticSeverity>(wire).unwrap())
    };
    assert_eq!(decoded("1"), Severity::Error);
    assert_eq!(decoded("2"), Severity::Warning);
    assert_eq!(decoded("3"), Severity::Information);
    assert_eq!(decoded("4"), Severity::Hint);
}
