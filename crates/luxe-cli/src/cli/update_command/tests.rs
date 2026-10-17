use super::*;

/// Two files in a temporary directory: the copy the update ran from, and another.
fn copies() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let updated = dir.path().join("updated-luxe");
    let other = dir.path().join("other-luxe");
    for path in [&updated, &other] {
        std::fs::write(path, b"").expect("a stand-in binary");
    }
    (dir, updated, other)
}

/// The installer's updater runs from beside the binary even off `PATH`; anything not there, cargo
/// or brew, is found by name.
#[test]
fn a_program_beside_the_binary_runs_from_there() {
    let dir = tempfile::tempdir().expect("tempdir");
    let updater = dir.path().join(format!("luxe-cli-update{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(&updater, b"").expect("a stand-in updater");
    assert_eq!(program_path("luxe-cli-update", Some(dir.path())), updater);
    assert_eq!(program_path("brew", Some(dir.path())), PathBuf::from("brew"));
    assert_eq!(program_path("luxe-cli-update", None), PathBuf::from("luxe-cli-update"));
}

#[test]
fn the_version_is_the_last_word_when_it_is_semver() {
    assert_eq!(parse_version("luxe 0.2.0\n").as_deref(), Some("0.2.0"));
    assert_eq!(parse_version("luxe v0.2.0").as_deref(), Some("0.2.0"));
    assert_eq!(parse_version("luxe nightly"), None);
    assert_eq!(parse_version(""), None);
}

/// The new version reported, or one past it, is a finished update.
#[test]
fn a_copy_reporting_the_new_version_is_updated() {
    let (_dir, updated, _) = copies();
    assert_eq!(outcome(Some(&updated), &updated, Some("0.2.0"), "0.2.0"), Outcome::Updated);
    assert_eq!(outcome(Some(&updated), &updated, Some("0.2.1"), "0.2.0"), Outcome::Updated);
}

/// The loop Codex CLI's users hit: the update lands in one file and the shell runs another.
#[test]
fn another_copy_first_on_path_is_named() {
    let (_dir, updated, other) = copies();
    let shadowed = outcome(Some(&other), &updated, Some("0.1.0"), "0.2.0");
    assert_eq!(shadowed, Outcome::Shadowed { found: other, updated });
}

/// A symlink to the copy that updated is that copy, as the development install's is, so an old
/// version there means the package was behind, not that a second copy is in the way.
#[cfg(unix)]
#[test]
fn a_link_to_the_updated_copy_is_the_same_copy() {
    let (dir, updated, _) = copies();
    let link = dir.path().join("luxe");
    std::os::unix::fs::symlink(&updated, &link).expect("symlink");
    let behind = outcome(Some(&link), &updated, Some("0.1.0"), "0.2.0");
    assert_eq!(behind, Outcome::NotYet { reported: "0.1.0".into() });
}

#[test]
fn no_luxe_on_path_or_no_version_cannot_be_judged() {
    let (_dir, updated, _) = copies();
    assert_eq!(outcome(None, &updated, None, "0.2.0"), Outcome::Unknown);
    assert_eq!(outcome(Some(&updated), &updated, None, "0.2.0"), Outcome::Unknown);
}

/// Only the reader's own command is ever named, and a check offers `luxe update` beside it.
#[test]
fn the_instructions_name_the_owners_command_or_none() {
    let check = instructions(InstallSource::Cargo, true, false);
    assert!(check.contains("luxe update") && check.contains("cargo install"), "{check}");
    let download = instructions(InstallSource::Archive, false, false);
    assert!(download.contains("download it from"), "{download}");
    let windows = instructions(InstallSource::Cargo, false, true);
    assert!(
        windows.contains("once it has exited") && windows.contains("cargo install"),
        "{windows}"
    );
}
