//! A key typed for a plain-http endpoint is something the user should be told about.

use super::*;

/// Every message the login showed, whatever it was.
fn said(interaction: &Scripted) -> String {
    interaction
        .events()
        .into_iter()
        .filter_map(|event| match event {
            AuthEvent::Info { message, .. } => Some(message),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn custom_login_at(url: &str) -> String {
    let (_dir, service) = service();
    let interaction = Scripted::new(&["my-proxy", url, "openai-completions", "sk-x"]);
    service.login("custom", "api-key", interaction.clone()).await.expect("configured");
    said(&interaction)
}

/// The key goes to whatever URL was typed, and `http://` to another machine sends it readable
/// to everyone on the path. Not refused, since a lab server on a LAN is a fair thing to point at.
#[tokio::test]
async fn a_key_for_a_plain_http_endpoint_elsewhere_comes_with_a_warning() {
    let said = custom_login_at("http://192.168.1.20:8080/v1").await;
    assert!(said.contains("192.168.1.20") && said.contains("unencrypted"), "got: {said}");
}

/// This machine, https and the bare host (which becomes https) have nothing to warn about.
#[tokio::test]
async fn a_key_that_stays_on_this_machine_or_travels_over_tls_is_not_warned_about() {
    for url in [
        "http://127.0.0.1:1234/v1",
        "http://localhost:1234/v1",
        "http://[::1]:1234/v1",
        "https://example.com/v1",
        "example.com/v1",
    ] {
        let said = custom_login_at(url).await;
        assert!(!said.contains("unencrypted"), "{url} was warned about: {said}");
    }
}

/// The warning is only worth anything while the user can still say no, so it comes before the
/// key is asked for. Here nobody answers the key prompt, and the login still said it.
#[tokio::test]
async fn the_warning_comes_before_the_key_is_asked_for() {
    let (_dir, service) = service();
    let interaction =
        Scripted::new(&["my-proxy", "http://192.168.1.20:8080/v1", "openai-completions"]);
    let error = service.login("custom", "api-key", interaction.clone()).await.expect_err("no key");
    assert!(error.is_cancelled(), "got {error:?}");
    assert!(said(&interaction).contains("unencrypted"), "got: {}", said(&interaction));
}
