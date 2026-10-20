//! Wire log — an opt-in, on-disk record of everything luxe sends to a model and gets back.
//!
//! Set `LUXE_WIRE_LOG=<path>` and every provider call appends JSONL: the exact request body as it
//! goes on the wire (after provider-specific shaping — `cache_control` markers, tool schemas,
//! thinking config), the request headers with credential values redacted, and every normalized
//! [`ProviderEvent`] coming back. `luxe wire <path>` turns that into an audit report.
//!
//! Off unless the variable is set, and never fatal: a log that cannot be written is dropped with a
//! warning rather than failing a turn. The file is local — nothing here phones home.
//!
//! Requests are recorded by the adapters, the only place the final body exists; responses by
//! [`RecordingProvider`], which wraps any adapter and needs no per-provider knowledge. The two
//! halves are tied together by a call id the decorator puts in a task-local, so concurrent
//! sessions never interleave into one another's records.

use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use luxe_core::provider::{CompletionRequest, Provider, ProviderError, ProviderEvent};

/// Header names whose value is a credential and must never reach the log.
const SECRET_HEADERS: &[&str] =
    &["authorization", "x-api-key", "api-key", "cookie", "proxy-authorization"];

/// The open log file, resolved once from the environment.
static SINK: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();

/// Monotonic provider-call counter.
static SEQ: AtomicU64 = AtomicU64::new(0);

tokio::task_local! {
    /// The call id of the provider call this task is inside, set by [`RecordingProvider`].
    static CALL_ID: u64;
}

fn sink() -> Option<&'static Mutex<std::fs::File>> {
    SINK.get_or_init(|| {
        let path = std::env::var("LUXE_WIRE_LOG").ok().filter(|path| !path.trim().is_empty())?;
        let path = std::path::PathBuf::from(path);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => {
                tracing::info!(path = %path.display(), "wire log enabled");
                Some(Mutex::new(file))
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "wire log could not be opened");
                None
            }
        }
    })
    .as_ref()
}

/// Whether wire logging is on. Cheap enough to guard every call site.
pub fn enabled() -> bool {
    sink().is_some()
}

/// The call this task belongs to, or `0` for a provider used without the recording wrapper.
pub fn current_call_id() -> u64 {
    CALL_ID.try_with(|id| *id).unwrap_or(0)
}

fn append(record: serde_json::Value) {
    if let Some(sink) = sink() {
        append_to(sink, &record);
    }
}

/// Writes `record` to `sink`, saying once if it could not.
fn append_to(sink: &Mutex<impl Write>, record: &serde_json::Value) {
    let written = match sink.lock() {
        Ok(mut out) => write_record(&mut *out, record),
        Err(_) => Err(std::io::Error::other("the log's lock was poisoned")),
    };
    if let Err(error) = written {
        note_dropped(&error);
    }
}

/// One record as one line.
fn write_record(out: &mut impl Write, record: &serde_json::Value) -> std::io::Result<()> {
    let line = serde_json::to_string(record).map_err(std::io::Error::other)?;
    writeln!(out, "{line}")
}

/// Whether the log has already said it is dropping records.
static DROPPING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Says, once, that the log is dropping records, so a cut-short capture does not pass for a
/// quiet run. Returns whether this call was the one that said it.
fn note_dropped(error: &dyn std::fmt::Display) -> bool {
    let first = !DROPPING.swap(true, Ordering::Relaxed);
    if first {
        tracing::warn!(%error, "the wire log could not be written; later records may be missing");
    }
    first
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or_default()
}

/// Record one outgoing request exactly as it will be sent.
///
/// `headers` carries only what the adapter sets explicitly; credential header values become
/// `<redacted>`, and the body goes through the shared secret redactor in case a key reached the
/// prompt by some other route.
pub fn record_request(
    provider: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: &serde_json::Value,
) {
    if !enabled() {
        return;
    }
    let headers: serde_json::Map<String, serde_json::Value> = headers
        .iter()
        .map(|(name, value)| {
            let secret = SECRET_HEADERS.contains(&name.to_ascii_lowercase().as_str());
            let value = if secret { "<redacted>" } else { *value };
            ((*name).to_string(), serde_json::Value::String(value.to_string()))
        })
        .collect();
    let body = match serde_json::to_string(body) {
        Ok(text) => serde_json::from_str(&luxe_core::redact::process_secrets().redact(&text))
            .unwrap_or_else(|_| body.clone()),
        Err(_) => body.clone(),
    };
    append(serde_json::json!({
        "kind": "request",
        "call": current_call_id(),
        "ts_ms": now_ms(),
        "provider": provider,
        "url": url,
        "headers": headers,
        "body": body,
    }));
}

/// Longest reply text kept verbatim; beyond this the record keeps the count and an excerpt.
const MAX_TEXT: usize = 4_000;

/// Longest tool-call argument payload kept verbatim.
const MAX_ARGS: usize = 2_000;

/// Most arrival samples kept, so a pathological stream can't unbound the record.
const MAX_ARRIVALS: usize = 2_000;

