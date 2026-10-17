//! A DAP client over an adapter transport.
//!
//! Spawns (or dials in to) a debug adapter, correlates requests to responses by `seq`/`request_seq`,
//! folds events (`stopped`, `output`, `terminated`, …) into shared session state, and auto-replies
//! to reverse requests so the adapter never stalls. The transport is boxed so the same client drives
//! **stdio** adapters (debugpy, CodeLLDB, gdb) and **socket** adapters (Delve `dlv dap`, js-debug)
//! uniformly. Methods take `&self` (seq is atomic, writer/pending are behind mutexes) so a session
//! can be driven from an `Arc<DapClient>`.

use crate::protocol::{ProtocolMessage, StoppedBody};
use luxe_core::jsonrpc_frame::write_frame;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex as SyncMutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex, Notify};
use tracing::error;

/// Boxed write half of a DAP transport (child stdin, or a TCP write half).
type BoxWriter = Box<dyn AsyncWrite + Send + Unpin>;
/// Boxed read half of a DAP transport (child stdout, or a TCP read half).
type BoxReader = Box<dyn AsyncRead + Send + Unpin>;

/// Cap on captured program output so a chatty program can't grow the buffer unbounded.
const MAX_OUTPUT_BYTES: usize = 64 * 1024;

mod reader;

use reader::{display_output, drain_child_logs, read_loop, StderrHead};

/// Requests written and not yet answered, by seq.
type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<ProtocolMessage>>>>;

/// A written-but-not-yet-awaited request: its seq and the response channel.
pub(crate) struct PendingRequest {
    seq: i64,
    rx: oneshot::Receiver<ProtocolMessage>,
}

/// Live, event-folded session state shared with the background reader.
#[derive(Default)]
struct DapState {
    /// The adapter sent the `initialized` event (ready for breakpoint config).
    initialized: bool,
    /// The program terminated or the debug session ended.
    terminated: bool,
    /// Most recent `stopped` event (breakpoint/step/exception), cleared on continue.
    stopped: Option<StoppedBody>,
    /// Captured program output (bounded).
    output: String,
    /// Category of the last captured chunk (`stdout`, `stderr`, `console`, …).
    ///
    /// DAP `output` events don't align with line boundaries, so a chunk that doesn't end in a
    /// newline is a genuine partial line and must not be broken up. But two *different* streams
    /// interleaving mid-line — an adapter `console` note landing inside the program's stdout —
    /// silently glues unrelated words together. Tracking the category lets the display split
    /// exactly those boundaries and no others.
    last_output_category: Option<String>,
    /// Byte offsets in `output` where a category switch glued two partial lines together.
    output_breaks: Vec<usize>,
    /// The adapter's stream has ended, so nothing more will be read: no answer, and no event.
    ///
    /// Not `terminated`, which the program's end also sets while the adapter can still answer
    /// a `disconnect`.
    disconnected: bool,
}

/// Shared client/reader state.
struct Shared {
    state: SyncMutex<DapState>,
    notify: Notify,
}

impl Shared {
    fn update(&self, f: impl FnOnce(&mut DapState)) {
        if let Ok(mut state) = self.state.lock() {
            f(&mut state);
        }
        self.notify.notify_waiters();
    }
}

/// A running debug adapter connection.
pub struct DapClient {
    child: Mutex<Option<Child>>,
    /// The adapter's pid, used to SIGKILL the whole process group on drop so the debuggee the
    /// adapter spawned is reaped too (`kill_on_drop` only reaps the adapter itself).
    group_pid: Option<u32>,
    writer: Arc<Mutex<BoxWriter>>,
    next_seq: AtomicI64,
    pending: Pending,
    shared: Arc<Shared>,
    stderr: Arc<StderrHead>,
}

