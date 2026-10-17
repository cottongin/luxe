//! DAP reader — the background task that reads the adapter, and the folding of what it says.
//!
//! Responses go to their pending request, events fold into [`Shared`] state, and reverse requests
//! are refused at once so the adapter never waits on us.

use super::{BoxWriter, DapError, DapState, Pending, Shared, MAX_OUTPUT_BYTES};
use crate::protocol::{ProtocolMessage, StoppedBody};
use luxe_core::jsonrpc_frame::{read_frame, write_frame, MAX_FRAME_BYTES};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex as SyncMutex};
use tokio::io::{AsyncBufRead, AsyncReadExt};
use tokio::process::Child;
use tokio::sync::Mutex;
use tracing::debug;

/// The start of what an adapter wrote to stderr, filled as it arrives.
#[derive(Default)]
pub(super) struct StderrHead {
    /// What has arrived, up to [`STDERR_HEAD_BYTES`]
    pub(super) text: SyncMutex<String>,
    /// Whether stderr has closed, so nothing more is coming
    pub(super) closed: std::sync::atomic::AtomicBool,
}

/// Background reader: routes responses to pending requests, folds events, replies to reverse requests.
pub(super) async fn read_loop<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    shared: Arc<Shared>,
    writer: Arc<Mutex<BoxWriter>>,
    pending: Pending,
) -> Result<(), DapError> {
    let outcome = route_messages(reader, &shared, &writer, &pending).await;
    // Only the reader can answer an in-flight request, so once it stops — the adapter exited, or
    // its stream broke — nothing is coming for any of them. Dropping the senders fails each waiter
    // as `Cancelled` now instead of at its own timeout, and the session reads as over either way.
    // Marked first, so a request written from here on sees it and does not wait either.
    shared.update(|s| {
        s.disconnected = true;
        s.terminated = true;
    });
    pending.lock().await.clear();
    outcome
}

/// The reader's loop proper, split out so [`read_loop`] can clean up on every exit path.
async fn route_messages<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    shared: &Shared,
    writer: &Arc<Mutex<BoxWriter>>,
    pending: &Pending,
) -> Result<(), DapError> {
    while let Some(bytes) =
        read_frame(reader, MAX_FRAME_BYTES).await.map_err(|e| DapError::Io(e.to_string()))?
    {
        let msg: ProtocolMessage = match serde_json::from_slice(&bytes) {
            Ok(msg) => msg,
            Err(e) => {
                debug!(error = %e, "dap: dropping unparseable message");
                continue;
            }
        };
        match msg.kind.as_str() {
            "response" => {
                if let Some(request_seq) = msg.request_seq {
                    if let Some(tx) = pending.lock().await.remove(&request_seq) {
                        let _ = tx.send(msg);
                    }
                }
            }
            "event" => apply_event(shared, &msg),
            "request" => reply_unsupported(writer, &msg).await,
            _ => {}
        }
    }
    Ok(())
}

/// Render captured output with its recorded stream boundaries broken onto their own lines.
pub(super) fn display_output(state: &DapState) -> String {
    if state.output_breaks.is_empty() {
        return state.output.clone();
    }
    let mut out = String::with_capacity(state.output.len() + state.output_breaks.len());
    let mut cursor = 0usize;
    for offset in &state.output_breaks {
        let offset = (*offset).min(state.output.len());
        if offset < cursor || !state.output.is_char_boundary(offset) {
            continue;
        }
        out.push_str(&state.output[cursor..offset]);
        out.push('\n');
        cursor = offset;
    }
    out.push_str(&state.output[cursor..]);
    out
}

