use super::*;

/// `herdr status server --json` from herdr 0.9.3, running and stopped, trimmed to what matters.
const RUNNING: &str = r#"{"status":"running","running":true,"version":"0.9.3","protocol":22,"compatible":true,"endpoint_compatible":true,"session":"luxetest"}"#;
const STOPPED: &str = r#"{"status":"not_running","running":false,"version":null,"protocol":null,"capabilities":null,"compatible":null,"endpoint_compatible":null,"session":null}"#;

/// Herdr counts as running only when its server is up and speaks this CLI's protocol.
///
/// The break this catches: reading only `running`, so a server left from an older herdr gets
/// commands it can't answer and every start fails, when a child would have worked.
#[test]
fn herdr_runs_when_its_server_is_up_and_compatible() {
    assert!(is_running(RUNNING));
    assert!(!is_running(STOPPED));
    assert!(!is_running(&RUNNING.replace(r#""compatible":true"#, r#""compatible":false"#)));
    assert!(!is_running("not json"));
}

/// The pane to run `luxe` in is read from what `workspace create` printed.
///
/// The break this catches: a field renamed in the reply this reads, copied from herdr 0.9.3.
#[test]
fn the_new_pane_is_read_from_herdrs_reply() {
    let created = r#"{"id":"cli:workspace:create","result":{"root_pane":{"agent_status":"unknown","cwd":"/tmp/herdr-proj","focused":false,"pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2"},"tab":{"tab_id":"w2:t1","workspace_id":"w2"},"type":"workspace_created","workspace":{"label":"herdr-proj","workspace_id":"w2"}}}"#;
    assert_eq!(parse::<Created>(created).unwrap().root_pane.pane_id, "w2:p1");
}

/// A new session joins the workspace already working in its folder, and only that one.
///
/// The break this catches: matching a folder's parent or neighbor, which puts the tab in some other
/// project's workspace.
#[test]
fn a_session_joins_the_workspace_in_its_folder() {
    let listed = r#"{"id":"cli:pane:list","result":{"panes":[{"cwd":"/home/alex/app-two","pane_id":"w1:p1","workspace_id":"w1"},{"cwd":"/home/alex/app","pane_id":"w2:p1","workspace_id":"w2"}],"type":"pane_list"}}"#;
    let panes = parse::<PaneList>(listed).unwrap().panes;
    assert_eq!(workspace_for(&panes, Path::new("/home/alex/app")), Some("w2"));
    assert_eq!(workspace_for(&panes, Path::new("/home/alex")), None);
}

/// A failure says why in herdr's words, and anything that isn't its JSON passes through.
///
/// The break this catches: showing the device herdr's raw JSON, or nothing when it isn't JSON.
#[test]
fn a_refusal_says_why_in_herdrs_words() {
    let stderr = r#"{"error":{"code":"pane_not_found","message":"pane w9:p9 not found"},"id":"cli:request"}"#;
    assert_eq!(refusal(stderr), "pane w9:p9 not found");
    assert_eq!(refusal("  no server\n"), "no server");
}