impl DapClient {
    /// Spawn a **stdio** adapter (debugpy, CodeLLDB, gdb) and start the background reader.
    pub async fn spawn(command: &[String], cwd: &std::path::Path) -> Result<Self, DapError> {
        let program = command.first().ok_or(DapError::EmptyCommand)?;
        let mut cmd = Command::new(program);
        cmd.args(&command[1..]);
        cmd.current_dir(cwd);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd.kill_on_drop(true);
        // Own process group so dropping the client reaps the debuggee the adapter spawns, not just
        // the adapter (matches the bash tool's discipline).
        luxe_core::program::detach_from_terminal(&mut cmd);

        let mut child = cmd.spawn().map_err(|e| DapError::SpawnFailed {
            adapter: program.clone(),
            error: e.to_string(),
        })?;
        let stdin = child.stdin.take().ok_or(DapError::Io("adapter stdin unavailable".into()))?;
        let stdout =
            child.stdout.take().ok_or(DapError::Io("adapter stdout unavailable".into()))?;
        let stderr = drain_child_logs(&mut child);
        Ok(Self::start(Some(child), Box::new(stdin), Box::new(stdout), stderr))
    }

    /// Connect to a **socket** adapter that dials in to us (Delve `dlv dap --client-addr`,
    /// js-debug). We bind a free loopback port, substitute it into `command` at `addr_placeholder`,
    /// spawn the adapter, and accept its connection — race-free (the OS picked the port).
    pub async fn connect_dial_in(
        command: &[String],
        cwd: &std::path::Path,
        addr_placeholder: &str,
    ) -> Result<Self, DapError> {
        let program = command.first().ok_or(DapError::EmptyCommand)?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| DapError::Io(format!("failed to bind DAP listener: {e}")))?;
        let addr = listener.local_addr().map_err(|e| DapError::Io(e.to_string()))?.to_string();

        let args: Vec<String> =
            command[1..].iter().map(|a| a.replace(addr_placeholder, &addr)).collect();
        let mut cmd = Command::new(program);
        cmd.args(&args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        luxe_core::program::detach_from_terminal(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| DapError::SpawnFailed {
            adapter: program.clone(),
            error: e.to_string(),
        })?;
        let stderr = drain_child_logs(&mut child);

        let (stream, _) = tokio::time::timeout(Duration::from_secs(15), listener.accept())
            .await
            .map_err(|_| {
                DapError::Io(format!(
                    "debug adapter '{program}' did not dial in (check it's installed and the \
                     target builds; see dap.adapter logs)"
                ))
            })?
            .map_err(|e| DapError::Io(format!("accept from adapter failed: {e}")))?;
        let (read_half, write_half) = stream.into_split();
        Ok(Self::start(Some(child), Box::new(write_half), Box::new(read_half), stderr))
    }

    /// Wire up shared state + the background reader over an already-connected transport.
    fn start(
        child: Option<Child>,
        writer: BoxWriter,
        reader: BoxReader,
        stderr: Arc<StderrHead>,
    ) -> Self {
        let group_pid = child.as_ref().and_then(Child::id);
        let writer = Arc::new(Mutex::new(writer));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let shared =
            Arc::new(Shared { state: SyncMutex::new(DapState::default()), notify: Notify::new() });

        let reader_writer = writer.clone();
        let reader_pending = pending.clone();
        let reader_shared = shared.clone();
        tokio::spawn(async move {
            let mut reader = tokio::io::BufReader::new(reader);
            if let Err(e) =
                read_loop(&mut reader, reader_shared, reader_writer, reader_pending).await
            {
                error!("dap reader task exited: {e}");
            }
        });

        Self {
            child: Mutex::new(child),
            group_pid,
            writer,
            next_seq: AtomicI64::new(1),
            pending,
            shared,
            stderr,
        }
    }