/// Accumulates one call's response into a single record.
///
/// One line per event would be one line per *token* — unreadable, and it would bury the structure
/// an audit is actually looking for. Instead the stream is folded into what came back (text, tool
/// calls, usage, stop reason) plus an arrival timeline of `[ms_offset, chars]` pairs, which is what
/// proves "renders as it arrives" without duplicating the text.
#[derive(Default)]
struct ResponseRecord {
    counts: std::collections::BTreeMap<&'static str, u64>,
    text: String,
    thinking_chars: usize,
    tool_calls: Vec<(String, String, String)>,
    usage: Option<serde_json::Value>,
    stop: Option<String>,
    errors: Vec<String>,
    arrival: Vec<(u128, usize)>,
    first_delta_ms: Option<u128>,
}

impl ResponseRecord {
    fn observe(&mut self, event: &ProviderEvent, elapsed_ms: u128) {
        let kind = match event {
            ProviderEvent::Start => "start",
            ProviderEvent::TextStart { .. } => "text_start",
            ProviderEvent::TextDelta { delta, .. } => {
                if self.first_delta_ms.is_none() {
                    self.first_delta_ms = Some(elapsed_ms);
                }
                if self.text.len() < MAX_TEXT {
                    self.text.push_str(delta);
                }
                if self.arrival.len() < MAX_ARRIVALS {
                    self.arrival.push((elapsed_ms, delta.chars().count()));
                }
                "text_delta"
            }
            ProviderEvent::TextEnd { .. } => "text_end",
            ProviderEvent::ThinkingStart { .. } => "thinking_start",
            ProviderEvent::ThinkingDelta { delta, .. } => {
                if self.first_delta_ms.is_none() {
                    self.first_delta_ms = Some(elapsed_ms);
                }
                self.thinking_chars += delta.chars().count();
                if self.arrival.len() < MAX_ARRIVALS {
                    self.arrival.push((elapsed_ms, delta.chars().count()));
                }
                "thinking_delta"
            }
            ProviderEvent::ThinkingSignature { .. } => "thinking_signature",
            ProviderEvent::ReasoningItem { .. } => "reasoning_item",
            ProviderEvent::ReasoningOpaque { .. } => "reasoning_opaque",
            ProviderEvent::RedactedThinking { .. } => "redacted_thinking",
            ProviderEvent::ThinkingEnd { .. } => "thinking_end",
            ProviderEvent::ToolCallStart { id, name, .. } => {
                self.tool_calls.push((id.clone(), name.clone(), String::new()));
                "tool_call_start"
            }
            ProviderEvent::ToolCallDelta { json_delta, .. } => {
                if let Some(call) = self.tool_calls.last_mut() {
                    if call.2.len() < MAX_ARGS {
                        call.2.push_str(json_delta);
                    }
                }
                "tool_call_delta"
            }
            ProviderEvent::ToolCallEnd { .. } => "tool_call_end",
            ProviderEvent::Usage(usage) => {
                self.usage = Some(serde_json::json!({
                    "prompt_tokens": usage.prompt_tokens,
                    "completion_tokens": usage.completion_tokens,
                    "total_tokens": usage.total_tokens,
                    "cache_read_tokens": usage.cache_read_tokens,
                    "cache_write_tokens": usage.cache_write_tokens,
                    "reasoning_tokens": usage.reasoning_tokens,
                }));
                "usage"
            }
            ProviderEvent::PlanUsage(_) => "plan_usage",
            ProviderEvent::Done { reason } => {
                self.stop = Some(format!("{reason:?}"));
                "done"
            }
            ProviderEvent::Error { message, kind } => {
                self.errors.push(format!("{kind:?}: {message}"));
                "error"
            }
        };
        *self.counts.entry(kind).or_default() += 1;
    }

    fn finish(self, call: u64, provider: &str, elapsed_ms: u128, completed: bool) {
        let text = luxe_core::redact::process_secrets().redact(&self.text);
        append(serde_json::json!({
            "kind": "response",
            "call": call,
            "ts_ms": now_ms(),
            "provider": provider,
            "elapsed_ms": elapsed_ms,
            "first_delta_ms": self.first_delta_ms,
            "drained": completed,
            "events": self.counts,
            "text": text,
            "thinking_chars": self.thinking_chars,
            "tool_calls": self.tool_calls.iter().map(|(id, name, args)| serde_json::json!({
                "id": id,
                "name": name,
                "arguments": luxe_core::redact::process_secrets().redact(args),
            })).collect::<Vec<_>>(),
            "usage": self.usage,
            "stop": self.stop,
            "errors": self.errors,
            "arrival": self.arrival,
        }));
    }
}

fn record_error(call: u64, provider: &str, error: &ProviderError) {
    append(serde_json::json!({
        "kind": "error",
        "call": call,
        "ts_ms": now_ms(),
        "provider": provider,
        "error": luxe_core::redact::process_secrets().redact(&error.to_string()),
    }));
}

/// Wraps any provider and records the event stream it produces.
///
/// Provider-agnostic on purpose: it sees normalized [`ProviderEvent`]s, so a new adapter is logged
/// the day it exists. The request half lives in the adapters, which are the only place the final
/// wire body is known.
pub struct RecordingProvider {
    inner: Arc<dyn Provider + Send + Sync>,
}

