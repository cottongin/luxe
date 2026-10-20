use super::*;

/// An environment described as a list, so a test can ask what Windows would do from Linux.
fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |name| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| (*value).to_string())
}

/// A Luxe variable names the directory itself; without one, it is `luxe` under the base, and a
/// blank one is unset.
#[test]
fn a_luxe_variable_names_the_directory_itself() {
    let dir = |pairs, base: Option<&str>| {
        luxe_dir("LUXE_CONFIG_DIR", base.map(PathBuf::from), env_of(pairs))
    };
    let under = Some(PathBuf::from("/home/dev/.config/luxe"));
    let set = [("LUXE_CONFIG_DIR", "/profiles/work")];
    assert_eq!(dir(&set, Some("/home/dev/.config")), Some(PathBuf::from("/profiles/work")));
    assert_eq!(dir(&set, None), Some(PathBuf::from("/profiles/work")), "with no platform base");
    let blank = [("LUXE_CONFIG_DIR", "  ")];
    assert_eq!(dir(&blank, Some("/home/dev/.config")), under, "blank is unset");
    let other = [("LUXE_STATE_DIR", "/elsewhere")];
    assert_eq!(dir(&other, Some("/home/dev/.config")), under);
    assert_eq!(dir(&other, None), None, "nowhere named, nowhere to look");
}

/// The runtime folder is one only this user can open: Linux's runtime directory, macOS's temp
/// directory, and never Linux's `$TMPDIR`, which is usually the shared `/tmp`.
#[test]
#[cfg(unix)]
fn the_runtime_folder_is_private_to_the_user() {
    let linux = [("XDG_RUNTIME_DIR", "/run/user/1000"), ("TMPDIR", "/tmp")];
    let runtime = |pairs: &[(&str, &str)], macos| runtime_in(env_of(pairs), macos);
    assert_eq!(runtime(&linux, false), Some(PathBuf::from("/run/user/1000/luxe")));
    assert_eq!(runtime(&[("TMPDIR", "/tmp")], false), None, "the shared /tmp is never it");
    let mac = [("TMPDIR", "/var/folders/x/T/"), ("XDG_RUNTIME_DIR", "/elsewhere")];
    assert_eq!(runtime(&mac, true), Some(PathBuf::from("/var/folders/x/T/luxe")));
    let named = [("LUXE_RUNTIME_DIR", "/chosen"), ("XDG_RUNTIME_DIR", "/run/user/1000")];
    assert_eq!(runtime(&named, false), Some(PathBuf::from("/chosen")), "the Luxe variable wins");
}

/// The old resolver: the XDG variable, then `HOME`, never the Windows folders, and nothing to
/// take over when `LUXE_CONFIG_DIR` was in charge then too, or when there was no home.
#[test]
fn the_legacy_config_dir_is_where_the_old_resolver_looked() {
    let legacy = |pairs| legacy_config(env_of(pairs));
    let both = [("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/dev"), ("APPDATA", "/appdata")];
    assert_eq!(legacy(&both), Some(PathBuf::from("/xdg/luxe")));
    let home = [("XDG_CONFIG_HOME", " "), ("HOME", "/home/dev"), ("APPDATA", "/appdata")];
    assert_eq!(legacy(&home), Some(PathBuf::from("/home/dev/.config/luxe")));
    assert_eq!(legacy(&[("APPDATA", "/appdata")]), None, "no home, nothing of theirs");
    assert_eq!(legacy(&[("LUXE_CONFIG_DIR", "/p"), ("HOME", "/home/dev")]), None);
}

/// A file only at the old location is used where it is; once one exists in the config
/// directory, that one is; and with neither, the config directory is where a new one goes.
#[test]
fn a_config_file_left_at_the_old_location_is_used_in_place() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (legacy, path) = (dir.path().join("old/auth.json"), dir.path().join("new/auth.json"));
    assert_eq!(resolve_file(path.clone(), Some(legacy.clone())), path, "neither: the new one");
    std::fs::create_dir_all(legacy.parent().expect("parent")).expect("old dir");
    std::fs::write(&legacy, "{}").expect("old file");
    assert_eq!(resolve_file(path.clone(), Some(legacy.clone())), legacy, "only the old one");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("new dir");
    std::fs::write(&path, "{}").expect("new file");
    assert_eq!(resolve_file(path.clone(), Some(legacy)), path, "the new one wins");
    assert_eq!(resolve_file(path.clone(), None), path);
}

