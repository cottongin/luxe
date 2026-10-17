use super::*;
use serde_json::json;

/// A server naming a file outside the project must not get it written, and nothing else either.
#[tokio::test]
async fn an_edit_naming_a_file_outside_the_project_writes_nothing() {
    let project = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let inside = project.path().join("a.rs");
    let hosts = elsewhere.path().join("hosts");
    std::fs::write(&inside, "let a = 1;\n").unwrap();
    std::fs::write(&hosts, "127.0.0.1 localhost\n").unwrap();
    let edit = changes(&[
        (&inside, vec![edit_at(0, 4, 0, 5, "b")]),
        (&hosts, vec![edit_at(0, 0, 0, 0, "6.6.6.6 bank\n")]),
    ]);
    let result = apply(&edit, project.path(), None).await;
    assert!(result.is_error, "{result:?}");
    assert!(text_of(&result).contains("outside the project"), "{}", text_of(&result));
    assert_eq!(std::fs::read_to_string(&inside).unwrap(), "let a = 1;\n");
    assert_eq!(std::fs::read_to_string(&hosts).unwrap(), "127.0.0.1 localhost\n");
}

/// A fetched repository is a stranger's code inside the project, and no edit lands there.
#[tokio::test]
async fn an_edit_inside_a_fetched_repository_writes_nothing() {
    let project = tempfile::tempdir().unwrap();
    let clone = luxe_core::fetched::cache_dir(project.path()).join("someone").join("repo");
    std::fs::create_dir_all(&clone).unwrap();
    let fetched = clone.join("lib.rs");
    std::fs::write(&fetched, "fn a() {}\n").unwrap();
    let edit = changes(&[(&fetched, vec![edit_at(0, 3, 0, 4, "b")])]);
    let result = apply(&edit, project.path(), None).await;
    assert!(result.is_error, "{result:?}");
    assert_eq!(std::fs::read_to_string(&fetched).unwrap(), "fn a() {}\n");
}

/// Every file is read and planned before any is written, so a bad second file leaves the first.
#[tokio::test]
async fn an_unreadable_second_file_leaves_the_first_untouched() {
    let project = tempfile::tempdir().unwrap();
    let first = project.path().join("a.rs");
    std::fs::write(&first, "let a = 1;\n").unwrap();
    let missing = project.path().join("missing.rs");
    let edit = changes(&[
        (&first, vec![edit_at(0, 4, 0, 5, "b")]),
        (&missing, vec![edit_at(0, 0, 0, 0, "x")]),
    ]);
    let result = apply(&edit, project.path(), None).await;
    assert!(result.is_error, "{result:?}");
    assert_eq!(std::fs::read_to_string(&first).unwrap(), "let a = 1;\n");
}

/// A write that fails partway puts back what was already written.
#[cfg(unix)]
#[tokio::test]
async fn a_failed_write_restores_the_files_already_written() {
    use std::os::unix::fs::PermissionsExt;
    let project = tempfile::tempdir().unwrap();
    let first = project.path().join("a.rs");
    let locked = project.path().join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    let second = locked.join("b.rs");
    std::fs::write(&first, "let a = 1;\n").unwrap();
    std::fs::write(&second, "let b = 1;\n").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(locked.join("probe"), "").is_ok() {
        return; // running as root: a read-only directory cannot make the write fail
    }
    let edit = changes(&[
        (&first, vec![edit_at(0, 4, 0, 5, "x")]),
        (&second, vec![edit_at(0, 4, 0, 5, "y")]),
    ]);
    let result = apply(&edit, project.path(), None).await;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.is_error, "{result:?}");
    assert_eq!(std::fs::read_to_string(&first).unwrap(), "let a = 1;\n");
    assert_eq!(std::fs::read_to_string(&second).unwrap(), "let b = 1;\n");
}

