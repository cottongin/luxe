use super::*;

/// Each phrase on its own decides the kind, so a reworded clause cannot quietly stop a retry
/// or start one. One row a phrase, because a chain of `||` hides a dead link behind the others.
#[test]
fn every_phrase_alone_decides_the_kind() {
    use ProviderErrorKind as Kind;
    let rows = [
        ("HTTP 401", Kind::Auth),
        ("HTTP 403", Kind::Auth),
        ("invalid_api_key", Kind::Auth),
        ("Invalid API key", Kind::Auth),
        ("Unauthorized", Kind::Auth),
        ("Authentication failed", Kind::Auth),
        ("HTTP 429", Kind::RateLimit),
        ("Rate limit reached", Kind::RateLimit),
        ("rate_limit_error", Kind::RateLimit),
        ("connection refused", Kind::Transport),
        ("connection reset by peer", Kind::Transport),
        ("connection closed before message completed", Kind::Transport),
        ("dns error", Kind::Transport),
        ("tcp connect error", Kind::Transport),
        ("timed out after 60s", Kind::Transport),
        ("overloaded_error", Kind::Unavailable),
        ("server_error", Kind::Unavailable),
        ("500 Internal Server Error", Kind::Unavailable),
        ("502 Bad Gateway", Kind::Unavailable),
        ("503 Service Unavailable", Kind::Unavailable),
        ("504 Gateway Timeout", Kind::Unavailable),
        ("error 529 <unknown status code>", Kind::Unavailable),
        ("exceeds the context length", Kind::Overflow),
        ("context_length_exceeded", Kind::Overflow),
        ("maximum context is 200k", Kind::Overflow),
        ("too many tokens", Kind::Overflow),
    ];
    for (message, kind) in rows {
        assert_eq!(classify_provider_message(message), kind, "{message}");
    }
    assert_eq!(classify_provider_message("400 bad request"), Kind::Other("provider".into()));
    assert!(Kind::Unavailable.is_transient() && Kind::RateLimit.is_transient());
    assert!(Kind::Transport.is_transient());
    assert!(!Kind::Auth.is_transient() && !Kind::Overflow.is_transient());
    assert!(!Kind::Other("x".into()).is_transient());
}

/// A number inside a longer one is not a status code, and an overflow is decided first.
///
/// The break this catches: "130401 tokens" in an overflow body read as a 401, so the reader
/// was told to log in again instead of the conversation being compacted; "1429" read as a rate
/// limit the same way.
#[test]
fn classify_does_not_read_numbers_inside_other_numbers() {
    use ProviderErrorKind as Kind;
    let overflow = "prompt is too long: the request resulted in 130401 tokens, past the \
                    maximum context of 128000";
    assert_eq!(classify_provider_message(overflow), Kind::Overflow);
    assert_eq!(classify_provider_message("job 1429 failed"), Kind::Other("provider".into()));
    assert_eq!(classify_provider_message("ref 4030 is stale"), Kind::Other("provider".into()));
    assert_eq!(classify_provider_message("order 74012 missing"), Kind::Other("provider".into()));
    assert_eq!(classify_provider_message("status 401"), Kind::Auth, "a code alone still is");
}

/// An HTTP error is judged by its status, and only a 4xx without a status of its own reads
/// the body.
#[test]
fn an_http_error_is_judged_by_its_status_first() {
    use ProviderErrorKind as Kind;
    let http = |status, body: &str| ProviderError::Http { status, body: body.into() }.kind();
    let rows = [
        (401, "{}", Kind::Auth),
        (403, "{}", Kind::Auth),
        (429, "{}", Kind::RateLimit),
        (408, "{}", Kind::Unavailable),
        (500, "{}", Kind::Unavailable),
        (503, "context length exceeded", Kind::Unavailable),
        (529, "{}", Kind::Unavailable),
        (400, "maximum context length is 8192 tokens; you sent 130401", Kind::Overflow),
        (413, "too many tokens", Kind::Overflow),
        (400, "invalid tool schema", Kind::Other("provider".into())),
        (404, "{}", Kind::Other("provider".into())),
    ];
    for (status, body, kind) in rows {
        assert_eq!(http(status, body), kind, "{status} {body}");
    }
}