    /// Send a request and await its response, failing on a non-success reply.
    pub async fn request(
        &self,
        command: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<Value, DapError> {
        let pending = self.write_request(command, arguments).await?;
        self.await_response(pending, command, timeout).await
    }

    /// Write a request now and return a handle to await its response later.
    ///
    /// Splitting send from await lets the launch/attach choreography guarantee `launch`/`attach`
    /// reaches the adapter **before** `configurationDone`, while its response (which only arrives
    /// after) is awaited afterward. Sending it inline (not in a spawned task) avoids a write-order
    /// race on stdin.
    pub(crate) async fn write_request(
        &self,
        command: &str,
        arguments: Value,
    ) -> Result<PendingRequest, DapError> {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let mut message = json!({ "seq": seq, "type": "request", "command": command });
        if !arguments.is_null() {
            message["arguments"] = arguments;
        }
        let payload = serde_json::to_vec(&message).map_err(|e| DapError::Io(e.to_string()))?;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(seq, tx);
        // the reader marks itself done before it clears the map, so one of the two catches this
        let refused = match self.shared.state.lock() {
            Ok(state) => state.disconnected.then_some(DapError::Cancelled),
            Err(_) => Some(DapError::Poisoned),
        };
        if let Some(error) = refused {
            self.pending.lock().await.remove(&seq);
            return Err(error);
        }
        let written = {
            let mut guard = self.writer.lock().await;
            write_frame(&mut *guard, &payload).await
        };
        if let Err(error) = written {
            self.pending.lock().await.remove(&seq);
            return Err(DapError::Io(error.to_string()));
        }
        Ok(PendingRequest { seq, rx })
    }

    /// Await a previously-written request's response, failing on a non-success reply or timeout.
    pub(crate) async fn await_response(
        &self,
        pending: PendingRequest,
        command: &str,
        timeout: Duration,
    ) -> Result<Value, DapError> {
        let response = match tokio::time::timeout(timeout, pending.rx).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => return Err(DapError::Cancelled),
            Err(_) => {
                self.pending.lock().await.remove(&pending.seq);
                return Err(DapError::Timeout { command: command.to_string() });
            }
        };
        if response.success == Some(false) {
            return Err(DapError::RequestFailed {
                command: command.to_string(),
                message: response.message.unwrap_or_else(|| "request failed".to_string()),
            });
        }
        Ok(response.body.unwrap_or(Value::Null))
    }

    /// Wait until the adapter emits `initialized`, or time out; an adapter that goes away first
    /// is `Cancelled` at once.
    pub async fn wait_for_initialized(&self, timeout: Duration) -> Result<(), DapError> {
        self.wait_until(timeout, |s| s.initialized || s.disconnected).await?;
        let initialized = self.shared.state.lock().map_err(|_| DapError::Poisoned)?.initialized;
        if initialized {
            Ok(())
        } else {
            Err(DapError::Cancelled)
        }
    }

    /// Wait until the session stops or terminates. Returns the stop body, or `None` on terminate.
    pub async fn wait_for_stop_or_terminate(
        &self,
        timeout: Duration,
    ) -> Result<Option<StoppedBody>, DapError> {
        self.wait_until(timeout, |s| s.stopped.is_some() || s.terminated).await?;
        let state = self.shared.state.lock().map_err(|_| DapError::Poisoned)?;
        Ok(state.stopped.clone())
    }