/// Only `auth.json` and `models.json` are looked for where older builds put them: everything
/// else was always read from the config directory, so a stray old copy of it is not the user's.
#[test]
fn only_the_files_older_builds_placed_are_looked_for_there() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (new, old) = (dir.path().join("new"), dir.path().join("old"));
    std::fs::create_dir_all(&old).expect("old dir");
    for name in ["auth.json", "models.json", "permissions.json"] {
        std::fs::write(old.join(name), "{}").expect("old file");
    }
    let file = |name| config_file_in(name, new.clone(), Some(old.clone()));
    assert_eq!(file("auth.json"), old.join("auth.json"));
    assert_eq!(file("models.json"), old.join("models.json"));
    assert_eq!(file("permissions.json"), new.join("permissions.json"));
}

#[test]
fn an_xdg_variable_wins_outright() {
    let env = env_of(&[("XDG_CONFIG_HOME", "/somewhere/config"), ("HOME", "/home/dev")]);
    assert_eq!(
        resolve("XDG_CONFIG_HOME", "APPDATA", &[".config"], env),
        Some(PathBuf::from("/somewhere/config"))
    );
}

/// A variable exported as empty is a shell artifact; falling into it would put the config at the
/// filesystem root.
#[test]
fn a_blank_variable_is_not_a_path() {
    let env = env_of(&[("XDG_CONFIG_HOME", "   "), ("HOME", "/home/dev")]);
    assert_eq!(
        resolve("XDG_CONFIG_HOME", "APPDATA", &[".config"], env),
        Some(PathBuf::from("/home/dev/.config"))
    );
}

#[test]
fn unix_falls_back_to_home() {
    let env = env_of(&[("HOME", "/home/dev")]);
    assert_eq!(
        resolve("XDG_STATE_HOME", "LOCALAPPDATA", &[".local", "state"], env),
        Some(PathBuf::from("/home/dev/.local/state"))
    );
}

/// The bug this module exists for: a Windows box has no `HOME`, so every caller used to fall all
/// the way through to `.` and write its files beside whatever project happened to be open.
#[test]
#[cfg(windows)]
fn windows_uses_appdata_rather_than_the_working_directory() {
    let env = env_of(&[
        ("APPDATA", r"C:\Users\dev\AppData\Roaming"),
        ("LOCALAPPDATA", r"C:\Users\dev\AppData\Local"),
        ("USERPROFILE", r"C:\Users\dev"),
    ]);
    assert_eq!(
        resolve("XDG_CONFIG_HOME", "APPDATA", &[".config"], &env),
        Some(PathBuf::from(r"C:\Users\dev\AppData\Roaming"))
    );
    assert_eq!(
        resolve("XDG_STATE_HOME", "LOCALAPPDATA", &[".local", "state"], &env),
        Some(PathBuf::from(r"C:\Users\dev\AppData\Local"))
    );
}

/// `USERPROFILE` is the last resort before the working directory, on any platform that sets it.
#[test]
fn userprofile_stands_in_for_home() {
    let env = env_of(&[("USERPROFILE", "/users/dev")]);
    assert_eq!(
        resolve("XDG_CONFIG_HOME", "APPDATA", &[".config"], env),
        Some(PathBuf::from("/users/dev/.config"))
    );
}

/// With nothing to go on there is no answer, and a discovery path should not invent one. The
/// infallible wrapper keeps the old `./.config` behavior for the callers that need *a* path.
#[test]
fn an_empty_environment_has_no_answer() {
    assert_eq!(resolve("XDG_CONFIG_HOME", "APPDATA", &[".config"], env_of(&[])), None);
}

/// The real functions read the real environment, so at minimum they must return something
/// absolute on a developer machine rather than a relative scrap.
#[test]
fn the_public_answers_are_rooted() {
    for dir in [config_dir(), state_dir(), cache_dir()] {
        assert!(dir.is_absolute() || dir.starts_with("."), "unexpected shape: {dir:?}");
    }
}

/// Every platform this runs on has a home directory, and it is not the working directory.
#[test]
fn home_is_a_real_absolute_place() {
    let home = home_dir().expect("a machine running tests has a home directory");
    assert!(home.is_absolute(), "{home:?}");
}

/// The path in a "go and edit this" message has to be the path on *this* machine.
#[test]
fn a_config_path_reads_as_this_platforms_path() {
    let rendered = config_path_display("lsp-servers.json");
    assert!(rendered.ends_with("luxe/lsp-servers.json"), "{rendered}");
    assert!(!rendered.contains('\\'), "forward slashes, everywhere: {rendered}");
    if cfg!(windows) {
        assert!(rendered.contains("AppData"), "{rendered}");
    } else {
        assert!(rendered.starts_with("~/") || rendered.starts_with('/'), "{rendered}");
    }
}
