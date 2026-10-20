//! End-to-end `DapService` test against a Python DAP adapter stub.
//!
//! The stub (named `codelldb` on a temp PATH so the native-adapter resolver picks it) implements
//! the launch choreography and a small breakpoint/step/inspect session, exercising the client's
//! seq correlation, event folding, and settle waits without a real debugger installed.
// These tests stand up a stub server as an executable shell script, so they are unix-only by
// construction — the thing under test is portable, the fixture is not. Gated at the file level rather
// than per function so a Windows `cargo test` compiles the crate instead of failing on the fixture.
#![cfg(unix)]

use luxe_dap::{BreakpointRequest, DapService, FunctionBreakpoint, SourceBreakpoint};
use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tokio::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

/// One test's changes to the process environment: made under the lock every test in this binary
/// takes, and undone when dropped, even past a failed assertion.
struct TestEnv {
    saved: Vec<(&'static str, Option<OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl TestEnv {
    /// `dir` put in front of `PATH`, so the stub there is the adapter found.
    async fn prepend_path(dir: &Path) -> Self {
        let mut env = Self { saved: Vec::new(), _lock: ENV_LOCK.lock().await };
        let old = std::env::var_os("PATH");
        let dirs = std::iter::once(dir.to_path_buf())
            .chain(old.iter().flat_map(std::env::split_paths))
            .collect::<Vec<_>>();
        env.set("PATH", std::env::join_paths(dirs).expect("a temp dir joins PATH"));
        env
    }

    fn set(&mut self, name: &'static str, value: impl AsRef<OsStr>) {
        self.saved.push((name, std::env::var_os(name)));
        std::env::set_var(name, value);
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        for (name, value) in self.saved.drain(..).rev() {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

const STUB: &str = r#"#!/usr/bin/env python3
import json, os, sys

# A file of this name in the working directory switches on a misbehavior
def mode(name):
    return os.path.exists(name)

seq = 0
def send(obj):
    global seq
    seq += 1
    obj["seq"] = seq
    data = json.dumps(obj).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(data))
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()

def read():
    header = b""
    while b"\r\n\r\n" not in header:
        ch = sys.stdin.buffer.read(1)
        if not ch:
            return None
        header += ch
    n = 0
    for line in header.split(b"\r\n"):
        if line.lower().startswith(b"content-length:"):
            n = int(line.split(b":")[1].strip())
    return json.loads(sys.stdin.buffer.read(n))

def respond(req, body=None):
    send({"type": "response", "request_seq": req["seq"], "success": True,
          "command": req["command"], "body": body or {}})

pending_start = None
while True:
    msg = read()
    if msg is None:
        break
    cmd = msg.get("command")
    if cmd == "initialize" and mode("die_on_initialize"):
        sys.stderr.write("codelldb: this build needs a newer libpython\n")
        sys.stderr.flush()
        sys.exit(3)
    if cmd == "initialize" and mode("deaf_after_initialize"):
        # stops listening first, so the launch that follows always meets a closed pipe
        os.close(0)
        respond(msg, {"supportsConfigurationDoneRequest": True})
        sys.stderr.write("codelldb: stopped reading\n")
        sys.stderr.flush()
        import time
        time.sleep(1)
        sys.exit(6)
    if cmd == "initialize" and mode("quiet_after_initialize"):
        respond(msg, {"supportsConfigurationDoneRequest": True})
        sys.stderr.write("codelldb: no debuggee support\r\non this host\r\n")
        sys.stderr.flush()
        sys.exit(5)
    if cmd == "initialize" and mode("binary_stderr"):
        sys.stderr.buffer.write(b"\xff\xfe not text\n")
        sys.stderr.flush()
    if cmd == "initialize":
        respond(msg, {"supportsConfigurationDoneRequest": True})
        send({"type": "event", "event": "initialized"})
    elif cmd == "launch" or cmd == "attach":
        pending_start = msg  # respond after configurationDone
    elif cmd == "setBreakpoints" and mode("refuse_breakpoints"):
        send({"type": "response", "request_seq": msg["seq"], "success": False,
              "command": cmd, "message": "no such source"})
    elif cmd == "setBreakpoints":
        bps = msg["arguments"]["breakpoints"]
        unbound = mode("unverified_breakpoints")
        respond(msg, {"breakpoints": [
            {"verified": False, "message": "no code at this line"} if unbound and i == 0
            else {"verified": True, "line": b["line"]} for i, b in enumerate(bps)]})
    elif cmd == "setFunctionBreakpoints":
        bps = msg["arguments"]["breakpoints"]
        respond(msg, {"breakpoints": [{"verified": True} for _ in bps]})
    elif cmd == "setVariable":
        respond(msg, {"value": msg["arguments"]["value"]})
    elif cmd == "configurationDone" and mode("refuse_configuration_done"):
        send({"type": "response", "request_seq": msg["seq"], "success": False,
              "command": cmd, "message": "not ready"})
    elif cmd == "configurationDone" and mode("die_after_configuration"):
        respond(msg)
        sys.stderr.write("codelldb: lost the debuggee\n")
        sys.stderr.flush()
        sys.exit(4)
    elif cmd == "configurationDone" and mode("refuse_launch"):
        respond(msg)
        send({"type": "response", "request_seq": pending_start["seq"], "success": False,
              "command": pending_start["command"], "message": "program not found"})
    elif cmd == "configurationDone":
        if mode("binary_stderr"):
            # a drain that stopped at the bad line has closed the pipe, and this raises
            sys.stderr.write("still talking\n")
            sys.stderr.flush()
        respond(msg)
        if pending_start:
            respond(pending_start)
        send({"type": "event", "event": "output", "body": {"category": "stdout", "output": "hello from stub\n"}})
        send({"type": "event", "event": "stopped", "body": {"reason": "breakpoint", "threadId": 1}})
    elif cmd == "threads":
        respond(msg, {"threads": [{"id": 1, "name": "main"}]})
    elif cmd == "stackTrace":
        respond(msg, {"stackFrames": [
            {"id": 1000, "name": "compute", "line": 5, "column": 1,
             "source": {"name": "app.py", "path": "/tmp/app.py"}}
        ]})
    elif cmd == "scopes":
        respond(msg, {"scopes": [{"name": "Locals", "variablesReference": 2000, "expensive": False}]})
    elif cmd == "variables":
        respond(msg, {"variables": [
            {"name": "x", "value": "42", "type": "int", "variablesReference": 0},
            {"name": "label", "value": "'answer'", "type": "str", "variablesReference": 0}
        ]})
    elif cmd == "continue":
        respond(msg, {"allThreadsContinued": True})
        send({"type": "event", "event": "terminated"})
    elif cmd == "disconnect":
        respond(msg)
        break
    else:
        respond(msg)
"#;

fn write_stub(dir: &std::path::Path) -> PathBuf {
    let stub = dir.join("codelldb");
    std::fs::write(&stub, STUB).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    stub
}

#[tokio::test]
async fn launch_stop_inspect_continue_and_terminate() {
    let dir_guard = tempfile::tempdir().expect("temp dir");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    write_stub(&dir);

    let _env = TestEnv::prepend_path(&dir).await;

    let service = DapService::new();
    // A non-.py program resolves to the first native adapter on PATH — our `codelldb` stub.
    let outcome = service
        .launch(
            "/tmp/app".to_string(),
            vec![],
            dir.display().to_string(),
            false,
            vec![BreakpointRequest {
                path: "/tmp/app.py".to_string(),
                breakpoints: vec![SourceBreakpoint { line: 5, condition: Some("x > 0".into()) }],
            }],
            None,
        )
        .await
        .expect("launch should succeed");

    assert_eq!(outcome.adapter_id, "codelldb");
    let stop = outcome.stop.expect("program should stop at the breakpoint");
    assert_eq!(stop.reason, "breakpoint");
    assert!(outcome.output.contains("hello from stub"));

    let session = service.get(&outcome.session_id).await.expect("session exists");

    // Threads + stack.
    assert_eq!(session.threads().await.unwrap()[0].name, "main");
    let frames = session.stack_trace(None).await.unwrap();
    assert_eq!(frames[0].name, "compute");
    assert_eq!(frames[0].line, 5);

    // Scopes + variables (auto-context path).
    let scopes = session.scopes(frames[0].id).await.unwrap();
    assert_eq!(scopes[0].name, "Locals");
    let vars = session.variables(scopes[0].variables_reference).await.unwrap();
    assert_eq!(vars.len(), 2);
    assert_eq!(vars[0].name, "x");
    assert_eq!(vars[0].value, "42");

    // set_variable echoes the adapter's new value.
    let updated = session.set_variable(scopes[0].variables_reference, "x", "99").await.unwrap();
    assert_eq!(updated, "99");

    // Function breakpoints verify.
    let fn_bps = luxe_dap::service::set_function_breakpoints(
        &session.client,
        &[FunctionBreakpoint { name: "compute".into(), condition: None }],
    )
    .await
    .unwrap();
    assert_eq!(fn_bps.len(), 1);
    assert!(fn_bps[0].verified);

    // Continue runs to completion.
    let after = session.continue_(None).await.unwrap();
    assert!(after.is_none(), "continue should run to termination");
    assert!(session.client.is_terminated());

    service.terminate(&outcome.session_id).await.unwrap();
    assert!(service.get(&outcome.session_id).await.is_none());
}

#[tokio::test]
async fn attach_by_pid_reaches_a_stop() {
    let dir_guard = tempfile::tempdir().expect("temp dir");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    write_stub(&dir);

    let _env = TestEnv::prepend_path(&dir).await;

    let service = DapService::new();
    let outcome = service
        .attach(Some(4321), None, dir.display().to_string(), Vec::new(), Some("codelldb".into()))
        .await
        .expect("attach should succeed");
    assert_eq!(outcome.adapter_id, "codelldb");
    assert_eq!(outcome.stop.expect("attach should stop").reason, "breakpoint");

    let session = service.get(&outcome.session_id).await.expect("session exists");
    assert!(session.program.contains("attach pid 4321"));
    service.terminate(&outcome.session_id).await.unwrap();
}

/// A resume the adapter accepts and never reports on says the program is running.
///
/// Adapters do this. `gdb --interpreter=dap` answers `stepIn` with `success: true` and then sends
/// no event at all when there is nowhere to step (confirmed against the raw protocol), and no
/// client can invent a stop that was never reported. What luxe owes is a reply that says which
/// situation this is: the old one was `request 'event' timed out` after the full thirty-second
/// budget, which names an internal wait instead of the program's state and offers nothing to do
/// about it.
///
/// The stub answers any command it does not recognize with a bare success, which is precisely the
/// shape; the budget is turned down so the test costs a second rather than half a minute.
#[tokio::test]
async fn a_resume_that_never_stops_says_the_program_is_running() {
    let dir_guard = tempfile::tempdir().expect("temp dir");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    write_stub(&dir);

    let mut env = TestEnv::prepend_path(&dir).await;
    env.set("LUXE_DAP_STOP_TIMEOUT_SECS", "1");

    let service = DapService::new();
    let outcome = service
        .launch("/tmp/app".to_string(), vec![], "/tmp".to_string(), false, vec![], None)
        .await
        .expect("the stub launches");
    let session = service.get(&outcome.session_id).await.expect("a live session");

    let error = session.step("next", Some(1)).await.expect_err("the stub reports no stop");

    drop(env);
    service.terminate_all().await;

    assert!(matches!(error, luxe_dap::DapError::StillRunning { .. }), "{error:?}");
    let refused = error.to_string();
    assert!(refused.contains("still"), "it has to name the state: {refused}");
    assert!(refused.contains("pause"), "and the action that reaches it: {refused}");
    assert!(
        !refused.contains("'event'"),
        "naming an internal wait tells the caller nothing: {refused}"
    );
}

/// A directory of this test's own, deleted when it is dropped; it reads as the path it holds.
struct Scratch(tempfile::TempDir);

impl std::ops::Deref for Scratch {
    type Target = std::path::Path;

    fn deref(&self) -> &Self::Target {
        self.0.path()
    }
}

/// A temp directory holding the stub, on `PATH` until the [`TestEnv`] drops, with `modes` on.
async fn stub_dir(name: &str, modes: &[&str]) -> (Scratch, TestEnv) {
    let dir = Scratch(
        tempfile::Builder::new().prefix(&format!("luxe-dap-{name}-")).tempdir().expect("temp dir"),
    );
    write_stub(&dir);
    for mode in modes {
        std::fs::write(dir.join(mode), "").unwrap();
    }
    let env = TestEnv::prepend_path(&dir).await;
    (dir, env)
}

async fn launch_in(
    dir: &std::path::Path,
    lines: &[i64],
) -> Result<luxe_dap::LaunchOutcome, String> {
    let breakpoints = vec![BreakpointRequest {
        path: "/tmp/app.py".to_string(),
        breakpoints: lines.iter().map(|&line| SourceBreakpoint { line, condition: None }).collect(),
    }];
    let service = DapService::new();
    let outcome = service
        .launch("/tmp/app".to_string(), vec![], dir.display().to_string(), false, breakpoints, None)
        .await;
    service.terminate_all().await;
    outcome.map_err(|error| error.to_string())
}

/// A breakpoint the adapter would not bind is reported, by line, with the adapter's reason.
///
/// The break this catches: launch sent `setBreakpoints` with `let _ =`, so a refused breakpoint
/// vanished and the model went on believing it was set.
#[tokio::test]
async fn breakpoints_that_did_not_take_are_reported() {
    let (dir, _env) = stub_dir("unverified", &["unverified_breakpoints"]).await;
    let outcome = launch_in(&dir, &[5, 9]).await;
    std::fs::remove_file(dir.join("unverified_breakpoints")).unwrap();
    std::fs::write(dir.join("refuse_breakpoints"), "").unwrap();
    let refused = launch_in(&dir, &[5]).await;

    let outcome = outcome.expect("an unbound breakpoint does not fail the launch");
    assert_eq!(outcome.unverified, ["/tmp/app.py:5: no code at this line"]);
    let refused = refused.expect("a refused request does not fail it either");
    assert_eq!(refused.unverified.len(), 1);
    assert!(
        refused.unverified[0].contains("/tmp/app.py")
            && refused.unverified[0].contains("no such source")
    );
}

/// A refused `configurationDone` from an adapter that advertised it fails the launch, naming the
/// adapter, instead of waiting out "never stopped or exited".
#[tokio::test]
async fn a_refused_configuration_done_fails_the_launch() {
    let (dir, _env) = stub_dir("configuration", &["refuse_configuration_done"]).await;
    let started = std::time::Instant::now();
    let outcome = launch_in(&dir, &[]).await;

    let error = outcome.expect_err("the program never runs");
    assert!(error.contains("codelldb") && error.contains("configurationDone"), "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

/// An adapter that dies while starting says why, from its own stderr, and says it at once.
///
/// The break this catches: a stdio adapter's stderr went to the null device and a dead adapter
/// left its request waiting out the whole timeout, so the answer was a bare timeout.
#[tokio::test]
async fn an_adapter_that_dies_starting_says_why() {
    let (dir, _env) = stub_dir("dies", &["die_on_initialize"]).await;
    let started = std::time::Instant::now();
    let outcome = launch_in(&dir, &[]).await;

    let error = outcome.expect_err("nothing to launch with");
    assert!(error.contains("newer libpython"), "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
}

/// A client over a script that answers nothing, and does `behavior` once its first request lands.
async fn client_over(behavior: &str) -> luxe_dap::DapClient {
    let script = format!("import sys, time\nsys.stdin.buffer.read(1)\n{behavior}\n");
    let command = vec!["python3".to_string(), "-c".to_string(), script];
    luxe_dap::DapClient::spawn(&command, &std::env::temp_dir()).await.expect("spawned")
}

/// A request in flight when the adapter exits fails at once, not at its own timeout.
#[tokio::test]
async fn a_request_fails_at_once_when_the_adapter_exits() {
    let client = client_over("sys.exit(0)").await;
    let started = std::time::Instant::now();
    let error = client
        .request("threads", serde_json::Value::Null, std::time::Duration::from_secs(30))
        .await
        .expect_err("nobody will answer");
    assert!(matches!(error, luxe_dap::DapError::Cancelled), "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    assert!(client.is_terminated());
}

/// A stream that breaks its framing ends the session: requests fail and it reads as over.
///
/// The break this catches: a read error returned before `terminated` was set, so the session
/// never read as ended and every request waited out its timeout.
#[tokio::test]
async fn a_broken_stream_ends_the_session() {
    let client = client_over(
        "sys.stdout.buffer.write(b'Content-Length: nope\\r\\n\\r\\n'); sys.stdout.flush(); time.sleep(30)",
    )
    .await;
    let started = std::time::Instant::now();
    let error = client
        .request("threads", serde_json::Value::Null, std::time::Duration::from_secs(30))
        .await
        .expect_err("nobody will answer");
    assert!(matches!(error, luxe_dap::DapError::Cancelled), "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    assert!(client.is_terminated());

    // and one asked after the stream broke does not wait either
    let started = std::time::Instant::now();
    let again = client
        .request("threads", serde_json::Value::Null, std::time::Duration::from_secs(30))
        .await;
    assert!(again.is_err(), "{again:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
}

/// A launch that fails after configuration says how: an adapter that dies says it exited, with
/// what it printed, and one that refuses passes on its own reason, neither as a 30 s silence.
#[tokio::test]
async fn a_failed_launch_says_how_it_failed() {
    let (dir, _env) = stub_dir("launch-fails", &["die_after_configuration"]).await;
    let died = launch_in(&dir, &[]).await;
    std::fs::remove_file(dir.join("die_after_configuration")).unwrap();
    std::fs::write(dir.join("refuse_launch"), "").unwrap();
    let refused = launch_in(&dir, &[]).await;

    let died = died.expect_err("the adapter is gone");
    assert!(died.contains("exited") && died.contains("lost the debuggee"), "{died}");
    let refused = refused.expect_err("the adapter said no");
    assert!(refused.contains("refused") && refused.contains("program not found"), "{refused}");
}

/// A stderr line that is not text does not stop the drain, which would leave the adapter writing
/// into a closed pipe and dying of it.
#[tokio::test]
async fn stderr_that_is_not_text_keeps_draining() {
    let (dir, _env) = stub_dir("binary-stderr", &["binary_stderr"]).await;
    let outcome = launch_in(&dir, &[]).await;
    outcome.expect("the adapter outlives its own odd bytes");
}

/// An adapter that answers `initialize` and then exits says so at once, with what it printed,
/// instead of waiting out the start timeout and calling it slow.
#[tokio::test]
async fn an_adapter_that_exits_after_initialize_says_so_at_once() {
    let (dir, _env) = stub_dir("quiet", &["quiet_after_initialize"]).await;
    let started = std::time::Instant::now();
    let outcome = launch_in(&dir, &[]).await;

    let error = outcome.expect_err("it is gone");
    assert!(error.contains("exited while starting"), "{error}");
    assert!(error.contains("no debuggee support\non this host"), "{error:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
}

/// An adapter that stops reading after `initialize` is reported as an exit at the launch write,
/// the side of the race the test above only sometimes reaches.
#[tokio::test]
async fn a_launch_write_into_a_closed_adapter_is_an_exit() {
    let (dir, _env) = stub_dir("deaf", &["deaf_after_initialize"]).await;
    let outcome = launch_in(&dir, &[]).await;

    let error = outcome.expect_err("it stopped listening");
    assert!(error.contains("exited while starting"), "{error}");
    assert!(error.contains("stopped reading"), "{error}");
}

/// What went wrong is the variant, not a sentence to search: a caller can tell a debugger that is
/// not installed from a start that failed from a session that is gone.
#[tokio::test]
async fn failures_come_back_as_the_kind_of_failure_they_are() {
    use luxe_dap::DapError;

    let service = DapService::new();
    let unknown = service
        .launch("/tmp/app".into(), vec![], "/tmp".into(), false, vec![], Some("nope".into()))
        .await
        .expect_err("no adapter by that id");
    assert!(matches!(unknown, DapError::AdapterUnavailable(_)), "{unknown:?}");
    let gone = service.terminate("dbg-404").await.expect_err("no such session");
    assert!(matches!(gone, DapError::NoSession), "{gone:?}");

    let (dir, _env) = stub_dir("kinds", &["refuse_configuration_done"]).await;
    let refused = service
        .launch("/tmp/app".into(), vec![], dir.display().to_string(), false, vec![], None)
        .await
        .expect_err("configurationDone is refused");
    assert!(matches!(refused, DapError::StartFailed(_)), "{refused:?}");
}
