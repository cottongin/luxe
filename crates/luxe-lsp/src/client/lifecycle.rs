//! Lifecycle — starting a server, handshaking with it, and stopping it.

use super::*;

/// Keep the server's stderr drained for as long as it runs, retaining a bounded prefix.
///
/// Two jobs, and the second is the load-bearing one. The prefix is what a failed handshake
/// reports. The *draining* is what keeps a talkative server alive: a pipe nobody reads fills at
/// 64 KB and the next write blocks forever, so a server that narrates every request — clangd
/// does, by default — wedges and then dies. Stopping the reader once the prefix is full and
/// cancelling it once the handshake succeeds are the same bug twice.
pub(super) fn drain_stderr(
    stderr: tokio::process::ChildStderr,
) -> (tokio::task::JoinHandle<()>, Arc<Mutex<String>>) {
    /// Enough for a shim's refusal or a stack trace's first frames; a server that logs forever
    /// cannot grow this.
    const MAX: usize = 2_000;
    let collected = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&collected);
    let handle = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut reader = tokio::io::BufReader::new(stderr);
        let mut chunk = [0u8; 4096];
        let mut kept = Vec::new();
        while let Ok(read) = reader.read(&mut chunk).await {
            if read == 0 {
                break;
            }
            if kept.len() < MAX {
                let room = MAX - kept.len();
                kept.extend_from_slice(&chunk[..read.min(room)]);
                *sink.lock().await = String::from_utf8_lossy(&kept).trim().to_string();
            }
            // Past the prefix the bytes are dropped — but they are still read. See above.
        }
    });
    (handle, collected)
}

/// Wait for the child to stop, rendering how it went.
pub(super) async fn wait_for_exit(child: &mut tokio::process::Child) -> String {
    match child.wait().await {
        Ok(status) => status.to_string(),
        Err(error) => format!("could not be waited on: {error}"),
    }
}

/// How the `initialize` handshake ended, so the borrow of `self` can be released before reacting.
pub(super) enum Handshake {
    /// The server answered.
    Answered(Result<serde_json::Value, LspError>),
    /// The server stopped first, rendered as its exit status.
    Exited(String),
    /// Neither happened inside the timeout.
    Silent,
}

/// Reads a `diagnosticProvider` out of an `initialize` result.
///
/// The capability is either an options object or a registration object with an `id`; both carry the
/// optional `identifier` a pull request has to echo back, and neither is a bare boolean.
pub(super) fn pull_provider(init_response: &Value) -> Option<PullProvider> {
    let provider = init_response.get("capabilities")?.get("diagnosticProvider")?;
    provider.is_object().then(|| PullProvider {
        identifier: provider.get("identifier").and_then(Value::as_str).map(str::to_string),
    })
}

/// How long a failed handshake waits for its server's exit status to become collectible.
///
/// Generous next to the gap it covers (a process that has already closed its pipes is reaped in
/// microseconds, milliseconds on a loaded machine) and short enough to be invisible on the only
/// other path that reaches it — a live server that refused the handshake for some other reason.
pub(super) const EXIT_GRACE: Duration = Duration::from_millis(250);