/// An edit past the end of its file is reported as skipped, not counted, and not appended.
#[tokio::test]
async fn an_out_of_range_edit_is_reported_as_skipped() {
    let project = tempfile::tempdir().unwrap();
    let file = project.path().join("a.rs");
    std::fs::write(&file, "let a = 1;\n").unwrap();
    let edit = changes(&[(&file, vec![edit_at(0, 4, 0, 5, "b"), edit_at(40, 0, 40, 3, "zzz")])]);
    let result = apply(&edit, project.path(), None).await;
    assert!(!result.is_error, "{result:?}");
    let text = text_of(&result);
    assert!(text.contains("Applied 1 edit(s)"), "{text}");
    assert!(text.contains("skipped 1 edit(s)"), "{text}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "let b = 1;\n");
}

/// Creating, renaming or deleting a file is not something this applies, and the result says so.
#[tokio::test]
async fn resource_operations_are_reported_not_dropped() {
    let project = tempfile::tempdir().unwrap();
    let file = project.path().join("a.rs");
    std::fs::write(&file, "let a = 1;\n").unwrap();
    let mut edit = changes(&[(&file, vec![edit_at(0, 4, 0, 5, "b")])]);
    let created = crate::uri::from_path(&project.path().join("new.rs"));
    edit["documentChanges"].as_array_mut().unwrap().push(json!({"kind": "create", "uri": created}));
    let text = text_of(&apply(&edit, project.path(), None).await);
    assert!(text.contains("not applied") && text.contains("create"), "{text}");
}

/// `documentChanges` wins over `changes` when a server sends both, so an edit lands once.
#[tokio::test]
async fn changes_and_document_changes_together_apply_once() {
    let project = tempfile::tempdir().unwrap();
    let file = project.path().join("a.rs");
    std::fs::write(&file, "a\n").unwrap();
    let mut edit = changes(&[(&file, vec![edit_at(0, 0, 0, 0, "x")])]);
    edit["changes"] = json!({ crate::uri::from_path(&file): [edit_at(0, 0, 0, 0, "x")] });
    apply(&edit, project.path(), None).await;
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "xa\n");
}

/// A file an applied edit rewrote is current for the read guard, so the next `edit` is not refused.
#[tokio::test]
async fn applied_files_are_recorded_for_the_read_guard() {
    let project = tempfile::tempdir().unwrap();
    let file = project.path().join("a.rs");
    std::fs::write(&file, "let a = 1;\n").unwrap();
    let tracker = ReadTracker::new();
    let edit = changes(&[(&file, vec![edit_at(0, 4, 0, 5, "bee")])]);
    apply(&edit, project.path(), Some(&tracker)).await;
    assert_eq!(tracker.status("t", &file), luxe_core::read_tracker::ReadStatus::Current);
}

/// Inserts and a replace sharing one start all land, in the order the server sent them.
#[tokio::test]
async fn edits_sharing_a_start_apply_in_the_order_sent() {
    let project = tempfile::tempdir().unwrap();
    let file = project.path().join("a.rs");
    std::fs::write(&file, "let a = 1;\n").unwrap();
    let edit = changes(&[(
        &file,
        vec![edit_at(0, 4, 0, 4, "x"), edit_at(0, 4, 0, 4, "y"), edit_at(0, 4, 0, 5, "b")],
    )]);
    let text = text_of(&apply(&edit, project.path(), None).await);
    assert!(text.contains("Applied 3 edit(s)"), "{text}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "let xyb = 1;\n");
}

/// Two entries for one file are planned together, or the second is placed on the unedited text
/// and its write undoes the first.
#[tokio::test]
async fn two_entries_for_one_file_both_land() {
    let project = tempfile::tempdir().unwrap();
    let file = project.path().join("a.rs");
    std::fs::write(&file, "let a = b;\n").unwrap();
    let edit = changes(&[
        (&file, vec![edit_at(0, 4, 0, 5, "x")]),
        (&file, vec![edit_at(0, 8, 0, 9, "y")]),
    ]);
    apply(&edit, project.path(), None).await;
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "let x = y;\n");
}

/// A link inside the project that points out of it is judged by where it points.
#[cfg(unix)]
#[tokio::test]
async fn a_symlink_out_of_the_project_is_refused() {
    let project = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let target = elsewhere.path().join("secret.rs");
    std::fs::write(&target, "let a = 1;\n").unwrap();
    let link = project.path().join("link.rs");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let result =
        apply(&changes(&[(&link, vec![edit_at(0, 4, 0, 5, "b")])]), project.path(), None).await;
    assert!(result.is_error, "{result:?}");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "let a = 1;\n");
}
