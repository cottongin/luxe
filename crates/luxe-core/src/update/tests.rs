use super::*;

#[test]
fn a_cargo_install_is_told_to_use_cargo() {
    for path in [
        "/home/dev/.cargo/bin/luxe",
        "C:/Users/dev/.cargo/bin/luxe.exe",
        "/home/dev/.rustup/toolchains/stable/bin/luxe",
    ] {
        assert_eq!(InstallSource::from_path(Path::new(path)), InstallSource::Cargo, "{path}");
    }
    assert_eq!(InstallSource::Cargo.update_command(), Some("cargo install --force luxe-cli"));
}

/// Homebrew is checked first because its Linux and Intel-macOS prefixes sit *inside* paths the
/// other arms would otherwise claim.
#[test]
fn a_homebrew_install_is_not_mistaken_for_a_system_package() {
    for path in [
        "/opt/homebrew/bin/luxe",
        "/usr/local/Cellar/luxe/0.1.0/bin/luxe",
        "/home/linuxbrew/.linuxbrew/bin/luxe",
    ] {
        assert_eq!(InstallSource::from_path(Path::new(path)), InstallSource::Homebrew, "{path}");
    }
}

/// `/usr/local/bin` is where you put things a package manager did *not* install, so advising a
/// package manager there would be wrong.
#[test]
fn only_package_manager_prefixes_count_as_a_system_package() {
    assert_eq!(InstallSource::from_path(Path::new("/usr/bin/luxe")), InstallSource::SystemPackage);
    assert_eq!(InstallSource::from_path(Path::new("/usr/local/bin/luxe")), InstallSource::Archive);
    assert_eq!(
        InstallSource::from_path(Path::new("/home/dev/luxe/target/release/luxe")),
        InstallSource::Archive
    );
}

/// Every source says something actionable, and none of them name a command they cannot know is
/// right.
#[test]
fn every_source_gives_advice_and_none_of_it_is_a_guess() {
    for source in [
        InstallSource::Cargo,
        InstallSource::Homebrew,
        InstallSource::SystemPackage,
        InstallSource::Archive,
    ] {
        assert!(!source.advice().is_empty(), "{source:?}");
    }
    assert!(
        InstallSource::SystemPackage.update_command().is_none(),
        "naming apt on an Arch box is the failure this exists to avoid"
    );
}

/// The two orderings a string compare gets wrong, and which is the whole reason `semver` is a
/// dependency here.
#[test]
fn version_comparison_follows_semver_rather_than_the_alphabet() {
    assert!(is_newer("0.10.0", "0.9.0"), "0.10 is newer than 0.9 despite sorting before it");
    assert!(!is_newer("0.9.0", "0.10.0"));
    assert!(is_newer("0.2.0", "0.2.0-rc.1"), "a release beats its own pre-release");
    assert!(!is_newer("0.2.0-rc.1", "0.2.0"));
}

#[test]
fn a_release_tag_may_carry_a_v() {
    assert!(is_newer("v0.2.0", "0.1.0"));
    assert_eq!(normalize_version(" v1.2.3 "), "1.2.3");
}

#[test]
fn the_same_version_is_not_an_update() {
    assert!(!is_newer("0.1.0", "0.1.0"));
}

/// Garbage on either side must produce silence, not a notice about a version nobody can parse.
#[test]
fn an_unparseable_version_says_nothing() {
    assert!(!is_newer("not-a-version", "0.1.0"));
    assert!(!is_newer("0.2.0", "also-not-a-version"));
    assert!(!is_newer("", "0.1.0"));
}

#[test]
fn a_notice_names_both_versions_and_the_command() {
    let state = UpdateState { latest: Some("v0.2.0".into()), checked_at: 0 };
    let notice = notice(&state, "0.1.0", InstallSource::Cargo).expect("behind by a minor");
    let line = notice.line();
    assert!(line.contains("0.2.0"), "{line}");
    assert!(line.contains("you have 0.1.0"), "{line}");
    assert!(line.contains("cargo install --force luxe-cli"), "{line}");
}

#[test]
fn there_is_no_notice_when_current_or_unchecked() {
    let fresh = UpdateState { latest: Some("0.1.0".into()), checked_at: 0 };
    assert_eq!(notice(&fresh, "0.1.0", InstallSource::Cargo), None, "not behind");
    assert_eq!(
        notice(&UpdateState::default(), "0.1.0", InstallSource::Cargo),
        None,
        "never checked"
    );
}

#[test]
fn a_never_checked_state_is_stale_and_a_just_checked_one_is_not() {
    assert!(UpdateState::default().is_stale(CHECK_TTL));
    let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_secs();
    assert!(!UpdateState { latest: None, checked_at: now }.is_stale(CHECK_TTL));
}

/// A clock that jumped backwards must not make every launch re-check.
#[test]
fn a_future_timestamp_reads_as_fresh() {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_secs();
    assert!(!UpdateState { latest: None, checked_at: now + 86_400 }.is_stale(CHECK_TTL));
}

#[test]
fn state_round_trips_through_a_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("nested").join("update.json");
    let state = UpdateState { latest: Some("0.3.0".into()), checked_at: 42 };
    state.store(&path).expect("stores, creating the directory");
    let loaded = UpdateState::load(&path);
    assert_eq!(loaded.latest.as_deref(), Some("0.3.0"));
    assert_eq!(loaded.checked_at, 42);
}

/// Losing the cache costs one request, so a corrupt file is a default rather than an error.
#[test]
fn a_corrupt_cache_reads_as_never_checked() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("update.json");
    std::fs::write(&path, b"{not json").expect("write");
    assert!(UpdateState::load(&path).latest.is_none());
}
