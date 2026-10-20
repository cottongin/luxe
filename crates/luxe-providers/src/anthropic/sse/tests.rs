//! What the Anthropic decoder makes of the stream's errors and stop reasons.

use super::*;

fn decode(text: &str) -> Vec<ProviderEvent> {
    parse_anthropic_sse_text(text)
}

/// An `overloaded_error` frame is a provider that cannot serve the request just now, whatever its
/// message says.
///
/// The break this catches: the frame's type was thrown away and its kind set to an opaque
/// "anthropic error", so whether it was retried rode on the message happening to say "overloaded".
#[test]
fn an_overloaded_error_frame_is_unavailable() {
    let events = decode(
        "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Please try again\"}}",
    );
    let [ProviderEvent::Error { kind, message }] = &events[..] else { panic!("{events:?}") };
    assert_eq!(*kind, luxe_core::provider::ProviderErrorKind::Unavailable);
    assert_eq!(message, "Please try again");
}

fn stop(reason: &str) -> Vec<ProviderEvent> {
    decode(&format!(
        "event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"{reason}\"}}}}"
    ))
}

/// A refusal is the model declining, not finishing, and a stop reason luxe does not know is
/// said as itself.
///
/// The break this catches: every stop reason the decoder did not name became `EndTurn`, so a
/// refused request looked like a finished answer.
#[test]
fn a_refusal_is_not_an_end_of_turn() {
    let reason = |name: &str| match &stop(name)[..] {
        [ProviderEvent::Done { reason }] => reason.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(reason("refusal"), StopReason::ContentFilter);
    assert_eq!(reason("pause_turn"), StopReason::Other("pause_turn".into()));
    assert_eq!(reason("end_turn"), StopReason::EndTurn);
}

/// Frames that carry nothing for the engine produce no event, rather than an empty text delta
/// the engine has to filter.
#[test]
fn an_unknown_block_or_delta_produces_nothing() {
    let unknown_block = "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"server_tool_use\"}}";
    assert!(decode(unknown_block).is_empty());
    let unknown_delta = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"citations_delta\"}}";
    assert!(decode(unknown_delta).is_empty());
}
