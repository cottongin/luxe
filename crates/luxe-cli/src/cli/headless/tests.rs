use luxe_core::event::{Event, StopReason};

use super::*;

/// A channel holding `events`, closed after them unless `open`.
fn feed(events: Vec<Event>) -> tokio::sync::mpsc::Receiver<Event> {
    let (tx, rx) = tokio::sync::mpsc::channel(events.len().max(1));
    for event in events {
        tx.try_send(event).expect("room for the fixture");
    }
    rx
}

/// A turn that failed is a failed run, in both formats.
///
/// The break this catches: the error was printed and the run exited 0, so a script calling
/// `luxe -p` could not tell a refused key from an answer without parsing what it printed.
#[tokio::test]
async fn headless_exits_nonzero_on_a_provider_error() {
    let failed = || vec![Event::Error("turn error: invalid api key".into())];
    let text = stream_headless_text(&mut feed(failed())).await;
    assert!(matches!(&text, TurnOutcome::Failed(message) if message.contains("invalid api key")));
    let json = stream_headless_json(&mut feed(failed())).await;
    assert!(matches!(json, TurnOutcome::Failed(_)));
    assert!(finish(text).unwrap_err().is::<AlreadyReported>(), "said once, then exit 1");
}

/// An engine that went away before the turn ended is a failed run too, said as such.
#[tokio::test]
async fn headless_exits_nonzero_when_the_engine_disconnects() {
    let outcome = stream_headless_text(&mut feed(vec![Event::Notice("working".into())])).await;
    assert!(matches!(outcome, TurnOutcome::Disconnected));
    let error = finish(outcome).unwrap_err();
    assert!(!error.is::<AlreadyReported>());
    assert!(error.to_string().contains("stopped before the turn ended"), "{error}");
}

/// A turn that ended is a run that succeeded, however it ended.
#[tokio::test]
async fn a_finished_turn_exits_zero() {
    let ended = vec![Event::TurnEnded { reason: StopReason::EndTurn }];
    let outcome = stream_headless_json(&mut feed(ended)).await;
    assert!(matches!(outcome, TurnOutcome::Finished));
    assert!(finish(outcome).is_ok());
}