/// How long a server gets to exit after `shutdown` and `exit` before it is killed.
pub(super) const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// What the client tells a server about itself.
///
/// Advertises progress and diagnostics, so a server streams `$/progress` and `publishDiagnostics`.
///
/// And `codeAction`, which is not optional politeness: a server answers `textDocument/codeAction`
/// with an empty list unless the client says it understands code action *literals*, because the
/// older reply shape is a `Command` it cannot assume we can run. Undeclared, rust-analyzer answers
/// every `lsp_navigation action=code_action` with nothing. `resolveSupport` is the other half: an
/// action whose `edit` is computed lazily arrives without one, and applying it means asking for it.
fn initialize_params(root: &std::path::Path) -> Value {
    serde_json::json!({
        "processId": std::process::id(),
        "rootUri": crate::uri::from_path(root),
        "capabilities": {
            "window": { "workDoneProgress": true },
            "textDocument": {
                "synchronization": { "didSave": true, "dynamicRegistration": false },
                "publishDiagnostics": { "relatedInformation": true },
                // The pull half of diagnostics. A server only advertises
                // `diagnosticProvider` when the client says it can ask, and one that answers
                // *only* this way — TypeScript 7's native LSP — pushes nothing for an open
                // file, so without this its errors are invisible and read as "clean".
                // `relatedDocumentSupport: false` keeps an answer to one file about that file.
                "diagnostic": {
                    "dynamicRegistration": false,
                    "relatedDocumentSupport": false
                },
                "codeAction": {
                    "dataSupport": true,
                    "resolveSupport": { "properties": ["edit"] },
                    "codeActionLiteralSupport": {
                        "codeActionKind": {
                            "valueSet": [
                                "", "quickfix", "refactor", "refactor.extract",
                                "refactor.inline", "refactor.rewrite", "source",
                                "source.organizeImports", "source.fixAll"
                            ]
                        }
                    }
                }
            },
            // rust-analyzer signals readiness via this experimental notification.
            "experimental": { "serverStatusNotification": true }
        }
    })
}

/// A server's stderr drain: the task, and what it has kept.
type Complaints = (tokio::task::JoinHandle<()>, Arc<Mutex<String>>);

impl LspClient {
    /// Initialize the language server: spawn the process and perform the LSP handshake.
    pub async fn initialize(&mut self) -> Result<(), LspError> {
        let (mut child, complaints) = self.spawn_server()?;
        let outcome = self.race_handshake(&mut child).await;
        self.child = Some(child);
        let init_response = self.handshake_result(outcome, complaints).await?;
        debug!("LSP server initialized: {init_response:?}");
        self.pull_diagnostics = pull_provider(&init_response);

        self.notify("initialized", serde_json::json!({})).await?;
        // Ready only now: a client whose handshake failed must not report that it can be asked.
        self.state = ClientState::Initialized;
        Ok(())
    }

