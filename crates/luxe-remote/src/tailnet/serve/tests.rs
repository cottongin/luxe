use super::*;

fn config(json: &str) -> Value {
    serde_json::from_str(json).unwrap()
}

/// What `tailscale serve --bg http://127.0.0.1:4519` left in `serve status --json` (1.102.3),
/// with the machine's name changed.
const OURS: &str = r#"{"TCP":{"443":{"HTTPS":true}},"Web":{"desk.tail1234.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:4519"}}}}}"#;

/// A root that already proxies to the bridge's port counts as published, however the address is
/// written.
///
/// The break this catches: matching the proxy as an exact string, so `tailscale serve 4519`
/// (which writes `localhost`) is offered again and setup tries to change a working config.
#[test]
fn a_root_proxying_to_the_bridge_is_published() {
    assert_eq!(judge(&config(OURS), 4519), Published::Here);
    let spelled = OURS.replace("http://127.0.0.1:4519", "http://localhost:4519/");
    assert_eq!(judge(&config(&spelled), 4519), Published::Here);
}

/// A root that leads anywhere else is someone's, and setup leaves it alone.
///
/// The break this catches: taking any root handler as the bridge, so setup reports a working
/// setup that serves another app (or the bridge on another port) to the phone.
#[test]
fn a_root_leading_elsewhere_is_not_the_bridge() {
    let other_port = judge(&config(OURS), 4600);
    assert_eq!(other_port, Published::Elsewhere("http://127.0.0.1:4519".into()));
    let lan = OURS.replace("127.0.0.1", "192.168.1.9");
    assert!(matches!(judge(&config(&lan), 4519), Published::Elsewhere(_)));
}

/// Only the HTTPS root counts: a handler on another path or port isn't what the phone opens.
///
/// The break this catches: reading any handler, so `/files` or port 8443 reads as published and
/// the phone gets a 404 at the address setup printed.
#[test]
fn only_the_https_root_counts() {
    assert_eq!(judge(&config("{}"), 4519), Published::Nowhere);
    let other_path = OURS.replace(r#""/":"#, r#""/luxe":"#);
    assert_eq!(judge(&config(&other_path), 4519), Published::Nowhere);
    let other_port = OURS.replace(":443\"", ":8443\"");
    assert_eq!(judge(&config(&other_port), 4519), Published::Nowhere);
}

/// A `tailscale serve` running in a terminal counts too: its config sits under `Foreground`.
///
/// The break this catches: reading only the background config, so setup offers to publish over a
/// foreground serve that already does it.
#[test]
fn a_foreground_serve_counts() {
    let foreground = format!(r#"{{"Foreground":{{"a1b2":{OURS}}}}}"#);
    assert_eq!(judge(&config(&foreground), 4519), Published::Here);
}
