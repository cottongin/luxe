//! Debug session management — the warm, per-workspace `DapService`.
//!
//! Owns live [`DapSession`]s keyed by id, runs the DAP launch choreography (initialize → launch →
//! `initialized` event → breakpoints → configurationDone → first stop), and routes control/read
//! actions to a session's client. Mirrors the LSP service pattern: sessions are reused across tool
//! calls rather than re-created per request.

use crate::adapter::{self};
use crate::client::{DapClient, DapError};
use crate::protocol::{Scope, StackFrame, StoppedBody, Thread, Variable};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as SyncMutex};
use std::time::Duration;
use tokio::sync::Mutex;

mod breakpoints;
mod startup;

use breakpoints::parse_array;
pub use breakpoints::{set_breakpoints, set_function_breakpoints};

const STOP_TIMEOUT: Duration = Duration::from_secs(30);
/// Override for [`STOP_TIMEOUT`], for a program whose next stop is genuinely far away.
const STOP_TIMEOUT_ENV: &str = "LUXE_DAP_STOP_TIMEOUT_SECS";

/// How long to wait for a resumed program to stop.
fn stop_timeout() -> Duration {
    std::env::var(STOP_TIMEOUT_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .map_or(STOP_TIMEOUT, Duration::from_secs)
}
const CONTROL_TIMEOUT: Duration = Duration::from_secs(15);

/// A live debug session's status for `/debug` and the dock DEBUG section.
#[derive(Debug, Clone)]
pub struct DapSessionInfo {
    /// Session id (from launch/attach).
    pub id: String,
    /// Backing adapter id.
    pub adapter: String,
    /// Debuggee program / label.
    pub program: String,
    /// Whether the debuggee has terminated.
    pub terminated: bool,
    /// Whether the debuggee is currently paused at a stop (breakpoint/step/exception).
    pub stopped: bool,
    /// Why it stopped, when paused (e.g. `breakpoint`).
    pub stop_reason: Option<String>,
}

/// One source-line breakpoint, optionally conditional.
#[derive(Debug, Clone)]
pub struct SourceBreakpoint {
    /// 1-based line number.
    pub line: i64,
    /// Break only when this expression is true (adapter-evaluated), if set.
    pub condition: Option<String>,
}

/// A source file's breakpoints.
#[derive(Debug, Clone)]
pub struct BreakpointRequest {
    /// Absolute or workspace-relative source path.
    pub path: String,
    /// Breakpoints to set in this file.
    pub breakpoints: Vec<SourceBreakpoint>,
}

/// A breakpoint that triggers on entry to a named function, optionally conditional.
#[derive(Debug, Clone)]
pub struct FunctionBreakpoint {
    /// Function name to break on.
    pub name: String,
    /// Break only when this expression is true, if set.
    pub condition: Option<String>,
}

/// Outcome of a launch: the new session plus where it first stopped (or that it ran to completion).
#[derive(Debug, Clone)]
pub struct LaunchOutcome {
    /// Stable session id for follow-up actions.
    pub session_id: String,
    /// Adapter that backs the session.
    pub adapter_id: String,
    /// First stop, if the program paused (breakpoint / entry / exception).
    pub stop: Option<StoppedBody>,
    /// Whether the program already terminated.
    pub terminated: bool,
    /// Captured program output so far.
    pub output: String,
    /// Breakpoints the adapter did not bind, each with its reason
    pub unverified: Vec<String>,
}

/// `path` as the adapter needs it: absolute, relative ones taken from `base`.
///
/// Adapters resolve a relative path against their own idea of a directory, or not at all: `gdb`
/// leaves `src/lib.rs` pending as "No source file named src/lib.rs".
pub fn resolve(path: &str, base: &str) -> String {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        path.to_string()
    } else {
        Path::new(base).join(candidate).to_string_lossy().into_owned()
    }
}

/// One live debug session.
pub struct DapSession {
    /// Stable session id.
    pub id: String,
    /// Backing adapter id.
    pub adapter_id: String,
    /// Debuggee program (path).
    pub program: String,
    /// Directory the program runs in, which relative breakpoint paths resolve against
    pub cwd: String,
    /// The adapter connection.
    pub client: Arc<DapClient>,
    /// Thread selected by the last stop (default for control actions); interior-mutable so control
    /// methods stay `&self`.
    current_thread: SyncMutex<Option<i64>>,
}

/// Warm, per-workspace debug service.
#[derive(Default)]
pub struct DapService {
    sessions: Mutex<HashMap<String, Arc<DapSession>>>,
    next_id: AtomicU64,
    /// User-configured adapters (extension-routed + selectable by id), shadowing built-ins.
    custom: Vec<adapter::ConfiguredAdapter>,
}

impl DapService {
    /// Create an empty service (built-in adapters only).
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether this service could debug anything at all — i.e. some adapter is installed.
    pub fn has_adapter(&self) -> bool {
        adapter::any_adapter_available(&self.custom)
    }

