//! Incremental SSE decode — turn a live HTTP body into `ProviderEvent`s as bytes arrive.
//!
//! Every adapter here speaks server-sent events, and reading the *whole* response before parsing it
//! would make streaming a lie: reasoning and answer text would reach the UI together, at the end
//! of the call, and a cancel could not interrupt a provider call because nothing was being read.
//!
//! [`decode`] gives all three adapters that shape once. It buffers the body, cuts complete
//! `\n\n`-delimited records off the front, and hands each to an [`SseDecoder`] whose events are
//! yielded immediately. Cross-record state (Anthropic's split usage, the OpenAI-compat
//! `Usage`-before-`Done` ordering) lives in the decoder, which is why this is a trait and not a
//! function pointer.

use futures::stream::{BoxStream, Stream, StreamExt};
use luxe_core::provider::ProviderEvent;
use std::collections::VecDeque;
use tokio::sync::watch;

/// Per-provider translation of SSE records into `ProviderEvent`s.
///
/// Implementors are fed one complete record at a time, in order, and may keep whatever state the
/// wire format needs between records.
pub(crate) trait SseDecoder {
    /// Decodes one SSE record (event/data lines, no trailing blank line), appending its events.
    fn record(&mut self, record: &str, out: &mut Vec<ProviderEvent>);

    /// Called once after the last record, for anything the decoder was holding back.
    fn finish(&mut self, _out: &mut Vec<ProviderEvent>) {}
}

/// Streams `body` through `decoder`, yielding each `ProviderEvent` as soon as its record arrives.
///
/// A flipped `cancel` watch drops the body mid-flight, which is what actually aborts an in-flight
/// provider call; a transport error surfaces as a terminal `ProviderEvent::Error`.
pub(crate) fn decode<S, B, D>(
    body: S,
    cancel: watch::Receiver<bool>,
    decoder: D,
) -> BoxStream<'static, ProviderEvent>
where
    S: Stream<Item = Result<B, reqwest::Error>> + Send + 'static,
    B: AsRef<[u8]> + Send + 'static,
    D: SseDecoder + Send + 'static,
{
    let state = DecodeState {
        body: Box::pin(body),
        buffer: String::new(),
        pending: VecDeque::new(),
        decoder,
        cancel,
        drained: false,
    };
    futures::stream::unfold(state, |mut state| async move {
        loop {
            if let Some(event) = state.pending.pop_front() {
                return Some((event, state));
            }
            if state.drained {
                return None;
            }
            state.pump().await;
        }
    })
    .boxed()
}

struct DecodeState<S, D> {
    body: std::pin::Pin<Box<S>>,
    /// Bytes received but not yet forming a complete record.
    buffer: String,
    pending: VecDeque<ProviderEvent>,
    decoder: D,
    cancel: watch::Receiver<bool>,
    drained: bool,
}

impl<S, B, D> DecodeState<S, D>
where
    S: Stream<Item = Result<B, reqwest::Error>> + Send,
    B: AsRef<[u8]>,
    D: SseDecoder,
{
    /// Reads one chunk (or notices the end/cancel) and queues whatever it decoded.
    async fn pump(&mut self) {
        if *self.cancel.borrow_and_update() {
            return self.finish();
        }
        let quiet = async {
            match crate::http::idle_timeout() {
                Some(limit) => tokio::time::sleep(limit).await,
                None => std::future::pending().await,
            }
        };
        let chunk = tokio::select! {
            chunk = self.body.next() => chunk,
            // Cancelling has to interrupt the *read*, not just be noticed between chunks: a
            // provider that has gone quiet mid-turn would otherwise hold the turn open.
            _ = self.cancel.changed() => None,
            () = quiet => {
                let limit = crate::http::idle_timeout().map_or(0, |limit| limit.as_secs());
                self.pending.push_back(ProviderEvent::Error {
                    message: format!("stream error: timed out after {limit}s with nothing from the provider"),
                    kind: luxe_core::provider::ProviderErrorKind::Transport,
                });
                self.drained = true;
                return;
            }
        };
        match chunk {
            Some(Ok(bytes)) => {
                // Lossy is right here: a split multi-byte character can only be repaired by the
                // next chunk, and `\n\n` boundaries are ASCII, so records still cut correctly.
                self.buffer.push_str(&String::from_utf8_lossy(bytes.as_ref()));
                let mut events = Vec::new();
                for record in take_records(&mut self.buffer) {
                    self.decoder.record(&record, &mut events);
                }
                self.pending.extend(events);
            }
            Some(Err(error)) => {
                self.pending.push_back(ProviderEvent::Error {
                    message: format!("stream error: {error}"),
                    kind: luxe_core::provider::ProviderErrorKind::Transport,
                });
                self.drained = true;
            }
            None => self.finish(),
        }
    }

    /// Flushes a trailing partial record and lets the decoder emit anything it held back.
    fn finish(&mut self) {
        let mut events = Vec::new();
        let tail = std::mem::take(&mut self.buffer);
        if !tail.trim().is_empty() {
            self.decoder.record(&tail, &mut events);
        }
        self.decoder.finish(&mut events);
        self.pending.extend(events);
        self.drained = true;
    }
}

