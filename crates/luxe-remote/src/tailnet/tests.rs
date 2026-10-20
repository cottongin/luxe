use super::*;

/// Trimmed from a real `tailscale status --json` (1.102.3), with the names changed.
const RUNNING: &str = r#"{"Version":"1.102.3","BackendState":"Running","Self":{"ID":"nZN73L4Wtr11CNTRL","HostName":"desk","DNSName":"desk.tail1234.ts.net.","OS":"linux","UserID":1618724369016884,"Online":true},"MagicDNSSuffix":"tail1234.ts.net","CurrentTailnet":{"Name":"alex.github","MagicDNSSuffix":"tail1234.ts.net","MagicDNSEnabled":true},"User":{"1618724369016884":{"ID":1618724369016884,"LoginName":"alex@github","DisplayName":"Alex"},"7":{"ID":7,"LoginName":"sam@github","DisplayName":"Sam"}}}"#;

/// The owner is the user behind this machine, picked out of every user the tailnet lists.
///
/// The break this catches: a renamed field read as missing, or taking the first user listed, so
/// the allowlist holds someone else's login and the owner is refused (or a sharer let in).
#[test]
fn the_owner_is_the_user_behind_this_machine() {
    let tailnet = parse_status(RUNNING).unwrap();
    assert_eq!(tailnet.login, "alex@github");
    assert_eq!(tailnet.dns_name, "desk.tail1234.ts.net", "the trailing dot is a DNS detail");
}

/// The machine's name is the first label of its tailnet name, as its address leads with it.
///
/// The break this catches: the phone's apps for two computers named by the tailnet (both
/// `tail1234`) or by the whole address, too long for a home screen label.
#[test]
fn the_machine_is_named_by_its_first_label() {
    assert_eq!(parse_status(RUNNING).unwrap().machine(), "desk");
    let bare = Tailnet { login: String::new(), dns_name: "desk".into() };
    assert_eq!(bare.machine(), "desk");
}

/// A Tailscale that isn't connected says so, rather than handing back a stale login.
///
/// The break this catches: ignoring the state, so a logged-out daemon's leftover user is trusted.
#[test]
fn a_disconnected_tailscale_is_not_an_owner() {
    let stopped = RUNNING.replace(r#""BackendState":"Running""#, r#""BackendState":"NeedsLogin""#);
    let refused = parse_status(&stopped).unwrap_err();
    assert!(matches!(refused, TailnetError::NotRunning(state) if state == "NeedsLogin"));
}
