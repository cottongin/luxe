use super::*;

fn running(pid: u32, session_id: &str, cwd: &str) -> LiveSession {
    LiveSession {
        pid,
        session_id: session_id.to_string(),
        cwd: cwd.to_string(),
        address: format!("/run/luxe/{pid}.sock"),
        luxe_version: "0.1.3".to_string(),
    }
}

fn sessions() -> Vec<LiveSession> {
    vec![
        running(101, "aaaa-1111", "/work/app"),
        running(202, "aaab-2222", "/work/site"),
        running(303, "cccc-3333", "/work/site"),
    ]
}

/// With no target, the session running in this folder is the one joined.
///
/// The break this catches: ignoring the folder, so `luxe join` in one project lands in another's
/// session, or refuses because more than one Luxe is running somewhere.
#[test]
fn no_target_joins_the_session_running_here() {
    let all = sessions();
    assert_eq!(pick(&all, None, "/work/app").unwrap().pid, 101);
}

/// Two sessions in one folder are named, not guessed between.
///
/// The break this catches: taking the first match, which joins whichever pid sorts lower.
#[test]
fn two_sessions_in_the_folder_are_listed_not_guessed() {
    let all = sessions();
    let error = pick(&all, None, "/work/site").unwrap_err().to_string();
    assert!(error.contains("2 sessions match"), "{error}");
    assert!(error.contains("aaab-2222") && error.contains("cccc-3333"), "{error}");
}

/// A target names a session by the start of its id or by its Luxe's process id.
///
/// The break this catches: matching the id exactly, so the short form the listing invites fails,
/// or not accepting a pid, which is what `ps` and herdr show.
#[test]
fn a_target_is_an_id_prefix_or_a_pid() {
    let all = sessions();
    assert_eq!(pick(&all, Some("cccc"), "/elsewhere").unwrap().pid, 303);
    assert_eq!(pick(&all, Some("202"), "/elsewhere").unwrap().session_id, "aaab-2222");
}

/// A prefix two ids share is refused, and so is an empty one.
///
/// The break this catches: an empty target matching every id as a prefix of all of them.
#[test]
fn an_ambiguous_or_empty_target_is_refused() {
    let all = sessions();
    assert!(pick(&all, Some("aaa"), "/work/app").is_err());
    assert!(!names(&all[0], " "));
}

/// Nothing to join says why, and how sharing is turned back on when nothing is running.
///
/// The break this catches: a bare "not found" when every Luxe has `joinable` off, which leaves the
/// reader with nothing to try.
#[test]
fn nothing_to_join_says_what_is_running() {
    let none = pick(&[], None, "/work/app").unwrap_err().to_string();
    assert!(none.contains("joinable"), "{none}");
    let all = sessions();
    let elsewhere = pick(&all, None, "/work/other").unwrap_err().to_string();
    assert!(elsewhere.contains("no session is running in /work/other"), "{elsewhere}");
    assert!(elsewhere.contains("aaaa-1111  pid 101  /work/app"), "{elsewhere}");
}