/// Splits every complete record off the front of `buffer`, leaving the partial tail behind.
///
/// SSE separates records with a blank line. `\r\n\r\n` is accepted too — some proxies rewrite line
/// endings — and the `\r`s are trimmed by the per-line parsers downstream.
fn take_records(buffer: &mut String) -> Vec<String> {
    let mut records = Vec::new();
    loop {
        let boundary = buffer
            .find("\n\n")
            .map(|at| (at, 2))
            .or_else(|| buffer.find("\r\n\r\n").map(|at| (at, 4)));
        let Some((at, width)) = boundary else { return records };
        let record: String = buffer.drain(..at + width).collect();
        records.push(record[..at].to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records every record it is handed, and emits one text delta per record.
    #[derive(Default)]
    struct EchoDecoder {
        seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        finished: bool,
    }

    impl SseDecoder for EchoDecoder {
        fn record(&mut self, record: &str, out: &mut Vec<ProviderEvent>) {
            self.seen.lock().expect("lock").push(record.to_string());
            out.push(ProviderEvent::TextDelta { index: 0, delta: record.to_string() });
        }

        fn finish(&mut self, out: &mut Vec<ProviderEvent>) {
            self.finished = true;
            out.push(ProviderEvent::Done { reason: luxe_core::provider::StopReason::EndTurn });
        }
    }

    /// A body that sends one record and then nothing, ever: the stream ends with a timeout a retry
    /// can recognize, rather than holding the turn open.
    #[tokio::test(start_paused = true)]
    async fn a_stream_that_goes_quiet_times_out() {
        let first = futures::stream::iter([Ok::<_, reqwest::Error>(b"data: a\n\n".to_vec())]);
        let body = first.chain(futures::stream::pending());
        let (_cancel_tx, cancel) = watch::channel(false);
        let started = tokio::time::Instant::now();
        let events: Vec<ProviderEvent> =
            decode(body, cancel, EchoDecoder::default()).collect().await;
        assert!(matches!(&events[0], ProviderEvent::TextDelta { delta, .. } if delta == "data: a"));
        let ProviderEvent::Error { message, kind } = &events[1] else { panic!("{events:?}") };
        assert_eq!(*kind, luxe_core::provider::ProviderErrorKind::Transport);
        assert!(message.contains("timed out after 180s"), "{message}");
        assert_eq!(events.len(), 2, "and that is the end of it");
        assert_eq!(started.elapsed(), crate::http::IDLE_TIMEOUT);
    }

    fn chunks(
        parts: &[&str],
    ) -> impl Stream<Item = Result<Vec<u8>, reqwest::Error>> + Send + 'static {
        let owned: Vec<Vec<u8>> = parts.iter().map(|part| part.as_bytes().to_vec()).collect();
        futures::stream::iter(owned.into_iter().map(Ok))
    }

    #[test]
    fn records_are_cut_on_blank_lines_and_the_tail_is_kept() {
        let mut buffer = String::from("event: a\ndata: 1\n\nevent: b\ndata: 2\n\nevent: c\ndata:");
        let records = take_records(&mut buffer);
        assert_eq!(records, vec!["event: a\ndata: 1", "event: b\ndata: 2"]);
        assert_eq!(buffer, "event: c\ndata:");
    }

    #[test]
    fn crlf_record_separators_are_accepted() {
        let mut buffer = String::from("event: a\r\ndata: 1\r\n\r\nrest");
        assert_eq!(take_records(&mut buffer), vec!["event: a\r\ndata: 1"]);
        assert_eq!(buffer, "rest");
    }

    /// The point of the whole module: a record that arrives split across chunks is decoded as soon
    /// as its final byte lands, not when the body ends.
    #[tokio::test]
    async fn a_record_split_across_chunks_decodes_once_completed() {
        let (_tx, cancel) = watch::channel(false);
        let decoder = EchoDecoder::default();
        let seen = decoder.seen.clone();
        let mut stream =
            decode(chunks(&["data: he", "llo\n\ndata: wor", "ld\n\n"]), cancel, decoder);

        let first = stream.next().await;
        assert!(
            matches!(first, Some(ProviderEvent::TextDelta { ref delta, .. }) if delta == "data: hello")
        );
        assert_eq!(seen.lock().expect("lock").len(), 1, "only the completed record was decoded");

        let second = stream.next().await;
        assert!(
            matches!(second, Some(ProviderEvent::TextDelta { ref delta, .. }) if delta == "data: world")
        );
    }

    #[tokio::test]
    async fn a_trailing_partial_record_is_flushed_at_the_end() {
        let (_tx, cancel) = watch::channel(false);
        let mut stream =
            decode(chunks(&["data: one\n\ndata: two"]), cancel, EchoDecoder::default());
        let mut deltas = Vec::new();
        while let Some(event) = stream.next().await {
            if let ProviderEvent::TextDelta { delta, .. } = event {
                deltas.push(delta);
            }
        }
        assert_eq!(deltas, vec!["data: one", "data: two"]);
    }

    #[tokio::test]
    async fn a_cancelled_stream_stops_reading() {
        let (tx, cancel) = watch::channel(false);
        tx.send(true).expect("cancel");
        let mut stream = decode(chunks(&["data: one\n\n"]), cancel, EchoDecoder::default());
        // Only the decoder's finish output survives — no records were read.
        let events: Vec<_> = stream.by_ref().collect().await;
        assert_eq!(events.len(), 1, "cancelled before the first record: {events:?}");
        assert!(matches!(events[0], ProviderEvent::Done { .. }));
    }
}
