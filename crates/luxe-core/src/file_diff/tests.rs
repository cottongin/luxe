use super::*;

/// A scratch directory that already exists is someone else's, whatever it holds: it is never
/// written into, so a link planted there cannot redirect the write.
#[cfg(unix)]
#[test]
fn a_prepared_scratch_tree_is_never_reused() {
    let shared = tempfile::tempdir().unwrap();
    let victim = shared.path().join("victim.txt");
    std::fs::write(&victim, "untouched").unwrap();
    let planted = shared.path().join("luxe-peek-planted");
    std::fs::create_dir_all(planted.join("a")).unwrap();
    std::os::unix::fs::symlink(&victim, planted.join("a").join("notes.md")).unwrap();
    let result = ScratchPair::holding_at(planted, Path::new("notes.md"), b"before", b"after");
    assert!(result.is_err());
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched");
}

/// Both versions may be private, so the directory and the files are its owner's alone, and the
/// whole tree goes when the diff is done.
#[cfg(unix)]
#[test]
fn a_scratch_pair_is_private_and_removed() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    let pair = ScratchPair::holding(Path::new("notes.md"), b"before", b"after").unwrap();
    assert_eq!(mode(&pair.directory), 0o700);
    assert_eq!(mode(&pair.directory.join(&pair.before)), 0o600);
    assert_eq!(mode(&pair.directory.join(&pair.after)), 0o600);
    let directory = pair.directory.clone();
    drop(pair);
    assert!(!directory.exists());
}

#[test]
fn a_path_that_escapes_the_project_is_refused() {
    assert!(!is_safe_relative_path(Path::new("../secrets")));
    assert!(!is_safe_relative_path(Path::new("")));
    assert!(!is_safe_relative_path(Path::new("/etc/passwd")));
    assert!(is_safe_relative_path(Path::new("src/lib.rs")));
}

#[test]
fn a_request_id_is_distinct_and_labels_itself() {
    let id = FileDiffRequestId::new();
    assert!(id.as_str().starts_with("file-diff:"));
    assert_ne!(id, FileDiffRequestId::new());
}

/// A deletion has no new path, so matching only the new side would fall through to whatever
/// git happened to list first.
#[test]
fn the_record_is_matched_on_either_side_of_a_rename() {
    let diff = concat!(
        "diff --git a/old.rs b/new.rs\n",
        "similarity index 60%\n",
        "rename from old.rs\n",
        "rename to new.rs\n",
        "--- a/old.rs\n",
        "+++ b/new.rs\n",
        "@@ -1,2 +1,2 @@\n",
        " keep\n",
        "-was\n",
        "+is\n",
    );
    let by_new = parse_one(diff, "new.rs").unwrap().expect("the renamed file");
    let by_old = parse_one(diff, "old.rs").unwrap().expect("the same file, named before");
    assert_eq!(by_new, by_old);
    assert_eq!(by_new.new_path.as_deref(), Some(Path::new("new.rs")));
}

#[test]
fn a_diff_the_parser_rejects_is_reported_rather_than_swallowed() {
    let error = parse_one("this is not a diff\n", "src/lib.rs").unwrap_err();
    assert!(matches!(error, FileDiffFailure::Unreadable(_)), "{error:?}");
}

/// Every failure is something a person can read without a stack trace beside it.
#[test]
fn each_failure_says_what_to_do_about_it() {
    for failure in [
        FileDiffFailure::UnsafePath,
        FileDiffFailure::NotAWorktree,
        FileDiffFailure::TooLarge,
        FileDiffFailure::NotText,
        FileDiffFailure::Git("index.lock exists".into()),
        FileDiffFailure::Unreadable("invalid hunk header".into()),
    ] {
        let message = failure.to_string();
        assert!(!message.is_empty());
        assert!(
            message.chars().next().is_some_and(|first| !first.is_uppercase()),
            "messages are lowercase: {message:?}"
        );
    }
}

/// A peek given up on takes its git, and what git started, with it.
///
/// The break this catches: the peek's git had `kill_on_drop` and no process group, so a
/// dropped capture killed git and left its clean filter running.
#[cfg(unix)]
#[tokio::test]
async fn a_dropped_peek_leaves_no_child() {
    let project = tempfile::tempdir().unwrap();
    let pid_file = project.path().join("filter.pid");
    let git = |arguments: &[&str]| {
        let status = std::process::Command::new("git")
            .args(arguments)
            .current_dir(project.path())
            .output()
            .unwrap();
        assert!(status.status.success(), "git {arguments:?}");
    };
    git(&["init", "-q"]);
    std::fs::write(project.path().join("keep.txt"), "before\n").unwrap();
    git(&["add", "keep.txt"]);
    git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init"]);
    let filter = format!("sh -c 'echo $$ > {}; exec sleep 30'", pid_file.display());
    git(&["config", "filter.slow.clean", &filter]);
    std::fs::write(project.path().join(".gitattributes"), "keep.txt filter=slow\n").unwrap();
    std::fs::write(project.path().join("keep.txt"), "after\n").unwrap();

    let root = project.path().to_path_buf();
    let peek = tokio::spawn(async move { capture(&root, "keep.txt").await });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let pid = loop {
        let written = std::fs::read_to_string(&pid_file).unwrap_or_default();
        if let Ok(pid) = written.trim().parse::<u32>() {
            break pid;
        }
        assert!(std::time::Instant::now() < deadline, "the filter never started");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };

    peek.abort();
    let _ = peek.await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while crate::program::tests::still_running(pid) && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(!crate::program::tests::still_running(pid), "the filter {pid} outlived the peek");
}

#[test]
fn stderr_is_reduced_to_its_first_meaningful_line() {
    assert_eq!(stderr_summary(b"\n\nfatal: bad revision\nmore\n"), "fatal: bad revision");
    assert_eq!(stderr_summary(b""), "no diagnostic output");
    assert_eq!(stderr_summary(&vec![b'x'; 500]).chars().count(), 200);
}
