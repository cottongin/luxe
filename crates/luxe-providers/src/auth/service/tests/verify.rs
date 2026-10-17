//! Verification against an endpoint that stops answering.

use super::*;
use std::time::Duration;

/// A server that accepts, reads the request and then says `reply` (possibly nothing) before going
/// quiet for good. Answers once the request has arrived, so a paused clock can start there.
async fn quiet_server(reply: &'static str) -> (String, tokio::sync::oneshot::Receiver<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let endpoint = format!("http://{}/v1", listener.local_addr().expect("address"));
    let (arrived, heard) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut buffer = [0u8; 1024];
        let _ = socket.read(&mut buffer).await;
        let _ = socket.write_all(reply.as_bytes()).await;
        let _ = arrived.send(());
        std::future::pending::<()>().await;
    });
    (endpoint, heard)
}

/// Verify against `endpoint`, then let the clock run until something gives up.
///
/// The outer two minutes only exists so a verification with no bound of its own fails this test
/// instead of hanging it; the bounded one answers long before.
async fn verified_against(
    endpoint: String,
    heard: tokio::sync::oneshot::Receiver<()>,
) -> Option<Result<Vec<String>, AuthError>> {
    let descriptor = provider_descriptor("openai").expect("in the catalog");
    let verifying = tokio::spawn(async move {
        verify_credential(descriptor, Some(&endpoint), &RequestAuth::none()).await
    });
    heard.await.expect("the request arrived");
    tokio::time::pause();
    tokio::time::timeout(Duration::from_secs(120), verifying).await.ok().map(|done| done.unwrap())
}

/// `luxe auth login` against an endpoint that takes the connection and never answers used to
/// wait until Ctrl-C: only the background refreshers had a timeout.
#[tokio::test]
async fn an_endpoint_that_never_answers_ends_the_verification() {
    let (endpoint, heard) = quiet_server("").await;
    let outcome = verified_against(endpoint, heard).await.expect("gave up on its own");
    let error = outcome.expect_err("nothing came back");
    assert!(matches!(error, AuthError::Network(_)), "got {error:?}");
    assert!(error.to_string().contains("timed out"), "got {error}");
    assert_says_it_once(&error);
}

/// Headers arrive, the body never does: the first timeout is behind us and the read is not
/// covered by it.
#[tokio::test]
async fn an_endpoint_that_stalls_mid_answer_ends_the_verification() {
    let reply = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 500\r\n\r\n{\"data\":";
    let (endpoint, heard) = quiet_server(reply).await;
    let outcome = verified_against(endpoint, heard).await.expect("gave up on its own");
    let error = outcome.expect_err("the list never finished");
    assert!(matches!(error, AuthError::Network(_)), "got {error:?}");
    assert!(error.to_string().contains("timed out"), "got {error}");
    assert_says_it_once(&error);
}

/// Nothing listening reads as the one thing it is, not as a provider error inside a network one.
#[tokio::test]
async fn an_endpoint_that_refuses_the_connection_says_so_plainly() {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let descriptor = provider_descriptor("openai").expect("in the catalog");
    let endpoint = format!("http://127.0.0.1:{port}/v1");
    let error = verify_credential(descriptor, Some(&endpoint), &RequestAuth::none())
        .await
        .expect_err("nothing is listening");
    assert!(error.to_string().contains("could not reach"), "got {error}");
    // The cause, which reqwest's own text for the error leaves out
    assert!(error.to_string().to_lowercase().contains("refused"), "got {error}");
    assert_says_it_once(&error);
}

/// One `network error:` prefix, and no `provider error:` from the adapters' vocabulary.
fn assert_says_it_once(error: &AuthError) {
    let said = error.to_string();
    assert_eq!(said.matches("network error").count(), 1, "got {said}");
    assert!(!said.contains("provider error"), "got {said}");
}
