//! Every adapter's request carries each header once.
//!
//! `json` sets `Content-Type` and `header` appends, so a request built by hand can send it twice,
//! and a strict HTTP/1.1 server refuses that outright. One owner builds the request for all three
//! adapters; these read the head off the wire to hold it.

use futures::StreamExt;
use luxe_core::message::Message;
use luxe_core::provider::{CompletionRequest, ModelInfo, ModelRef, Provider, ThinkingLevel};
use luxe_providers::anthropic::{AnthropicCompat, AnthropicProvider};
use luxe_providers::config::{CompatFlags, ProviderConfig};
use luxe_providers::openai_compat::OpenaiCompatProvider;
use luxe_providers::openai_responses::OpenaiResponsesProvider;
use std::io::{Read, Write};

/// Answers one request with an empty event stream and hands back what the request looked like.
fn serve_once() -> (u16, std::thread::JoinHandle<String>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept");
        let mut raw = Vec::new();
        let mut chunk = [0u8; 4096];
        while !raw.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = socket.read(&mut chunk).unwrap_or(0);
            if read == 0 {
                break;
            }
            raw.extend_from_slice(&chunk[..read]);
        }
        let reply =
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
        let _ = socket.write_all(reply.as_bytes());
        String::from_utf8_lossy(&raw).to_string()
    });
    (port, server)
}

fn request() -> CompletionRequest {
    CompletionRequest {
        model: ModelInfo {
            r#ref: ModelRef::new("test", "test-model"),
            max_tokens: 1024,
            context_window: 8192,
            ..Default::default()
        },
        messages: vec![Message::user("hello")],
        thinking: ThinkingLevel::Off,
        max_tokens: Some(256),
        ..Default::default()
    }
}

/// How many times the head names `header`, whatever its case.
fn times(head: &str, header: &str) -> usize {
    let prefix = format!("{header}:");
    head.lines().filter(|line| line.to_ascii_lowercase().starts_with(&prefix)).count()
}

async fn sent_by(provider: &dyn Provider, server: std::thread::JoinHandle<String>) -> String {
    let (_keep, cancel) = tokio::sync::watch::channel(false);
    let stream = provider.stream(request(), cancel).await.expect("the stream opens");
    let _: Vec<_> = stream.collect().await;
    server.join().expect("the server saw a request")
}

#[tokio::test]
async fn the_chat_completions_request_has_one_content_type() {
    let (port, server) = serve_once();
    let provider = OpenaiCompatProvider::new(
        ProviderConfig { api_key: Some("sk-test".into()), ..Default::default() },
        CompatFlags::default(),
    )
    .with_base_url(format!("http://127.0.0.1:{port}/v1"));
    let head = sent_by(&provider, server).await;
    assert_eq!(times(&head, "content-type"), 1, "{head}");
}

#[tokio::test]
async fn the_responses_request_has_one_content_type() {
    let (port, server) = serve_once();
    let provider = OpenaiResponsesProvider::new(ProviderConfig {
        api_key: Some("sk-test".into()),
        base_url: Some(format!("http://127.0.0.1:{port}/v1")),
        ..Default::default()
    });
    let head = sent_by(&provider, server).await;
    assert_eq!(times(&head, "content-type"), 1, "{head}");
}

#[tokio::test]
async fn the_messages_request_has_one_content_type() {
    let (port, server) = serve_once();
    let provider = AnthropicProvider::new(
        "sk-test".into(),
        Some(format!("http://127.0.0.1:{port}/v1")),
        request().model,
        AnthropicCompat::default(),
    );
    let head = sent_by(&provider, server).await;
    assert_eq!(times(&head, "content-type"), 1, "{head}");
}
