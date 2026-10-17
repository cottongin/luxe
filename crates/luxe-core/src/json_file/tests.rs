use super::*;

/// A broken file is moved aside under the lock, and a later broken file does not overwrite the
/// first one set aside.
#[test]
fn a_broken_file_is_set_aside_and_never_over_an_earlier_one() {
    let (dir, file) = file(FileMode::OwnerOnly);
    for broken in ["{ first", "{ second"] {
        std::fs::write(file.path(), broken).unwrap();
        let ((), aside) = file
            .modify_setting_aside(|root| {
                root.insert("fresh".into(), Value::Bool(true));
                Ok(())
            })
            .unwrap();
        let aside = aside.expect("the broken file was moved");
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), broken);
    }
    assert_eq!(std::fs::read_to_string(dir.path().join("thing.json.bad")).unwrap(), "{ first");
    assert_eq!(file.read().unwrap().get("fresh"), Some(&Value::Bool(true)));
}

/// Already-exists always waits; access denied waits only on Windows, where it is the previous
/// holder's directory still being removed; anything else fails at once.
#[test]
fn a_lock_waits_out_only_what_waiting_can_fix() {
    use std::io::ErrorKind::{AlreadyExists, NotFound, PermissionDenied};
    assert_eq!(lock_wait(AlreadyExists, false), LockWait::Wait, "another writer has it");
    assert_eq!(lock_wait(PermissionDenied, true), LockWait::Wait, "Windows letting it go");
    assert_eq!(lock_wait(PermissionDenied, false), LockWait::Fail, "elsewhere it is real");
    assert_eq!(lock_wait(NotFound, true), LockWait::Fail, "no directory to lock in");
}

/// Elsewhere than Windows, a lock that cannot be created for want of permission fails at once
/// with the reason, rather than waiting out the deadline and calling it a timeout.
#[cfg(unix)]
#[test]
fn a_lock_without_permission_fails_with_the_reason() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
    let started = std::time::Instant::now();
    let error = FileLock::acquire(&dir.path().join("file.json")).err().expect("refused");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(error, FileError::Io { .. }), "{error}");
    assert!(started.elapsed() < LOCK_TIMEOUT, "it did not wait for the deadline");
}

/// The OS error is said once, whether a caller prints the message alone or walks the chain.
#[test]
fn an_io_failure_names_its_cause_once() {
    let error = FileError::io("reading thing.json", std::io::Error::other("disk on fire"));
    assert_eq!(error.to_string(), "reading thing.json: disk on fire");
    let chain = crate::error_cause::chain(&error);
    assert_eq!(chain.matches("disk on fire").count(), 1, "{chain}");
}

fn file(mode: FileMode) -> (tempfile::TempDir, JsonFile) {
    let dir = tempfile::tempdir().unwrap();
    let file = JsonFile::new(dir.path().join("thing.json"), mode);
    (dir, file)
}

#[test]
fn an_absent_or_empty_file_reads_as_nothing() {
    let (_dir, file) = file(FileMode::OwnerOnly);
    assert!(file.read().is_none());
    std::fs::write(file.path(), "  \n").unwrap();
    assert!(file.read().is_none());
}

#[test]
fn modify_preserves_every_key_it_did_not_touch() {
    let (_dir, file) = file(FileMode::OwnerOnly);
    std::fs::write(file.path(), r#"{"keep": {"deep": 1}, "also": "me"}"#).unwrap();
    file.modify(|root| {
        root.insert("added".into(), Value::Bool(true));
        Ok(())
    })
    .unwrap();
    let root = file.read().unwrap();
    assert_eq!(root["keep"]["deep"], 1);
    assert_eq!(root["also"], "me");
    assert_eq!(root["added"], true);
}

#[test]
fn a_malformed_file_blocks_the_write_and_is_left_alone() {
    let (_dir, file) = file(FileMode::OwnerOnly);
    std::fs::write(file.path(), "{ nope").unwrap();
    let error = file
        .modify(|root| {
            root.insert("added".into(), Value::Bool(true));
            Ok(())
        })
        .expect_err("must refuse");
    assert!(matches!(error, FileError::Unparseable { .. }), "got {error:?}");
    assert_eq!(std::fs::read_to_string(file.path()).unwrap(), "{ nope");
}

#[test]
fn a_rejecting_edit_leaves_the_file_untouched() {
    let (_dir, file) = file(FileMode::OwnerOnly);
    std::fs::write(file.path(), r#"{"a": 1}"#).unwrap();
    let error =
        file.modify(|_| Err::<(), _>(FileError::Rejected("no".into()))).expect_err("propagates");
    assert!(matches!(error, FileError::Rejected(_)));
    assert_eq!(file.read().unwrap()["a"], 1);
    assert!(!file.path().with_extension("lock").exists(), "the lock must be released");
}

#[cfg(unix)]
#[test]
fn owner_only_mode_is_enforced_and_preserve_mode_respects_the_user() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, secret) = file(FileMode::OwnerOnly);
    secret.modify(|root| Ok(root.insert("k".into(), Value::Bool(true)))).unwrap();
    assert_eq!(crate::fs_atomic::existing_mode(secret.path()), Some(0o600));

    let dir = tempfile::tempdir().unwrap();
    let config = JsonFile::new(dir.path().join("models.json"), FileMode::PreserveExisting);
    // A fresh file still starts tight.
    config.modify(|root| Ok(root.insert("a".into(), Value::Bool(true)))).unwrap();
    assert_eq!(crate::fs_atomic::existing_mode(config.path()), Some(0o600));
    // A mode the user chose survives a rewrite.
    std::fs::set_permissions(config.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
    config.modify(|root| Ok(root.insert("b".into(), Value::Bool(true)))).unwrap();
    assert_eq!(crate::fs_atomic::existing_mode(config.path()), Some(0o644));
}

#[test]
fn a_stale_lock_is_broken_instead_of_blocking_forever() {
    let (_dir, file) = file(FileMode::OwnerOnly);
    let lock = file.path().with_extension("lock");
    std::fs::create_dir(&lock).unwrap();
    backdate(&lock, LOCK_STALE + Duration::from_secs(5));
    file.modify(|root| Ok(root.insert("a".into(), Value::Bool(true)))).unwrap();
    assert!(file.read().is_some());
}

#[test]
fn a_fresh_lock_makes_a_writer_time_out_rather_than_corrupt() {
    let (_dir, file) = file(FileMode::OwnerOnly);
    let lock = file.path().with_extension("lock");
    std::fs::create_dir(&lock).unwrap();
    let error = file.modify(|_| Ok(())).expect_err("must not write through a live lock");
    assert!(matches!(error, FileError::LockTimeout(_)), "got {error:?}");
    std::fs::remove_dir(&lock).unwrap();
}

/// Set an mtime in the past.
///
/// Not `touch`: `touch -d @<epoch>` is a GNU extension BSD rejects. Not `std` either, because
/// the lock is a *directory*, and opening one to set its times needs `FILE_FLAG_BACKUP_SEMANTICS`
/// on Windows, so it has to work there or the test it serves asserts that nothing happens.
fn backdate(path: &Path, by: Duration) {
    let when = filetime::FileTime::from_system_time(SystemTime::now() - by);
    filetime::set_file_mtime(path, when).expect("set mtime");
}