    /// Starts the server process and the reader that routes what it says, and keeps its stderr.
    fn spawn_server(&mut self) -> Result<(Child, Option<Complaints>), LspError> {
        let mut cmd = Command::new(&self.config.command[0]);
        cmd.args(&self.config.command[1..]);
        cmd.current_dir(&self.config.root_path);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        // Piped, not null: when a server refuses to start it says why here, and that sentence is
        // the whole answer. Discarded, every startup failure would look like a timeout.
        cmd.stderr(std::process::Stdio::piped());
        cmd.kill_on_drop(true);

        let mut child = cmd.spawn().map_err(|e| LspError::SpawnFailed {
            server: self.config.server_id.clone(),
            error: e.to_string(),
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| LspError::IoError(std::io::Error::other("Failed to take stdin")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| LspError::IoError(std::io::Error::other("Failed to take stdout")))?;
        let complaints = child.stderr.take().map(drain_stderr);
        let stdin = Arc::new(Mutex::new(stdin));
        self.stdin = Some(stdin.clone());

        // Background reader must run before the initialize request so the response is routed.
        let tx = self.notification_tx.clone();
        let pending = self.pending_requests.clone();
        let shared = self.shared.clone();
        let mut stdout = tokio::io::BufReader::new(stdout);
        tokio::spawn(async move {
            if let Err(e) = read_loop(&mut stdout, shared, stdin, tx, pending).await {
                error!("LSP reader task exited with error: {e}");
            }
        });
        Ok((child, complaints))
    }

    /// Sends `initialize` and waits for whichever comes first: the answer, the server stopping, or
    /// the timeout.
    async fn race_handshake(&mut self, child: &mut Child) -> Handshake {
        let init_params = initialize_params(&self.config.root_path);
        let init_timeout = self.config.init_timeout;
        // Race the handshake against the server *stopping*. A server that dies in ten milliseconds
        // — a shim with no version set, a missing runtime, a bad config — is not the same as one
        // that is merely slow, and waiting out the whole timeout would tell the caller the wrong
        // thing. Whichever finishes first is the answer.
        let mut outcome = {
            let handshake = self.request("initialize", init_params);
            tokio::pin!(handshake);
            tokio::select! {
                biased;
                response = &mut handshake => Handshake::Answered(response),
                status = wait_for_exit(child) => Handshake::Exited(status),
                () = tokio::time::sleep(init_timeout) => Handshake::Silent,
            }
        };
        // A server can lose the race with its own pipes: die fast enough and the *write* fails
        // before the exit is observable, leaving a bare "Broken pipe". A handshake that
        // never got an answer, against a process that has since exited, *is* that exit — and the
        // exit is the useful half.
        //
        // Waited for rather than polled once. The kernel delivers `EPIPE` the moment the read end
        // closes, but the exit *status* only becomes collectible when this process is notified,
        // which under load lands later — so a single `try_wait` answers "still running" about a
        // process that is already gone and the caller is told about a broken pipe instead. Bounded,
        // because a live server that refused for some other reason must not be waited on for long.
        if matches!(
            &outcome,
            Handshake::Answered(Err(LspError::IoError(_) | LspError::RequestCancelled))
        ) {
            if let Ok(status) = timeout(EXIT_GRACE, wait_for_exit(child)).await {
                outcome = Handshake::Exited(status);
            }
        }
        outcome
    }

    /// The answer to `initialize`, or why there is none: the server's exit with what it said, or
    /// the silence.
    async fn handshake_result(
        &mut self,
        outcome: Handshake,
        complaints: Option<Complaints>,
    ) -> Result<Value, LspError> {
        let server_id = self.config.server_id.clone();
        let init_timeout = self.config.init_timeout;
        let init_response = match outcome {
            Handshake::Exited(status) => {
                let stderr = match complaints {
                    Some((handle, buffer)) => {
                        // The child is gone, so its stderr is at EOF and the drain ends on its own.
                        let _ = handle.await;
                        buffer.lock().await.clone()
                    }
                    None => String::new(),
                };
                return Err(LspError::ServerExited { server: server_id, status, stderr });
            }
            // The drain runs for the life of the server, and the handle is held so shutdown can end
            // it. Cancelling it here instead, to avoid detaching a `JoinHandle`, wedges any server
            // that logs more than a pipe buffer: clangd narrates every request to stderr by
            // default, fills 64 KB during its first index, and blocks forever on the next write.
            other => {
                if let Some((handle, _)) = complaints {
                    self.stderr_drain = Some(handle);
                }
                match other {
                    Handshake::Answered(response) => response?,
                    Handshake::Silent => {
                        return Err(LspError::InitTimeout {
                            server: server_id,
                            timeout: init_timeout,
                        })
                    }
                    Handshake::Exited(_) => unreachable!("handled above"),
                }
            }
        };
        Ok(init_response)
    }

    /// Shut the server down cleanly, sending `shutdown` + `exit` and reaping the child.
    pub async fn shutdown(&mut self) -> Result<(), LspError> {
        if self.state == ClientState::Shutdown {
            return Ok(());
        }
        if let Some(stdin) = self.stdin.clone() {
            let mut guard = stdin.lock().await;
            for message in [
                JsonRpcMessage::request(0, "shutdown".to_string(), serde_json::json!({})),
                JsonRpcMessage::notification("exit".to_string(), serde_json::json!({})),
            ] {
                if let Ok(json) = serde_json::to_string(&message) {
                    let _ = write_frame(&mut *guard, json.as_bytes()).await;
                }
            }
        }
        if let Some(handle) = self.stderr_drain.take() {
            handle.abort();
        }
        if let Some(mut child) = self.child.take() {
            // a server that ignores `exit` must not hold its caller forever
            if timeout(SHUTDOWN_GRACE, child.wait()).await.is_err() {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
        }
        self.state = ClientState::Shutdown;
        Ok(())
    }
}
