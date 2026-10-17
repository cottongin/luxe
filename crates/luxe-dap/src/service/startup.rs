//! Startup — the launch and attach choreography, step by step.
//!
//! initialize → launch/attach → `initialized` → breakpoints → configurationDone → first stop. Each
//! step that can fail says what happened in the user's terms, with whatever the adapter wrote to
//! stderr, because an adapter that cannot start usually explains itself there and nowhere else.

use super::{breakpoints, resolve, stop_timeout, BreakpointRequest, CONTROL_TIMEOUT};
use crate::client::{DapClient, DapError, PendingRequest};
use crate::protocol::StoppedBody;
use serde_json::{json, Value};
use std::time::Duration;

const INIT_TIMEOUT: Duration = Duration::from_secs(10);
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Said when attaching fails quietly, which is how a refused ptrace looks from here.
///
/// An adapter that never answers has not failed in any way it will tell us about: `gdb
/// --interpreter=dap` asked to attach to a process it may not ptrace simply goes quiet. An internal
/// `request 'event' timed out` would name a wait instead of the situation, for the failure a user
/// can most readily fix.
const PTRACE_HINT: &str = " — attaching to a process you did not start needs ptrace permission: \
                           check `sysctl kernel.yama.ptrace_scope` (1 forbids it), or launch the \
                           program from here instead of attaching";

/// What a handshake leaves to finish: the launch or attach reply, and whether the adapter said it
/// takes `configurationDone`.
pub(super) struct Started {
    pub(super) pending: PendingRequest,
    pub(super) configuration_done: bool,
}

/// One launch or attach in progress.
pub(super) struct Launch<'a> {
    pub(super) client: &'a DapClient,
    /// The adapter's id, for messages
    pub(super) adapter: &'a str,
    /// `launch` or `attach`
    pub(super) request: &'a str,
}

impl Launch<'_> {
    /// Initializes the adapter, sends the launch or attach, and waits for `initialized`.
    ///
    /// The request goes out before `configurationDone` but is answered after it, as the protocol's
    /// choreography has it, so its reply is returned to await later. It is written inline rather
    /// than from a spawned task, which is what keeps the two writes in order.
    pub(super) async fn handshake(&self, request_args: Value) -> Result<Started, DapError> {
        let init_args = json!({
            "clientID": "luxe",
            "adapterID": self.adapter,
            "linesStartAt1": true,
            "columnsStartAt1": true,
            "pathFormat": "path",
            "supportsRunInTerminalRequest": false,
        });
        let capabilities = match self.client.request("initialize", init_args, INIT_TIMEOUT).await {
            Ok(capabilities) => capabilities,
            Err(error) => {
                let message = format!("the {} adapter did not initialize: {error}", self.adapter);
                return Err(self.with_stderr(message).await);
            }
        };
        let configuration_done = capabilities
            .get("supportsConfigurationDoneRequest")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        // The program does not exist yet, so anything said so far was the adapter's own.
        self.client.discard_output_so_far();
        // an adapter that answered and died at once loses the race to this write, not to the wait
        let pending = match self.client.write_request(self.request, request_args).await {
            Ok(pending) => pending,
            Err(DapError::Cancelled | DapError::Io(_)) => return Err(self.exited_starting().await),
            Err(error) => return Err(error),
        };
        // Quiet is the failure here; see [`PTRACE_HINT`] for why it is named, not a timeout.
        match self.client.wait_for_initialized(INIT_TIMEOUT).await {
            Ok(()) => {}
            Err(DapError::Cancelled) => return Err(self.exited_starting().await),
            Err(_) => {
                let mut message = format!(
                    "the {} adapter never finished starting ({}s)",
                    self.adapter,
                    INIT_TIMEOUT.as_secs()
                );
                self.hint(&mut message);
                return Err(self.with_stderr(message).await);
            }
        }
        Ok(Started { pending, configuration_done })
    }

    /// Sets the breakpoints and finishes configuration, returning the breakpoints that did not
    /// bind.
    ///
    /// A refused `configurationDone` from an adapter that advertised it is a failed start, since
    /// the program never runs. One that never advertised it is still sent it, and its refusal is
    /// only logged: the protocol says not to ask, and some adapters need asking anyway.
    pub(super) async fn configure(
        &self,
        requests: &[BreakpointRequest],
        cwd: &str,
        advertised: bool,
    ) -> Result<Vec<String>, DapError> {
        let mut unverified = Vec::new();
        for request in requests {
            let path = resolve(&request.path, cwd);
            match breakpoints::set_breakpoints(self.client, &path, &request.breakpoints).await {
                Ok(bound) => {
                    unverified.extend(breakpoints::unverified(&path, &request.breakpoints, &bound));
                }
                Err(error) => unverified.push(format!("{path}: {error}")),
            }
        }
        match self.client.request("configurationDone", Value::Null, CONTROL_TIMEOUT).await {
            Ok(_) => {}
            Err(error) if !advertised => {
                let adapter = self.adapter;
                tracing::debug!(%error, adapter, "unadvertised configurationDone failed");
            }
            Err(error) => {
                let message =
                    format!("the {} adapter refused `configurationDone`: {error}", self.adapter);
                return Err(self.with_stderr(message).await);
            }
        }
        Ok(unverified)
    }

    /// Awaits the launch or attach reply, then the first stop or the end of the program.
    pub(super) async fn first_stop(
        &self,
        pending: PendingRequest,
    ) -> Result<Option<StoppedBody>, DapError> {
        let request = self.request;
        // An adapter that never answers goes quiet for the whole launch timeout; see
        // [`PTRACE_HINT`].
        if let Err(error) = self.client.await_response(pending, request, LAUNCH_TIMEOUT).await {
            let adapter = self.adapter;
            let mut message = match error {
                DapError::Cancelled => {
                    format!("the {adapter} adapter exited before answering `{request}`")
                }
                DapError::RequestFailed { message, .. } => {
                    format!("the {adapter} adapter refused `{request}`: {message}")
                }
                _ => format!(
                    "the {adapter} adapter did not answer `{request}` within {}s",
                    LAUNCH_TIMEOUT.as_secs()
                ),
            };
            self.hint(&mut message);
            return Err(self.with_stderr(message).await);
        }
        // The adapter accepted the launch or attach and then reported nothing. `gdb
        // --interpreter=dap` does exactly that when asked to attach to a process it may not
        // ptrace: it answers the request, and no stop ever comes.
        match self.client.wait_for_stop_or_terminate(stop_timeout()).await {
            Ok(stop) => Ok(stop),
            Err(DapError::Timeout { .. }) => {
                let mut message = format!(
                    "the {} adapter accepted `{request}` but the program never stopped or \
                     exited ({}s)",
                    self.adapter,
                    stop_timeout().as_secs()
                );
                self.hint(&mut message);
                Err(DapError::StartFailed(message))
            }
            Err(error) => Err(error),
        }
    }

    /// The adapter went away after `initialize`, said with whatever it printed on the way out.
    async fn exited_starting(&self) -> DapError {
        let mut message = format!("the {} adapter exited while starting", self.adapter);
        self.hint(&mut message);
        self.with_stderr(message).await
    }

    fn hint(&self, message: &mut String) {
        if self.request == "attach" {
            message.push_str(PTRACE_HINT);
        }
    }

    /// `message`, with what the adapter wrote to stderr when it wrote anything.
    async fn with_stderr(&self, message: String) -> DapError {
        DapError::StartFailed(match self.client.adapter_stderr().await {
            stderr if stderr.is_empty() => message,
            stderr => format!("{message}; the adapter said:\n{stderr}"),
        })
    }
}
