//! Background reader task — frames server stdout and dispatches messages.
//!
//! The reader owns the live server→client stream: it routes responses to their pending request,
//! auto-replies to server→client requests (so `$/progress`, capability registration, and config
//! requests don't stall the server), and folds `publishDiagnostics` + `$/progress` into the shared
//! [`SharedState`] so a warm client can report settled diagnostics without a one-shot wait.

use super::*;
use luxe_core::jsonrpc_frame::{read_frame, write_frame, MAX_FRAME_BYTES};

/// Background reader: continuously read messages from stdout and dispatch them.
pub(crate) async fn read_loop<R: AsyncReadExt + AsyncBufRead + Unpin>(
    reader: &mut R,
    shared: Arc<SharedState>,
    stdin: Arc<Mutex<ChildStdin>>,
    tx: mpsc::Sender<JsonRpcMessage>,
    pending_requests: PendingRequests,
) -> Result<(), LspError> {
    let outcome = route_messages(reader, shared, stdin, tx, &pending_requests).await;
    // The reader is the only thing that can ever answer an in-flight request, so when it stops —
    // EOF because the server exited, or a transport error — nothing is coming for any of them.
    // Dropping the senders turns each waiter's `rx.await` into `RequestCancelled`; left alive, a
    // server that died mid-session would leave its turn waiting forever instead of failing
    pending_requests.lock().await.clear();
    outcome
}

/// The reader's loop proper, split out so [`read_loop`] can clean up on every exit path.
async fn route_messages<R: AsyncReadExt + AsyncBufRead + Unpin>(
    reader: &mut R,
    shared: Arc<SharedState>,
    stdin: Arc<Mutex<ChildStdin>>,
    tx: mpsc::Sender<JsonRpcMessage>,
    pending_requests: &PendingRequests,
) -> Result<(), LspError> {
    loop {
        let Some(bytes) = read_frame(reader, MAX_FRAME_BYTES).await? else {
            break; // EOF — server exited.
        };
        let msg: JsonRpcMessage = match serde_json::from_slice(&bytes) {
            Ok(msg) => msg,
            Err(e) => {
                debug!(error = %e, "lsp: dropping unparseable server message");
                continue;
            }
        };

        match (&msg.id, &msg.method) {
            // Response to one of our requests: id present, no method.
            (Some(id), None) => {
                if let Some(id) = id.as_u64() {
                    if let Some(sender) = pending_requests.lock().await.remove(&id) {
                        // A JSON-RPC failure carries `error` and no `result`. Reading only
                        // `result` turned every server error into `Null`, which each caller then
                        // rendered as an empty answer: "No definition found" for a server that had
                        // in fact said "Could not find a valid TypeScript installation".
                        let _ = sender.send(match &msg.error {
                            Some(error) => Err(error.clone()),
                            None => Ok(msg.result.clone().unwrap_or(Value::Null)),
                        });
                    }
                }
            }
            // Server→client request: id and method both present. Reply so it never blocks.
            (Some(id), Some(method)) => {
                reply_to_server_request(&stdin, id.clone(), method, msg.params.as_ref()).await;
            }
            // Notification: method, no id.
            (None, Some(method)) => {
                apply_notification(&shared, method, msg.params.as_ref());
                // Best-effort tap for `wait_for_notification`; never block the reader on a full tap.
                let _ = tx.try_send(msg);
            }
            _ => {}
        }
    }
    Ok(())
}

/// Folds a server notification into shared state: diagnostics cache + progress tracking.
fn apply_notification(shared: &SharedState, method: &str, params: Option<&Value>) {
    match method {
        "textDocument/publishDiagnostics" => {
            let Some(params) = params else { return };
            let Some(uri) = params.get("uri").and_then(Value::as_str) else { return };
            let diagnostics = params
                .get("diagnostics")
                .and_then(Value::as_array)
                .map(|raw| {
                    raw.iter()
                        .filter_map(|d| match serde_json::from_value::<Diagnostic>(d.clone()) {
                            Ok(diag) => Some(diag),
                            Err(e) => {
                                debug!(uri, error = %e, "lsp: dropping unparseable diagnostic");
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            {
                let mut inner = shared.inner.lock().expect("diagnostics state poisoned");
                inner.diagnostics.insert(uri.to_string(), diagnostics);
                inner.publish_count += 1;
                inner.mark_change();
            }
            shared.notify.notify_waiters();
        }
        "$/progress" => {
            let kind = params
                .and_then(|p| p.get("value"))
                .and_then(|v| v.get("kind"))
                .and_then(Value::as_str);
            let mut inner = shared.inner.lock().expect("diagnostics state poisoned");
            match kind {
                Some("begin") => inner.progress_active += 1,
                Some("end") => {
                    inner.progress_active = inner.progress_active.saturating_sub(1);
                }
                _ => {}
            }
            inner.mark_change();
            drop(inner);
            shared.notify.notify_waiters();
        }
        // Server-surfaced messages: keep Error/Warning ones so an empty diagnostics result from a
        // misconfigured/incompatible server (e.g. tsserver that can't find a usable TypeScript)
        // isn't silent. Info/log chatter (type >= 3) is ignored.
        "window/showMessage" | "window/logMessage" => {
            let Some(params) = params else { return };
            let severity = params.get("type").and_then(Value::as_u64).unwrap_or(0);
            if severity == 1 || severity == 2 {
                if let Some(message) = params.get("message").and_then(Value::as_str) {
                    if let Ok(mut inner) = shared.inner.lock() {
                        inner.push_notice(severity as u8, message.to_string());
                    }
                }
            }
        }
        // rust-analyzer's readiness signal: `quiescent: true` once indexing/priming settles.
        "experimental/serverStatus" => {
            let quiescent =
                params.and_then(|p| p.get("quiescent")).and_then(Value::as_bool).unwrap_or(false);
            {
                let mut inner = shared.inner.lock().expect("diagnostics state poisoned");
                inner.quiescent = quiescent;
                inner.mark_change();
            }
            shared.notify.notify_waiters();
        }
        _ => {}
    }
}

/// Auto-replies to a server→client request so the server never stalls waiting on us.
///
/// We don't implement these capabilities, so the replies are deliberately empty: `null` for
/// capability registration / progress-create / message requests, and an array of `null`s for
/// `workspace/configuration` (the server then falls back to defaults).
async fn reply_to_server_request(
    stdin: &Arc<Mutex<ChildStdin>>,
    id: Value,
    method: &str,
    params: Option<&Value>,
) {
    let result = if method == "workspace/configuration" {
        let items = params
            .and_then(|p| p.get("items"))
            .and_then(Value::as_array)
            .map_or(1, |items| items.len());
        Value::Array(vec![Value::Null; items.max(1)])
    } else {
        Value::Null
    };

    let response = JsonRpcMessage {
        jsonrpc: "2.0".to_string(),
        id: Some(id),
        method: None,
        params: None,
        result: Some(result),
        error: None,
    };
    let Ok(json) = serde_json::to_string(&response) else { return };
    let mut guard = stdin.lock().await;
    if let Err(e) = write_frame(&mut *guard, json.as_bytes()).await {
        debug!(method, error = %e, "lsp: failed to reply to server request");
    }
}
