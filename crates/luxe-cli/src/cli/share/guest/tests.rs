use super::*;

fn name_of(line: &str) -> Option<String> {
    introduction(line).map(|said| said.name)
}

/// An introduction gives its name, made fit for one line; nothing else does.
///
/// The break this catches, one per line: taking the name from another frame (submit), passing a
/// newline or escape code into the terminal's notice (control), letting a long name push the
/// notice off the line (long), or naming a client nothing (blank).
#[test]
fn only_an_introduction_names_a_client() {
    let introduce = |name: &str| format!(r#"{{"kind":"introduce","content":{{"name":"{name}"}}}}"#);
    assert_eq!(name_of(&introduce("iPhone")).as_deref(), Some("iPhone"));
    let submit = r#"{"kind":"submit_user_message","content":{"text":"introduce yourself"}}"#;
    assert_eq!(name_of(submit), None);
    assert_eq!(name_of(&introduce(r"Alex\n\u001b[2Jphone")).as_deref(), Some("Alex  [2Jphone"));
    assert_eq!(name_of(&introduce(&"x".repeat(60))).map(|name| name.len()), Some(NAME_LIMIT));
    assert_eq!(name_of(&introduce("  ")), None);
}

/// What `input` passes on to the protocol, and the guest it leaves.
async fn sifted(input: &str) -> (String, Guest) {
    let (sifted, mut protocol) = tokio::io::duplex(1024);
    let (guest, named) = Guest::new(Some(7));
    sift(input.as_bytes(), sifted, named).await;
    let mut passed = String::new();
    tokio::io::AsyncReadExt::read_to_string(&mut protocol, &mut passed).await.unwrap();
    (passed, guest)
}

const WATCHER: &str =
    "{\"kind\":\"introduce\",\"content\":{\"name\":\"Notifications\",\"watch_only\":true}}\n";

/// The client's other lines reach the protocol in order, and its introduction doesn't.
///
/// The break this catches: forwarding the introduction, which the protocol refuses back to the
/// bridge as if it had sent something wrong; or losing the line after it.
#[tokio::test]
async fn sifting_keeps_the_introduction_and_passes_the_rest() {
    let input = "{\"kind\":\"cancel\"}\n{\"kind\":\"introduce\",\"content\":{\"name\":\"iPad\"}}\n\
                 {\"kind\":\"retry\"}\n";
    let (passed, guest) = sifted(input).await;
    assert_eq!(passed, "{\"kind\":\"cancel\"}\n{\"kind\":\"retry\"}\n");
    assert_eq!(guest.to_string(), "iPad");
    assert!(!guest.watch_only());
}

/// A client that opens as watch-only is marked so, and nothing it sends after reaches the
/// protocol.
///
/// The break this catches: an unannounced client steering or approving, unseen at the desk.
#[tokio::test]
async fn a_watch_only_client_cant_act() {
    let (passed, guest) = sifted(&format!("{WATCHER}{{\"kind\":\"cancel\"}}\n")).await;
    assert_eq!(passed, "");
    assert!(guest.watch_only());
    assert_eq!(guest.to_string(), "Notifications");
}

/// Watch-only counts only as the opening line.
///
/// The break this catches: a client cancelling the turn, then introducing itself as watch-only so
/// the terminal never says it was there.
#[tokio::test]
async fn watch_only_after_acting_is_just_a_name() {
    let (passed, guest) = sifted(&format!("{{\"kind\":\"cancel\"}}\n{WATCHER}")).await;
    assert_eq!(passed, "{\"kind\":\"cancel\"}\n");
    assert!(!guest.watch_only());
}
