//! Frames — one JSON object per line, written without growing one buffer to the biggest frame.
//!
//! A joiner's `session_loaded` carries the whole transcript, tens of megabytes on a long session,
//! and a phone joins again on every reconnect. Encoded into one `String` it grows by doubling,
//! about four times its size allocated for every join, so a frame is encoded into fixed-size pieces
//! instead, and a shared session's transcript goes out from the snapshot's copy, not a clone of it.

use luxe_core::event::SessionMessage;
use luxe_core::Event;
use serde::Serialize;
use std::sync::Arc;
use tokio::io::{AsyncWrite, AsyncWriteExt};

/// Bytes in each piece of an encoded frame.
const CHUNK_BYTES: usize = 64 * 1024;

/// What a client is sent ahead of the event stream: `ready`, then for a joiner the transcript.
pub(in crate::cli) struct Opening {
    pub(in crate::cli) ready: Event,
    /// The transcript so far, for a client joining a session already under way
    pub(in crate::cli) transcript: Option<SharedTranscript>,
}

impl From<Event> for Opening {
    fn from(ready: Event) -> Self {
        Self { ready, transcript: None }
    }
}

/// A session's transcript, sent as its `session_loaded` frame and shared with the snapshot that
/// keeps it, so a join doesn't copy every message.
pub(in crate::cli) struct SharedTranscript {
    pub(in crate::cli) session_id: String,
    pub(in crate::cli) messages: Arc<Vec<SessionMessage>>,
}

/// `Event::SessionLoaded`'s wire shape over borrowed fields; a test pins it to the event's own.
#[derive(Serialize)]
#[serde(tag = "kind", content = "content", rename_all = "snake_case")]
enum LoadedFrame<'a> {
    SessionLoaded { session_id: &'a str, messages: &'a [SessionMessage] },
}

impl Serialize for SharedTranscript {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let frame =
            LoadedFrame::SessionLoaded { session_id: &self.session_id, messages: &self.messages };
        frame.serialize(serializer)
    }
}

/// Write one frame and flush it, so a streaming client sees tokens as they arrive.
///
/// A frame that cannot serialize is logged and dropped rather than ending the session: losing one
/// event is recoverable for a client, and a dead stream is not. Nothing is written until the whole
/// frame has encoded, so a failure never leaves half a line.
pub(super) async fn write_frame<W, F>(out: &mut W, frame: &F) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
    F: Serialize + ?Sized,
{
    let mut encoded = Pieces::default();
    if let Err(error) = serde_json::to_writer(&mut encoded, frame) {
        tracing::warn!(%error, "rpc: dropping an event that would not serialize");
        return Ok(());
    }
    for piece in encoded.full.iter().chain(std::iter::once(&encoded.current)) {
        out.write_all(piece).await?;
    }
    out.write_all(b"\n").await?;
    out.flush().await
}

/// An encoded frame in pieces of at most `CHUNK_BYTES`. The first grows like any buffer, so a
/// token's frame costs what it always did, and a large frame is never copied into a bigger one.
#[derive(Default)]
struct Pieces {
    full: Vec<Vec<u8>>,
    current: Vec<u8>,
}

impl std::io::Write for Pieces {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.current.len() == CHUNK_BYTES {
            let next = Vec::with_capacity(CHUNK_BYTES);
            self.full.push(std::mem::replace(&mut self.current, next));
        }
        let taken = bytes.len().min(CHUNK_BYTES - self.current.len());
        self.current.extend_from_slice(&bytes[..taken]);
        Ok(taken)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