/// Fold a DAP event into shared session state.
pub(super) fn apply_event(shared: &Shared, msg: &ProtocolMessage) {
    let Some(event) = msg.event.as_deref() else { return };
    match event {
        "initialized" => shared.update(|s| s.initialized = true),
        "stopped" => {
            let body = msg
                .body
                .clone()
                .and_then(|b| serde_json::from_value::<StoppedBody>(b).ok())
                .unwrap_or_default();
            shared.update(|s| s.stopped = Some(body));
        }
        "continued" => shared.update(|s| s.stopped = None),
        "terminated" | "exited" => shared.update(|s| s.terminated = true),
        "output" => {
            let chunk = msg
                .body
                .as_ref()
                .and_then(|b| b.get("output"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let category = msg
                .body
                .as_ref()
                .and_then(|b| b.get("category"))
                .and_then(Value::as_str)
                .unwrap_or("console")
                .to_string();
            if !chunk.is_empty() {
                let chunk = chunk.to_string();
                shared.update(move |s| {
                    // A different stream cutting in mid-line is a display boundary; the bytes
                    // themselves stay exactly as the adapter sent them.
                    let switched =
                        s.last_output_category.as_deref().is_some_and(|last| last != category);
                    if switched && !s.output.ends_with('\n') && !s.output.is_empty() {
                        s.output_breaks.push(s.output.len());
                    }
                    s.last_output_category = Some(category);
                    s.output.push_str(&chunk);
                    if s.output.len() > MAX_OUTPUT_BYTES {
                        let start = luxe_core::output_limit::ceil_boundary(
                            &s.output,
                            s.output.len() - MAX_OUTPUT_BYTES,
                        );
                        s.output = s.output.split_off(start);
                        s.output_breaks.retain(|offset| *offset > start);
                        for offset in &mut s.output_breaks {
                            *offset -= start;
                        }
                    }
                });
            }
        }
        _ => {}
    }
}

/// Reply to an adapter→client (reverse) request with a failure, so the adapter never blocks on us.
async fn reply_unsupported(writer: &Arc<Mutex<BoxWriter>>, msg: &ProtocolMessage) {
    let response = json!({
        "seq": 0,
        "type": "response",
        "request_seq": msg.seq,
        "success": false,
        "command": msg.command.clone().unwrap_or_default(),
        "message": "unsupported reverse request",
    });
    if let Ok(payload) = serde_json::to_vec(&response) {
        let mut guard = writer.lock().await;
        let _ = write_frame(&mut *guard, &payload).await;
    }
}

/// The most of an adapter's stderr kept to explain a failed start.
const STDERR_HEAD_BYTES: usize = 2_000;

/// Drains an adapter's stdout, when it is not the transport, and its stderr to `dap.adapter` logs
/// so its pipes never block, keeping the start of stderr: when an adapter fails to start, that is
/// usually where it said why. Both are read as bytes to the end, so a line that is not UTF-8 does
/// not stop the drain and leave the adapter writing into a closed pipe.
pub(super) fn drain_child_logs(child: &mut Child) -> Arc<StderrHead> {
    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(drain(stdout, None));
    }
    let head = Arc::new(StderrHead::default());
    match child.stderr.take() {
        Some(stderr) => {
            tokio::spawn(drain(stderr, Some(Arc::clone(&head))));
        }
        None => head.closed.store(true, std::sync::atomic::Ordering::Release),
    }
    head
}

/// Reads `pipe` to its end, logging each chunk and keeping the start in `head`, when given one.
async fn drain(mut pipe: impl tokio::io::AsyncRead + Unpin, head: Option<Arc<StderrHead>>) {
    let mut chunk = [0u8; 4096];
    let mut kept = Vec::new();
    while let Ok(read) = pipe.read(&mut chunk).await {
        if read == 0 {
            break;
        }
        for line in String::from_utf8_lossy(&chunk[..read]).lines() {
            debug!(target: "dap.adapter", "{line}");
        }
        if let Some(head) = head.as_ref().filter(|_| kept.len() < STDERR_HEAD_BYTES) {
            kept.extend_from_slice(&chunk[..read.min(STDERR_HEAD_BYTES - kept.len())]);
            if let Ok(mut text) = head.text.lock() {
                *text = String::from_utf8_lossy(&kept).replace("\r\n", "\n");
            }
        }
    }
    if let Some(head) = head {
        head.closed.store(true, std::sync::atomic::Ordering::Release);
    }
}