/// A kind the provider named survives into the error built from it, and one that names
/// nothing leaves the words to decide.
///
/// What this pins is the constructor: the kind a mid-stream error keeps is not read by anything
/// yet, so a stream that dropped it would pass every other test.
#[test]
fn a_reported_kind_is_kept() {
    let reported =
        ProviderError::reported(ProviderErrorKind::Unavailable, "Please try again".into());
    assert_eq!(reported.kind(), ProviderErrorKind::Unavailable);
    assert_eq!(reported.to_string(), "provider error: Please try again");
    let unnamed = ProviderError::reported(ProviderErrorKind::Other("x".into()), "HTTP 429".into());
    assert_eq!(unnamed.kind(), ProviderErrorKind::RateLimit);
}

#[derive(Debug)]
struct Layer {
    text: &'static str,
    cause: Option<Box<Layer>>,
}

impl std::fmt::Display for Layer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.text)
    }
}

impl std::error::Error for Layer {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.as_deref().map(|cause| cause as &(dyn std::error::Error + 'static))
    }
}

fn nested(texts: &[&'static str]) -> Layer {
    let mut layers = texts.iter().rev();
    let mut current = Layer { text: layers.next().expect("at least one"), cause: None };
    for text in layers {
        current = Layer { text, cause: Some(Box::new(current)) };
    }
    current
}

/// The cause is what a transport failure is *about*, so it has to survive being formatted.
///
/// The break this catches, which shipped: `format!("{e}")` on a `reqwest::Error`. Its `Display`
/// stops at "error sending request for url (...)" and the reason sits three levels down its
/// source chain — so every connection refused, DNS failure and timeout in Luxe reported the same
/// uninformative line, and a real provider outage was indistinguishable from a typo in a base URL.
#[test]
fn the_whole_chain_reaches_the_message() {
    let error = nested(&[
        "error sending request for url (https://x/y)",
        "client error (Connect)",
        "tcp connect error",
        "Connection refused (os error 111)",
    ]);
    let described = describe_error_chain(&error);
    assert!(described.starts_with("error sending request for url"), "{described}");
    assert!(described.contains("Connection refused (os error 111)"), "{described}");
    assert!(described.contains("tcp connect error"), "{described}");
}

/// A layer that merely restates its parent is not repeated.
#[test]
fn a_restated_cause_is_not_repeated() {
    let error = nested(&["request failed: broken pipe", "broken pipe"]);
    assert_eq!(describe_error_chain(&error), "request failed: broken pipe");
}

/// An error with no cause reads exactly as it always did.
#[test]
fn a_lone_error_is_unchanged() {
    assert_eq!(describe_error_chain(&nested(&["plain failure"])), "plain failure");
}

/// A transport failure classifies as one, not as a provider refusal.
///
/// It decides the reaction: a connection reset is worth retrying, whereas an auth failure needs
/// a new credential and an overflow needs a smaller prompt. Lumping them together is how a
/// retryable blip gets reported as something the user has to fix.
#[test]
fn transport_failures_are_classified_as_transport() {
    for message in [
        "network error: error sending request for url (https://x): tcp connect error: Connection refused (os error 111)",
        "network error: connection closed before message completed",
        "network error: dns error: failed to lookup address information",
        "network error: operation timed out",
        "network error: connection reset by peer",
    ] {
        assert_eq!(
            classify_provider_message(message),
            ProviderErrorKind::Transport,
            "{message}"
        );
    }
}

/// Classification of everything else is unchanged — transport must not swallow the others.
#[test]
fn transport_does_not_capture_the_other_kinds() {
    assert_eq!(classify_provider_message("HTTP 401 unauthorized"), ProviderErrorKind::Auth);
    assert_eq!(classify_provider_message("429 rate limit"), ProviderErrorKind::RateLimit);
    assert_eq!(
        classify_provider_message("maximum context length exceeded"),
        ProviderErrorKind::Overflow
    );
}
