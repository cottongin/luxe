//! What each adapter makes of a provider's error: the status first, then the provider's own error
//! type, and only then its words.

#![allow(clippy::unwrap_used)]

use luxe_core::provider::{ProviderErrorKind as Kind, ProviderEvent};

use crate::sse::SseDecoder;

/// Every status lands on one kind, whatever the body says beside it.
///
/// The break this catches: the five adapters' error paths each wrote `"{status}: {body}"` into
/// one string, which core then searched for numbers — so a 400 quoting "130401 tokens" was an
/// auth failure, and only two of three adapters made a 401 an auth error at all.
#[test]
fn a_status_decides_the_kind() {
    let rows = [
        (401, "{}", Kind::Auth),
        (403, "{}", Kind::Auth),
        (429, "{}", Kind::RateLimit),
        (500, "{}", Kind::Unavailable),
        (529, "{\"type\":\"overloaded_error\"}", Kind::Unavailable),
        (400, "prompt is too long: 130401 tokens > 128000 maximum context", Kind::Overflow),
        (400, "{\"type\":\"invalid_request_error\"}", Kind::Other("provider".into())),
    ];
    for (status, body, kind) in rows {
        let status = reqwest::StatusCode::from_u16(status).unwrap();
        assert_eq!(crate::http::status_error(status, body.into()).kind(), kind, "{status}");
    }
}

/// The kind an error event carries, and nothing else.
fn kind_of(events: &[ProviderEvent]) -> Kind {
    match events {
        [ProviderEvent::Error { kind, .. }] => kind.clone(),
        other => panic!("one error event, not {other:?}"),
    }
}

/// A chat-completions error frame takes its kind from its `type`, or its `code`, whichever names
/// something, and a numeric code is read as the status it is.
///
/// The break this catches: the frame's type was stored as an opaque string kind, so what the
/// engine made of an error rode on its wording.
#[test]
fn a_compat_error_frame_is_typed() {
    let decode = |frame: &str| {
        let mut events = Vec::new();
        crate::openai_compat::CompatDecoder::default().record(frame, &mut events);
        kind_of(&events)
    };
    let overloaded = r#"data: {"error":{"message":"Overloaded","type":"overloaded_error"}}"#;
    assert_eq!(decode(overloaded), Kind::Unavailable);
    let limited = r#"data: {"error":{"message":"slow down","code":"rate_limit_exceeded"}}"#;
    assert_eq!(decode(limited), Kind::RateLimit);
    let quota = r#"data: {"error":{"message":"no","type":"insufficient_quota"}}"#;
    assert_eq!(decode(quota), Kind::Other("insufficient_quota".into()));
    // OpenAI's own overflow frame: the type says nothing, the code does
    let overflow = r#"data: {"error":{"message":"too long","type":"invalid_request_error","code":"context_length_exceeded"}}"#;
    assert_eq!(decode(overflow), Kind::Overflow);
}

/// A numeric `code`, as vLLM and llama.cpp send, is a status, and the frame still parses.
///
/// The break this catches: `code` was read as a string, so a frame carrying a number failed to
/// parse at all, was dropped, and the turn ended as though it had finished.
#[test]
fn a_numeric_error_code_is_read_as_a_status() {
    let decode = |frame: &str| {
        let mut events = Vec::new();
        crate::openai_compat::CompatDecoder::default().record(frame, &mut events);
        kind_of(&events)
    };
    assert_eq!(decode(r#"data: {"error":{"message":"slow down","code":429}}"#), Kind::RateLimit);
    let long = r#"data: {"error":{"message":"maximum context length is 8192","code":400}}"#;
    assert_eq!(decode(long), Kind::Overflow);
    let events = crate::openai_responses::parse_responses_sse(
        "event: error\ndata: {\"code\":503,\"message\":\"busy\"}\n\n",
    );
    assert_eq!(kind_of(&events), Kind::Unavailable);
}

/// A Responses API error event takes its kind from its `code`.
#[test]
fn a_responses_error_event_is_typed() {
    let events = crate::openai_responses::parse_responses_sse(
        "event: error\ndata: {\"code\":\"server_error\",\"message\":\"try again\"}\n\n",
    );
    assert_eq!(kind_of(&events), Kind::Unavailable);
    // An older capture put `response.failed`'s error at the top
    let events = crate::openai_responses::parse_responses_sse(
        "event: response.failed\ndata: {\"error\":{\"code\":\"server_error\",\"message\":\"m\"}}\n\n",
    );
    let [ProviderEvent::Error { kind, message }] = &events[..] else { panic!("{events:?}") };
    assert_eq!((kind, message.as_str()), (&Kind::Unavailable, "m"));
    // `response.failed` carries its error inside the response it failed
    let events = crate::openai_responses::parse_responses_sse(
        "event: response.failed\ndata: {\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"rate_limit_exceeded\",\"message\":\"x\"}}}\n\n",
    );
    let [ProviderEvent::Error { kind, message }] = &events[..] else { panic!("{events:?}") };
    assert_eq!((kind, message.as_str()), (&Kind::RateLimit, "x"));
}