    /// Create a service with user-configured custom adapters (from config).
    pub fn with_adapters(custom: Vec<adapter::CustomAdapter>) -> Self {
        Self {
            custom: custom.iter().map(adapter::ConfiguredAdapter::from_custom).collect(),
            ..Self::default()
        }
    }

    /// Launch a program under a debug adapter, applying breakpoints, and wait for the first stop.
    pub async fn launch(
        &self,
        program: String,
        args: Vec<String>,
        cwd: String,
        stop_on_entry: bool,
        breakpoints: Vec<BreakpointRequest>,
        adapter: Option<String>,
    ) -> Result<LaunchOutcome, DapError> {
        let spec = adapter::resolve(&program, adapter.as_deref(), &self.custom)?;
        let request_args = spec.launch_arguments(&program, &args, &cwd, stop_on_entry);
        self.start(spec, "launch".to_string(), request_args, cwd, breakpoints, program).await
    }

    /// Attach to a running process (`pid`) or a listening adapter (`port`).
    pub async fn attach(
        &self,
        pid: Option<i64>,
        port: Option<i64>,
        cwd: String,
        breakpoints: Vec<BreakpointRequest>,
        adapter: Option<String>,
    ) -> Result<LaunchOutcome, DapError> {
        // No program path for attach; `resolve("", ...)` picks the native adapter unless overridden.
        let spec = adapter::resolve("", adapter.as_deref(), &self.custom)?;
        let request_args = spec.attach_arguments(pid, port);
        let label = pid
            .map(|p| format!("attach pid {p}"))
            .or_else(|| port.map(|p| format!("attach :{p}")))
            .unwrap_or_else(|| "attach".to_string());
        self.start(spec, "attach".to_string(), request_args, cwd, breakpoints, label).await
    }

    /// Shared launch/attach choreography: initialize → request → `initialized` → breakpoints →
    /// configurationDone → first stop. `label` names the session's program column.
    async fn start(
        &self,
        spec: adapter::AdapterSpec,
        request: String,
        request_args: Value,
        cwd: String,
        breakpoints: Vec<BreakpointRequest>,
        label: String,
    ) -> Result<LaunchOutcome, DapError> {
        // stdio adapters run over the child's pipes; socket adapters dial in over TCP.
        let connected = match &spec.transport {
            adapter::Transport::Stdio => DapClient::spawn(&spec.command, Path::new(&cwd)).await,
            adapter::Transport::TcpDialIn { placeholder } => {
                DapClient::connect_dial_in(&spec.command, Path::new(&cwd), placeholder).await
            }
        };
        let client = Arc::new(connected?);
        let launch = startup::Launch { client: &client, adapter: &spec.id, request: &request };
        let started = launch.handshake(request_args).await?;
        let unverified = launch.configure(&breakpoints, &cwd, started.configuration_done).await?;
        let stop = launch.first_stop(started.pending).await?;

        let id = format!("dbg-{}", self.next_id.fetch_add(1, Ordering::Relaxed) + 1);
        let current_thread = stop.as_ref().and_then(|s| s.thread_id);
        let outcome = LaunchOutcome {
            session_id: id.clone(),
            adapter_id: spec.id.to_string(),
            stop,
            terminated: client.is_terminated(),
            output: client.output_for_display(),
            unverified,
        };
        let session = DapSession {
            id: id.clone(),
            adapter_id: spec.id.to_string(),
            program: label,
            cwd,
            client,
            current_thread: SyncMutex::new(current_thread),
        };
        self.sessions.lock().await.insert(id, Arc::new(session));
        Ok(outcome)
    }

    /// Fetch a session handle by id.
    pub async fn get(&self, id: &str) -> Option<Arc<DapSession>> {
        self.sessions.lock().await.get(id).cloned()
    }

