use super::*;

#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Editing a script keeps it a script. The rename puts a new file where the old one was, so a
/// mode not carried over is lost: every edit of an executable left it unrunnable.
#[cfg(unix)]
#[test]
fn a_replace_keeping_mode_leaves_a_script_executable() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("run.sh");
    fs::write(&script, b"#!/bin/sh\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    replace_keeping_mode(&script, b"#!/bin/sh\necho hi\n").unwrap();
    assert_eq!(mode_of(&script), 0o755);
    assert_eq!(fs::read(&script).unwrap(), b"#!/bin/sh\necho hi\n");
}

/// The mode kept is the mode the file had, bit for bit, not one the umask trimmed on the way.
#[cfg(unix)]
#[test]
fn a_kept_mode_is_exact_whatever_the_umask() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let shared = dir.path().join("shared");
    fs::write(&shared, b"a").unwrap();
    // World-writable on purpose: any umask that is not 000 strips at least one of these bits
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o777)).unwrap();
    replace_keeping_mode(&shared, b"b").unwrap();
    assert_eq!(mode_of(&shared), 0o777);
}

/// A file that did not exist gets the mode any new file would, not the owner-only one secrets get.
#[cfg(unix)]
#[test]
fn a_new_file_kept_mode_gets_the_ordinary_default() {
    let dir = tempfile::tempdir().unwrap();
    let ordinary = dir.path().join("ordinary");
    File::create(&ordinary).unwrap();
    let fresh = dir.path().join("fresh");
    replace_keeping_mode(&fresh, b"new").unwrap();
    assert_eq!(mode_of(&fresh), mode_of(&ordinary));
}

/// An unsynced replace still lands whole and leaves no staging file behind.
#[test]
fn an_unsynced_replace_lands_whole_and_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("cache").join("shard.json");
    replace_unsynced(&target, b"first").unwrap();
    replace_unsynced(&target, b"second").unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"second");
    let left: Vec<_> = fs::read_dir(target.parent().unwrap()).unwrap().flatten().collect();
    assert_eq!(left.len(), 1, "only the shard: {left:?}");
}

/// Every directory `create_private_dir` makes is owner-only, not just the last one; one that was
/// already there keeps its mode.
#[cfg(unix)]
#[test]
fn every_directory_created_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let leaf = dir.path().join("a").join("b").join("c");
    create_private_dir(&leaf).unwrap();
    for created in [dir.path().join("a"), dir.path().join("a/b"), leaf] {
        assert_eq!(mode(&created), 0o700, "{}", created.display());
    }
    assert_eq!(mode(dir.path()), 0o755, "an existing parent is left alone");
}

/// A fresh private directory refuses to adopt one that exists.
#[test]
fn a_new_private_directory_is_never_an_existing_one() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = dir.path().join("fresh");
    create_private_dir_new(&fresh).unwrap();
    let error = create_private_dir_new(&fresh).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
}

#[test]
fn a_replacement_lands_and_leaves_no_staging_file_behind() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    replace(&path, b"first").unwrap();
    replace(&path, b"second").unwrap();

    assert_eq!(fs::read(&path).unwrap(), b"second");
    let strays: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name != "state.json")
        .collect();
    assert!(strays.is_empty(), "left behind: {strays:?}");
}

#[test]
fn missing_parents_are_created() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a").join("b").join("c.json");
    replace(&path, b"{}").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"{}");
}

/// The staging name has to survive a real extension, or `main.rs` and `main.md` stage to the
/// same file and the last writer wins a race nobody knew they were in.
#[test]
fn two_files_sharing_a_stem_do_not_share_a_staging_name() {
    let dir = tempfile::tempdir().unwrap();
    let rust = staging_path(&dir.path().join("main.rs"));
    let markdown = staging_path(&dir.path().join("main.md"));
    assert_ne!(rust, markdown);
    assert_eq!(rust.parent(), Some(dir.path()), "staging stays on the target's filesystem");
}

/// Two writers of the *same* file must not collide on the staging name either — one would
/// truncate the other's half-written bytes and then rename them into place.
#[test]
fn concurrent_writers_of_one_file_get_distinct_staging_names() {
    let path = Path::new("/tmp/luxe/thing.json");
    assert_ne!(staging_path(path), staging_path(path));
}

#[cfg(unix)]
#[test]
fn a_private_replacement_is_owner_only_from_the_moment_it_exists() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    replace_private(&path, b"secret").unwrap();
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);

    let nested = dir.path().join("private");
    create_private_dir(&nested).unwrap();
    assert_eq!(fs::metadata(&nested).unwrap().permissions().mode() & 0o777, 0o700);
}

/// A failed write must not leave a stray staging file, and must not damage what was there.
#[test]
fn a_failure_leaves_the_previous_contents_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kept.json");
    replace(&path, b"original").unwrap();

    // A directory in the target's place: the rename cannot succeed.
    let blocked = dir.path().join("blocked.json");
    fs::create_dir(&blocked).unwrap();
    assert!(replace(&blocked, b"nope").is_err());

    assert_eq!(fs::read(&path).unwrap(), b"original");
    let strays: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(strays.is_empty(), "left behind: {strays:?}");
}