    /// Poll a predicate over shared state until true or the timeout elapses.
    async fn wait_until(
        &self,
        timeout: Duration,
        pred: impl Fn(&DapState) -> bool,
    ) -> Result<(), DapError> {
        let start = Instant::now();
        loop {
            {
                let state = self.shared.state.lock().map_err(|_| DapError::Poisoned)?;
                if pred(&state) {
                    return Ok(());
                }
            }
            if start.elapsed() >= timeout {
                return Err(DapError::Timeout { command: "event".to_string() });
            }
            tokio::select! {
                _ = self.shared.notify.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
        }
    }

    /// The start of what the adapter wrote to stderr, trimmed: usually why it failed to start.
    ///
    /// Waits a moment for stderr to close, since an adapter that just died has said its last
    /// words there and the drain may not have read them yet.
    pub async fn adapter_stderr(&self) -> String {
        let deadline = Instant::now() + Duration::from_millis(500);
        while !self.stderr.closed.load(Ordering::Acquire) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        self.stderr.text.lock().map(|text| text.trim().to_owned()).unwrap_or_default()
    }

    /// Snapshot: has the session terminated?
    pub fn is_terminated(&self) -> bool {
        self.shared.state.lock().map(|s| s.terminated).unwrap_or(true)
    }

    /// Snapshot: the latest stop, if any.
    pub fn last_stop(&self) -> Option<StoppedBody> {
        self.shared.state.lock().ok().and_then(|s| s.stopped.clone())
    }

    /// Clear the recorded stop (call when resuming execution).
    pub fn clear_stop(&self) {
        self.shared.update(|s| s.stopped = None);
    }

    /// Snapshot of captured program output, byte-for-byte as the adapter sent it.
    pub fn output(&self) -> String {
        self.shared.state.lock().map(|s| s.output.clone()).unwrap_or_default()
    }

    /// Captured output for *display*: identical to [`Self::output`] except that a mid-line switch
    /// between output streams is broken onto its own line, so an adapter note can't glue itself
    /// onto the end of a program's partial line.
    pub fn output_for_display(&self) -> String {
        self.shared.state.lock().map(|state| display_output(&state)).unwrap_or_default()
    }

    /// Forget everything captured so far, because none of it came from the program.
    ///
    /// Called once the adapter has answered `initialize` and before the program is launched or
    /// attached to. Up to that moment anything on the wire is the *adapter* talking: `gdb
    /// --interpreter=dap` opens with twelve lines of version and GPL notice, tagged `stdout` like
    /// real program output, and none of it belongs in the model's context.
    pub fn discard_output_so_far(&self) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.output.clear();
            state.last_output_category = None;
            state.output_breaks.clear();
        }
    }

    /// Terminate the adapter and reap the process group (adapter + debuggee).
    pub async fn shutdown(&self) {
        let _ = self
            .request("disconnect", json!({ "terminateDebuggee": true }), Duration::from_secs(3))
            .await;
        if let Some(pid) = self.group_pid {
            luxe_core::program::kill_tree(pid);
        }
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }
}

impl Drop for DapClient {
    /// Reap the whole process group so a dropped client never leaks the adapter or its debuggee.
    fn drop(&mut self) {
        if let Some(pid) = self.group_pid {
            luxe_core::program::kill_tree(pid);
        }
    }
}

/// DAP client errors.
#[derive(Debug, thiserror::Error)]
pub enum DapError {
    #[error("empty adapter command")]
    EmptyCommand,

    #[error("failed to spawn debug adapter '{adapter}': {error}")]
    SpawnFailed { adapter: String, error: String },

    #[error("io error: {0}")]
    Io(String),

    #[error("request '{command}' timed out")]
    Timeout { command: String },

    #[error("request '{command}' failed: {message}")]
    RequestFailed { command: String, message: String },

    #[error("request cancelled")]
    Cancelled,

    #[error("session state poisoned")]
    Poisoned,

    #[error("no such debug session")]
    NoSession,

    /// No adapter can run what was asked for: an unknown id, one that is not installed, or no
    /// native adapter at all
    #[error("{0}")]
    AdapterUnavailable(String),

    /// A launch or attach step failed, with the adapter's stderr when it had exited
    #[error("{0}")]
    StartFailed(String),

    /// No thread was named and none has stopped
    #[error("no active thread")]
    NoThread,

