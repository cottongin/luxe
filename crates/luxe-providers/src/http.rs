//! One HTTP client for the whole process.
//!
//! `reqwest::Client` owns a connection pool and a TLS configuration, so building one per request
//! throws away keep-alive and pays a fresh handshake every turn. It also stops being free once luxe
//! registers an adapter per known provider: twenty pools for one active conversation.
//!
//! Cloning the handle is cheap and shares the pool, which is the intended usage.

use std::sync::OnceLock;
use std::time::Duration;

/// How long to wait for a connection to open. Covers TCP and TLS only: the wait for the response
/// to start is [`START_TIMEOUT`], and a stream that goes quiet is [`IDLE_TIMEOUT`]'s.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a provider has to start answering a model request: its response headers.
///
/// Not a whole-request timeout, because a body streams for as long as the model talks. Without
/// it a provider that accepted the connection and then said nothing held the turn open for good.
pub(crate) const START_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a model's stream may go without a byte before it counts as stalled.
///
/// Generous, because a reasoning model can think for a long while between events, and a stall
/// before the first event is retried rather than reported.
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(180);

/// [`START_TIMEOUT`], or `LUXE_STREAM_START_TIMEOUT` in seconds; `0` waits forever.
pub(crate) fn start_timeout() -> Option<Duration> {
    timeout_from_env("LUXE_STREAM_START_TIMEOUT", START_TIMEOUT)
}

/// [`IDLE_TIMEOUT`], or `LUXE_STREAM_IDLE_TIMEOUT` in seconds; `0` waits forever.
pub(crate) fn idle_timeout() -> Option<Duration> {
    timeout_from_env("LUXE_STREAM_IDLE_TIMEOUT", IDLE_TIMEOUT)
}

fn timeout_from_env(name: &str, default: Duration) -> Option<Duration> {
    let seconds = std::env::var(name).ok().and_then(|value| value.trim().parse::<u64>().ok());
    match seconds {
        Some(0) => None,
        Some(seconds) => Some(Duration::from_secs(seconds)),
        None => Some(default),
    }
}

/// Sends a model request and waits for it to start answering, at most [`start_timeout`].
///
/// A timeout says "timed out", which is what classifies it as a transport failure worth trying
/// again rather than a request to change.
pub(crate) async fn send(
    request: reqwest::RequestBuilder,
) -> Result<reqwest::Response, luxe_core::provider::ProviderError> {
    let network = |error: reqwest::Error| {
        luxe_core::provider::ProviderError::Other(format!(
            "network error: {}",
            luxe_core::provider::describe_error_chain(&error)
        ))
    };
    match start_timeout() {
        None => request.send().await.map_err(network),
        Some(limit) => match tokio::time::timeout(limit, request.send()).await {
            Ok(sent) => sent.map_err(network),
            Err(_) => Err(luxe_core::provider::ProviderError::Other(format!(
                "network error: timed out after {}s waiting for the provider to start answering",
                limit.as_secs()
            ))),
        },
    }
}

/// The error for a response that came back with a failing `status`, carrying both halves so the
/// status decides its kind rather than a search of the body.
pub(crate) fn status_error(
    status: reqwest::StatusCode,
    body: String,
) -> luxe_core::provider::ProviderError {
    luxe_core::provider::ProviderError::Http { status: status.as_u16(), body }
}

/// What a provider's own error type or code says to do about it.
///
/// Read from the payload, so it never depends on the wording beside it. A type naming none of
/// these stays `Other`, which leaves the words to decide.
pub(crate) fn kind_of_error_type(error_type: &str) -> luxe_core::provider::ProviderErrorKind {
    use luxe_core::provider::ProviderErrorKind as Kind;
    match error_type {
        "overloaded_error" | "api_error" | "server_error" | "service_unavailable" => {
            Kind::Unavailable
        }
        "rate_limit_error" | "rate_limit_exceeded" => Kind::RateLimit,
        "authentication_error" | "permission_error" | "invalid_api_key" => Kind::Auth,
        "context_length_exceeded" => Kind::Overflow,
        other => Kind::Other(other.to_string()),
    }
}

/// What an error's `code` says: a name, like a type, or a number, read as the HTTP status it is.
///
/// Servers disagree: OpenAI sends `"context_length_exceeded"`, vLLM and llama.cpp send `429`.
pub(crate) fn kind_of_code(
    code: &serde_json::Value,
    message: &str,
) -> Option<luxe_core::provider::ProviderErrorKind> {
    match code {
        serde_json::Value::String(name) => Some(kind_of_error_type(name)),
        serde_json::Value::Number(number) => {
            let status = u16::try_from(number.as_u64()?).ok()?;
            let body = message.to_string();
            Some(luxe_core::provider::ProviderError::Http { status, body }.kind())
        }
        _ => None,
    }
}