    /// Describes every live session, in id order.
    pub async fn list(&self) -> Vec<DapSessionInfo> {
        let mut out: Vec<_> = self
            .sessions
            .lock()
            .await
            .values()
            .map(|session| {
                let terminated = session.client.is_terminated();
                let last_stop = session.client.last_stop();
                DapSessionInfo {
                    id: session.id.clone(),
                    adapter: session.adapter_id.clone(),
                    program: session.program.clone(),
                    terminated,
                    stopped: !terminated && last_stop.is_some(),
                    stop_reason: last_stop.map(|stop| stop.reason).filter(|r| !r.is_empty()),
                }
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Terminate a session and drop it from the map.
    pub async fn terminate(&self, id: &str) -> Result<(), DapError> {
        let session = self.sessions.lock().await.remove(id).ok_or(DapError::NoSession)?;
        session.client.shutdown().await;
        Ok(())
    }

    /// Terminate every live session (used when leaving a project). Best-effort; the process-group
    /// reaping on each client's drop is the backstop.
    pub async fn terminate_all(&self) {
        let ids: Vec<String> = self.sessions.lock().await.keys().cloned().collect();
        for id in ids {
            let _ = self.terminate(&id).await;
        }
    }
}

impl DapSession {
    /// Resolve the thread to act on: an explicit id, else the last stopped thread.
    fn thread(&self, explicit: Option<i64>) -> Result<i64, DapError> {
        let current = *self.current_thread.lock().expect("session state poisoned");
        explicit.or(current).ok_or(DapError::NoThread)
    }

    /// Record the thread selected by the most recent stop.
    fn remember_stop(&self, stop: &Option<StoppedBody>) {
        if let Some(thread_id) = stop.as_ref().and_then(|s| s.thread_id) {
            *self.current_thread.lock().expect("session state poisoned") = Some(thread_id);
        }
    }

    /// Resume execution; waits for the next stop or termination.
    pub async fn continue_(&self, thread: Option<i64>) -> Result<Option<StoppedBody>, DapError> {
        let thread = self.thread(thread)?;
        self.client.clear_stop();
        self.client.request("continue", json!({ "threadId": thread }), CONTROL_TIMEOUT).await?;
        let stop = self.await_stop("continue").await?;
        self.remember_stop(&stop);
        Ok(stop)
    }

    /// Wait for the stop that `command` was expected to produce, and say what it means if none
    /// comes.
    ///
    /// An adapter can accept a resume, answer `success: true`, and then report nothing — gdb does
    /// exactly that for a `stepIn` with nowhere to step. The program is *running*, so the error
    /// says that, with the two actions that reach a running program, rather than naming a wait.
    async fn await_stop(&self, command: &str) -> Result<Option<StoppedBody>, DapError> {
        let budget = stop_timeout();
        match self.client.wait_for_stop_or_terminate(budget).await {
            Err(DapError::Timeout { .. }) => Err(DapError::StillRunning {
                command: command.to_string(),
                secs: budget.as_secs(),
                env: STOP_TIMEOUT_ENV,
            }),
            other => other,
        }
    }

    /// Step (`next`, `stepIn`, `stepOut`); waits for the next stop.
    pub async fn step(
        &self,
        command: &str,
        thread: Option<i64>,
    ) -> Result<Option<StoppedBody>, DapError> {
        let thread = self.thread(thread)?;
        self.client.clear_stop();
        self.client.request(command, json!({ "threadId": thread }), CONTROL_TIMEOUT).await?;
        let stop = self.await_stop(command).await?;
        self.remember_stop(&stop);
        Ok(stop)
    }

    /// Pause a running thread.
    pub async fn pause(&self, thread: Option<i64>) -> Result<(), DapError> {
        let thread = self.thread(thread)?;
        self.client
            .request("pause", json!({ "threadId": thread }), CONTROL_TIMEOUT)
            .await
            .map(|_| ())
    }

    /// List threads.
    pub async fn threads(&self) -> Result<Vec<Thread>, DapError> {
        let body = self.client.request("threads", Value::Null, CONTROL_TIMEOUT).await?;
        Ok(parse_array(&body, "threads"))
    }

    /// Fetch a thread's stack frames.
    pub async fn stack_trace(&self, thread: Option<i64>) -> Result<Vec<StackFrame>, DapError> {
        let thread = self.thread(thread)?;
        let body = self
            .client
            .request("stackTrace", json!({ "threadId": thread, "levels": 20 }), CONTROL_TIMEOUT)
            .await?;
        Ok(parse_array(&body, "stackFrames"))
    }

    /// Fetch a frame's variable scopes.
    pub async fn scopes(&self, frame_id: i64) -> Result<Vec<Scope>, DapError> {
        let body =
            self.client.request("scopes", json!({ "frameId": frame_id }), CONTROL_TIMEOUT).await?;
        Ok(parse_array(&body, "scopes"))
    }

    /// Fetch variables for a `variablesReference`.
    pub async fn variables(&self, variables_reference: i64) -> Result<Vec<Variable>, DapError> {
        let body = self
            .client
            .request(
                "variables",
                json!({ "variablesReference": variables_reference }),
                CONTROL_TIMEOUT,
            )
            .await?;
        Ok(parse_array(&body, "variables"))
    }

    /// Evaluate an expression in a frame (or globally when `frame_id` is `None`).
    pub async fn evaluate(
        &self,
        expression: &str,
        frame_id: Option<i64>,
    ) -> Result<String, DapError> {
        let mut args = json!({ "expression": expression, "context": "watch" });
        if let Some(frame_id) = frame_id {
            args["frameId"] = json!(frame_id);
        }
        let body = self.client.request("evaluate", args, CONTROL_TIMEOUT).await?;
        Ok(body.get("result").and_then(Value::as_str).unwrap_or("").to_string())
    }

    /// Set a variable's value in a scope/structure; returns the adapter's rendered new value.
    pub async fn set_variable(
        &self,
        variables_reference: i64,
        name: &str,
        value: &str,
    ) -> Result<String, DapError> {
        let body = self
            .client
            .request(
                "setVariable",
                json!({ "variablesReference": variables_reference, "name": name, "value": value }),
                CONTROL_TIMEOUT,
            )
            .await?;
        Ok(body.get("value").and_then(Value::as_str).unwrap_or(value).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn terminate_all_is_a_noop_when_no_sessions() {
        let service = DapService::new();
        service.terminate_all().await;
        assert!(service.list().await.is_empty());
    }
}