    /// A resume was accepted and no stop followed
    #[error(
        "{command} was accepted but the program has not stopped after {secs}s — it is still \
         running. Use action=pause to stop it, action=output to see what it has printed, or set \
         {env} higher if the next stop is simply far away."
    )]
    StillRunning {
        /// The request that was accepted
        command: String,
        /// How long the wait was
        secs: u64,
        /// The environment variable that lengthens it
        env: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use super::reader::apply_event;
    use super::*;

    fn output_event(text: &str, category: &str) -> ProtocolMessage {
        serde_json::from_value(json!({
            "seq": 1,
            "type": "event",
            "event": "output",
            "body": { "output": text, "category": category },
        }))
        .expect("event parses")
    }

    fn shared() -> Shared {
        Shared { state: SyncMutex::new(DapState::default()), notify: Notify::new() }
    }

    /// The break this catches: the cut that keeps the newest output fell inside a multibyte char,
    /// panicked under the state lock, poisoned it, and the session read as over while it ran.
    #[test]
    fn output_past_the_cap_is_cut_on_a_char_boundary() {
        let shared = shared();
        // an odd byte on either side puts the cut inside an `é`
        apply_event(&shared, &output_event("x", "stdout"));
        apply_event(&shared, &output_event(&format!("{}y", "é".repeat(35_000)), "stdout"));
        assert!(!shared.state.is_poisoned(), "the reader survived");
        let state = shared.state.lock().unwrap();
        assert!(state.output.len() <= MAX_OUTPUT_BYTES, "{}", state.output.len());
        assert!(state.output.trim_end_matches('y').chars().all(|c| c == 'é'), "whole chars only");
    }

    /// A writer whose every write fails, as a closed pipe's does.
    struct Refusing;

    impl tokio::io::AsyncWrite for Refusing {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            _: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    /// A request whose write fails leaves no pending entry behind for nobody to answer.
    #[tokio::test]
    async fn a_failed_write_leaves_no_pending_entry() {
        let (_adapter, ours) = tokio::io::duplex(64);
        let client = DapClient::start(None, Box::new(Refusing), Box::new(ours), Arc::default());
        let written = client.write_request("threads", Value::Null).await;
        assert!(matches!(written, Err(DapError::Io(_))), "{:?}", written.err());
        assert!(client.pending.lock().await.is_empty());
    }

    /// A poisoned state lock is reported as poisoned, not as an adapter that exited, and the
    /// request leaves nothing pending.
    #[tokio::test]
    async fn a_poisoned_state_is_poisoned_not_an_exit() {
        let (_adapter, ours) = tokio::io::duplex(64);
        let (writer, _) = tokio::io::duplex(64);
        let client = DapClient::start(None, Box::new(writer), Box::new(ours), Arc::default());
        let shared = Arc::clone(&client.shared);
        let poisoner = std::thread::spawn(move || {
            let _held = shared.state.lock().unwrap();
            panic!("poison the state lock");
        });
        assert!(poisoner.join().is_err());
        let written = client.write_request("threads", Value::Null).await;
        assert!(matches!(written, Err(DapError::Poisoned)), "{:?}", written.err());
        assert!(client.pending.lock().await.is_empty());
    }

    #[test]
    fn partial_lines_from_one_stream_stay_intact() {
        // `print(..., end="")` and progress bars stream partial lines; splitting them would be
        // wrong, so same-category chunks are concatenated verbatim.
        let shared = shared();
        apply_event(&shared, &output_event("load", "stdout"));
        apply_event(&shared, &output_event("ing...", "stdout"));
        apply_event(&shared, &output_event(" done\n", "stdout"));
        let state = shared.state.lock().unwrap();
        assert_eq!(state.output, "loading... done\n");
        assert!(state.output_breaks.is_empty(), "no boundary inside one stream");
    }

    #[test]
    fn a_stream_switch_mid_line_is_broken_for_display_only() {
        // Two adapter messages must not glue into one nonsense token (`ptvsd` + `debugpy`).
        let shared = shared();
        apply_event(&shared, &output_event("ptvsd", "console"));
        apply_event(&shared, &output_event("debugpy", "stderr"));
        let state = shared.state.lock().unwrap();
        assert_eq!(state.output, "ptvsddebugpy", "captured bytes stay verbatim");
        assert_eq!(state.output_breaks, vec![5], "the boundary is recorded, not inserted");
        drop(state);

        let state = shared.state.lock().unwrap();
        assert_eq!(state.output, "ptvsddebugpy", "the snapshot stays byte-faithful");
        assert_eq!(display_output(&state), "ptvsd\ndebugpy", "display separates the streams");
    }

    #[test]
    fn a_stream_switch_after_a_newline_needs_no_boundary() {
        let shared = shared();
        apply_event(&shared, &output_event("hello\n", "stdout"));
        apply_event(&shared, &output_event("warning\n", "stderr"));
        let state = shared.state.lock().unwrap();
        assert!(state.output_breaks.is_empty(), "the newline already separates them");
        assert_eq!(state.output, "hello\nwarning\n");
    }
}
