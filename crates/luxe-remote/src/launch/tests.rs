use super::*;

fn session(pid: u32, cwd: &Path) -> LiveSession {
    LiveSession {
        pid,
        session_id: format!("session-{pid}"),
        cwd: cwd.to_string_lossy().into_owned(),
        address: String::new(),
        luxe_version: "0.1.3".into(),
    }
}

/// `~` and `~/sub` are the home folder and a folder in it.
///
/// The break this catches: taking `~` literally, so the folder a person types on a phone keyboard
/// is refused as not a full path.
#[test]
fn a_tilde_is_the_home_folder() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join("app")).unwrap();
    assert_eq!(resolve_folder("~", Some(home.path())).unwrap(), home.path());
    assert_eq!(resolve_folder(" ~/app ", Some(home.path())).unwrap(), home.path().join("app"));
}

/// A relative path is refused rather than resolved against wherever the bridge happens to run.
///
/// The break this catches: dropping the full-path check, so `app` starts Luxe in the service's
/// working folder, which nobody chose.
#[test]
fn a_relative_path_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let refused = resolve_folder("app", Some(home.path())).unwrap_err();
    assert!(refused.to_string().contains("isn't a full path"), "{refused}");
}

/// A file, or nothing at all, isn't a folder to start in.
///
/// The break this catches: checking only that the path exists, so a file reaches the child as its
/// working folder and the start fails with an OS error nobody can act on.
#[test]
fn only_a_folder_that_exists_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, "").unwrap();
    for path in [file, dir.path().join("missing")] {
        let refused = resolve_folder(&path.to_string_lossy(), None).unwrap_err();
        assert!(refused.to_string().contains("isn't a folder"), "{refused}");
    }
}

/// The new session is the one that appeared since, in the folder asked for or a parent of it.
///
/// The break this catches, one per session in the list: answering with a session that was already
/// running there (1), or one that started elsewhere at the same moment (2). The last one is found
/// because a Luxe started in a repository's subfolder shares the repository's root.
#[test]
fn the_new_session_is_the_one_that_appeared_where_it_was_asked_for() {
    let root = tempfile::tempdir().unwrap();
    let asked = root.path().join("crates/app");
    std::fs::create_dir_all(&asked).unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let running = [session(1, &asked), session(2, elsewhere.path()), session(3, root.path())];
    let before = HashSet::from([1]);
    let found = newcomer(&running, &before, &asked).expect("the session in the repository");
    assert_eq!(found.pid, 3);
    assert_eq!(newcomer(&running[..2], &before, &asked), None);
}

/// A child started for a device shares its session, and ends when the bridge lets go of it.
///
/// The break this catches: losing the child's stdin, which ends the session the moment it starts,
/// or keeping it past the bridge, which leaves sessions running that nothing can reach.
#[cfg(unix)]
#[tokio::test]
async fn a_child_runs_while_the_bridge_holds_it() {
    let runtime = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let book = FolderBook::at(runtime.path().join("folders.json"));
    let launcher = Launcher::new(crate::tests::fake_luxe(runtime.path()), Herdr::Off, book);
    let (started, place) = launcher.start(folder.path(), runtime.path()).await.unwrap();
    assert_eq!(place, Place::Bridge);
    assert!(luxe_core::program::process_is_alive(started.pid));

    drop(launcher);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while luxe_core::program::process_is_alive(started.pid) {
        assert!(std::time::Instant::now() < deadline, "the child outlived the bridge");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