/// What an error frame's `type` and `code` say, whichever names something to do, since a proxy
/// may fill in either one; else the type's own name, else "unknown".
pub(crate) fn kind_of_frame(
    error_type: Option<&str>,
    code: Option<&serde_json::Value>,
    message: &str,
) -> luxe_core::provider::ProviderErrorKind {
    use luxe_core::provider::ProviderErrorKind as Kind;
    let typed = error_type.map(kind_of_error_type);
    let coded = code.and_then(|code| kind_of_code(code, message));
    let kinds: Vec<Kind> = [typed, coded].into_iter().flatten().collect();
    let named = kinds.iter().find(|kind| !matches!(kind, Kind::Other(_)));
    named.or(kinds.first()).cloned().unwrap_or_else(|| Kind::Other("unknown".into()))
}

/// A JSON request with `headers` on it, `Content-Type` among them or not.
///
/// `json` sets `Content-Type` itself and `header` *appends*, so a request that sets both carries
/// the header twice. OpenAI tolerates that and a strict proxy does not: an HTTP/1.1 server is
/// entitled to refuse a duplicated `Content-Type` outright, and aiohttp does. One owner for the
/// shape, so no adapter can get it wrong again.
pub(crate) fn json_request(
    request: reqwest::RequestBuilder,
    headers: &[(&str, &str)],
    body: &serde_json::Value,
) -> reqwest::RequestBuilder {
    headers
        .iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case("content-type"))
        .fold(request.json(body), |request, (name, value)| request.header(*name, *value))
}

pub(crate) fn shared_client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .user_agent(user_agent())
                .build()
                .unwrap_or_else(|error| {
                    // A client that can't be built means no TLS backend; every request would fail
                    // anyway, and a default client gives a clearer error at the call site.
                    tracing::error!(%error, "falling back to a default HTTP client");
                    reqwest::Client::new()
                })
        })
        .clone()
}

/// `luxe/<version>` — what providers see in their logs.
pub(crate) fn user_agent() -> String {
    format!("luxe/{}", luxe_core::version::VERSION)
}

#[cfg(test)]
mod json_request_tests {
    use super::*;

    /// One `Content-Type`, whoever asked for it: a duplicate is refused by strict servers.
    #[test]
    fn a_json_request_carries_one_content_type() {
        let headers = [("Content-Type", "application/json"), ("Accept", "text/event-stream")];
        let request = json_request(
            reqwest::Client::new().post("http://127.0.0.1:9/x"),
            &headers,
            &serde_json::json!({"a": 1}),
        )
        .build()
        .expect("a request");
        assert_eq!(request.headers().get_all("content-type").iter().count(), 1);
        assert_eq!(request.headers()["accept"], "text/event-stream");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server that takes the request and never answers: the wait ends at the start timeout,
    /// saying "timed out", which is what makes it a failure worth trying again.
    ///
    /// The clock is paused only once the request has arrived. Paused from the start, it jumped to
    /// the next timer while the OS was still connecting, which on macOS is often the case, and the
    /// connect timeout fired instead of the one under test.
    #[tokio::test]
    async fn a_provider_that_never_starts_answering_times_out() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let (arrived, heard) = tokio::sync::oneshot::channel();
        let _held = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buffer = [0u8; 64];
            let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buffer).await;
            let _ = arrived.send(());
            std::future::pending::<()>().await;
        });
        let request = shared_client().post(format!("http://{address}/v1/messages")).body("{}");
        let waiting = tokio::spawn(send(request));
        heard.await.expect("the request arrived");
        tokio::time::pause();
        let error = waiting.await.expect("joined").expect_err("nothing came back");
        let message = error.to_string();
        assert!(message.contains("timed out after 60s"), "{message}");
        assert!(error.kind().is_transient(), "{message}");
    }

    #[test]
    fn handing_out_the_client_twice_costs_nothing_and_identifies_luxe() {
        let first = shared_client();
        let second = shared_client();
        // Both handles are usable; the point of the OnceLock is that only one pool exists behind
        // them, which isn't observable from the public API beyond "this doesn't panic or stall".
        assert!(first.get("http://127.0.0.1:1/").build().is_ok());
        assert!(second.get("http://127.0.0.1:1/").build().is_ok());
        assert!(user_agent().starts_with("luxe/"));
    }
}