impl RecordingProvider {
    /// Wrap `inner`, or hand it back untouched when logging is off.
    pub fn wrap(inner: Arc<dyn Provider + Send + Sync>) -> Arc<dyn Provider + Send + Sync> {
        if enabled() {
            Arc::new(Self { inner })
        } else {
            inner
        }
    }
}

#[async_trait::async_trait]
impl Provider for RecordingProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn stream(
        &self,
        req: CompletionRequest,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<futures::stream::BoxStream<'static, ProviderEvent>, ProviderError> {
        let call = SEQ.fetch_add(1, Ordering::Relaxed);
        let provider = self.inner.name().to_string();
        let started = std::time::Instant::now();

        // The adapter records its request from inside this scope, so both halves share `call`.
        let stream = match CALL_ID.scope(call, self.inner.stream(req, cancel)).await {
            Ok(stream) => stream,
            Err(error) => {
                record_error(call, &provider, &error);
                return Err(error);
            }
        };

        Ok(Box::pin(Recording {
            inner: stream,
            open: Some(OpenRecord { call, provider, started, record: ResponseRecord::default() }),
        }))
    }

    // Forwarded, or wrapping a provider for the log would quietly switch its cache warming off
    fn prompt_cache_ttl(&self, req: &CompletionRequest) -> Option<std::time::Duration> {
        self.inner.prompt_cache_ttl(req)
    }

    async fn warm_cache(
        &self,
        req: CompletionRequest,
    ) -> Result<luxe_core::provider::Usage, ProviderError> {
        let call = SEQ.fetch_add(1, Ordering::Relaxed);
        let started = std::time::Instant::now();
        let warmed = CALL_ID.scope(call, self.inner.warm_cache(req)).await;
        match &warmed {
            // A response half, so `luxe wire` shows what the refresh read rather than a row of zeros
            Ok(usage) => {
                let mut record = ResponseRecord::default();
                record.observe(&ProviderEvent::Usage(usage.clone()), started.elapsed().as_millis());
                record.finish(call, self.inner.name(), started.elapsed().as_millis(), true);
            }
            Err(error) => record_error(call, self.inner.name(), error),
        }
        warmed
    }
}

/// The in-flight half of a recorded stream.
struct OpenRecord {
    call: u64,
    provider: String,
    started: std::time::Instant,
    record: ResponseRecord,
}

/// A stream that folds what passes through it into one response record.
///
/// The record is flushed on **drop**, not just on end-of-stream: the engine stops polling the
/// moment it sees `Done` and drops the rest, so an end-of-stream hook alone would silently log
/// nothing for a normal turn. `drained` in the record says which of the two happened.
struct Recording<S> {
    inner: S,
    open: Option<OpenRecord>,
}

impl<S> Recording<S> {
    fn flush(&mut self, drained: bool) {
        if let Some(open) = self.open.take() {
            let elapsed = open.started.elapsed().as_millis();
            open.record.finish(open.call, &open.provider, elapsed, drained);
        }
    }
}

impl<S> futures::Stream for Recording<S>
where
    S: futures::Stream<Item = ProviderEvent> + Unpin,
{
    type Item = ProviderEvent;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match std::pin::Pin::new(&mut this.inner).poll_next(cx) {
            std::task::Poll::Ready(Some(event)) => {
                if let Some(open) = this.open.as_mut() {
                    open.record.observe(&event, open.started.elapsed().as_millis());
                }
                std::task::Poll::Ready(Some(event))
            }
            std::task::Poll::Ready(None) => {
                this.flush(true);
                std::task::Poll::Ready(None)
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

impl<S> Drop for Recording<S> {
    fn drop(&mut self) {
        self.flush(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_headers_are_redacted_and_protocol_headers_are_not() {
        for name in ["Authorization", "x-api-key", "API-KEY", "Cookie"] {
            assert!(
                SECRET_HEADERS.contains(&name.to_ascii_lowercase().as_str()),
                "{name} must be treated as a credential"
            );
        }
        for name in ["anthropic-version", "Content-Type", "Accept", "anthropic-beta"] {
            assert!(
                !SECRET_HEADERS.contains(&name.to_ascii_lowercase().as_str()),
                "{name} is protocol, not a credential"
            );
        }
    }

    /// A record the log cannot take is said, once, rather than dropped without a word.
    ///
    /// The break this catches: every failure was swallowed, so a capture that stopped partway read
    /// as a session that went quiet.
    #[test]
    fn a_record_the_log_cannot_take_is_said_once() {
        struct Full;
        impl Write for Full {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("disk full"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        append_to(&Mutex::new(Full), &serde_json::json!({"kind": "request"}));
        let error = std::io::Error::other("again");
        assert!(!note_dropped(&error), "the append said it, so this one does not");
    }

    #[tokio::test]
    async fn a_call_id_is_zero_outside_a_recorded_call_and_set_inside_one() {
        assert_eq!(current_call_id(), 0, "an unwrapped adapter still records, under call 0");
        CALL_ID.scope(7, async { assert_eq!(current_call_id(), 7) }).await;
    }
}
