//! retry — a provider that could not answer just now gets the same request again.
//!
//! Only before it has said anything. A rate limit, a request that never arrived, an overloaded
//! endpoint or a stream that went silent before its first event all leave the conversation exactly
//! as it was, so the honest fix is the same request after a pause. Once a stream has produced
//! output, replaying it would draw that output twice and bill it twice, so a failure there ends
//! the turn the way it always did.

use std::time::Duration;

use futures::StreamExt;

use super::*;
use crate::provider::{CompletionRequest, ProviderError, ProviderEvent};

/// Tries after the first, unless `LUXE_PROVIDER_RETRIES` says otherwise.
const DEFAULT_RETRIES: u32 = 3;

/// The pause before the first retry, doubling for each after it.
const FIRST_PAUSE: Duration = Duration::from_secs(2);

/// The longest pause between tries.
const LONGEST_PAUSE: Duration = Duration::from_secs(30);

/// How many times a failed request is sent again: `LUXE_PROVIDER_RETRIES`, `0` for never.
fn retries() -> u32 {
    std::env::var("LUXE_PROVIDER_RETRIES")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(DEFAULT_RETRIES)
}

/// The pause before retry `attempt`, counting from one.
fn pause(attempt: u32) -> Duration {
    FIRST_PAUSE.saturating_mul(1 << attempt.saturating_sub(1).min(8)).min(LONGEST_PAUSE)
}

/// Why a stream's first event says to try again, or `None` when it says anything else.
fn transient_first(event: &ProviderEvent) -> Option<String> {
    match event {
        ProviderEvent::Error { message, kind } => {
            let error = ProviderError::reported(kind.clone(), message.clone());
            error.kind().is_transient().then(|| message.clone())
        }
        _ => None,
    }
}

impl Engine {
    /// Opens `request`'s stream on `model`'s provider, sending it again while the provider fails
    /// in a way a second try can fix and has not yet said anything.
    ///
    /// The first event is read here to see which it is, then handed back at the head of the
    /// stream, so the reader downstream sees exactly what the provider sent. A cancel during a
    /// pause ends the wait at once.
    pub(super) async fn stream_with_retry(
        &self,
        model: &crate::provider::ModelInfo,
        request: CompletionRequest,
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<futures::stream::BoxStream<'static, ProviderEvent>, ProviderError> {
        let attempts = retries();
        let mut attempt = 0;
        loop {
            let opened =
                self.provider_for(model).stream(request.clone(), self.cancel_tx.subscribe()).await;
            let reason = match opened {
                Ok(mut stream) => match stream.next().await {
                    Some(first) => match transient_first(&first) {
                        Some(reason) if attempt < attempts && !self.is_cancelled() => reason,
                        _ => {
                            return Ok(futures::stream::once(async { first }).chain(stream).boxed())
                        }
                    },
                    None => return Ok(futures::stream::empty().boxed()),
                },
                Err(error) if error.kind().is_transient() && attempt < attempts => {
                    error.to_string()
                }
                Err(error) => return Err(error),
            };
            if self.is_cancelled() {
                return Ok(futures::stream::empty().boxed());
            }
            attempt += 1;
            warn!(attempt, attempts, %reason, "the provider could not answer; trying again");
            let _ = event_tx
                .send(Event::ProviderRetrying { attempt, attempts, reason: reason.clone() })
                .await;
            let mut cancel = self.cancel_tx.subscribe();
            tokio::select! {
                () = tokio::time::sleep(pause(attempt)) => {}
                _ = cancel.wait_for(|cancelled| *cancelled) => {
                    return Ok(futures::stream::empty().boxed());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pause_doubles_and_stops_growing() {
        let secs: Vec<u64> = (1..=6).map(|attempt| pause(attempt).as_secs()).collect();
        assert_eq!(secs, [2, 4, 8, 16, 30, 30]);
    }

    #[test]
    fn only_a_failure_a_second_try_can_fix_is_retried() {
        let error = |message: &str, kind| ProviderEvent::Error { message: message.into(), kind };
        use crate::provider::ProviderErrorKind as Kind;
        assert!(transient_first(&error("x", Kind::RateLimit)).is_some());
        assert!(transient_first(&error("x", Kind::Transport)).is_some());
        assert!(
            transient_first(&error("overloaded_error", Kind::Other("anthropic".into()))).is_some()
        );
        assert!(transient_first(&error("invalid_api_key", Kind::Auth)).is_none());
        assert!(transient_first(&error("overloaded", Kind::Auth)).is_none(), "the kind decides");
        assert!(transient_first(&error("bad request", Kind::Other("x".into()))).is_none());
        let text = ProviderEvent::TextDelta { index: 0, delta: "hi".into() };
        assert!(transient_first(&text).is_none(), "output has started");
    }
}
