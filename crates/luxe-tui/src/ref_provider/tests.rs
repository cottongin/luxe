use super::*;
use std::io::Write;

#[test]
fn file_provider_enumerates_files() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.rs");
    let mut file = std::fs::File::create(&file_path).unwrap();
    writeln!(file, "fn main() {{}}").unwrap();

    let provider = FileRefProvider::new(dir.path());
    let candidates = provider.candidates("", dir.path());
    assert!(!candidates.is_empty());
}

#[test]
fn file_provider_filters_by_query() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::File::create(dir.path().join("main.rs")).unwrap();
    std::fs::File::create(dir.path().join("lib.rs")).unwrap();
    std::fs::File::create(dir.path().join("test.rs")).unwrap();

    let provider = FileRefProvider::new(dir.path());
    let candidates = provider.candidates("main", dir.path());
    assert!(!candidates.is_empty());
    assert!(candidates[0].label.contains("main"));
}

fn labels(candidates: &[Candidate]) -> Vec<&str> {
    candidates.iter().map(|candidate| candidate.label.as_str()).collect()
}

fn provider(files: &[&str]) -> FileRefProvider {
    FileRefProvider::with_files(files.iter().map(|file| (*file).to_string()))
}

fn changed(path: &str, status: FileChangeStatus) -> luxe_core::workspace::FileChangeSummary {
    luxe_core::workspace::FileChangeSummary {
        path: path.into(),
        status,
        additions: None,
        deletions: None,
        binary: false,
    }
}

/// With no query: what is being worked on first, in the order Git reported it, then the rest
/// alphabetically — hidden files and deleted ones left out.
#[test]
fn the_unfiltered_list_leads_with_the_working_set_and_skips_hidden_files() {
    let mut files = provider(&[
        ".cargo/mutants.toml",
        ".gitignore",
        "README.md",
        "src/lib.rs",
        "src/main.rs",
        ".github/workflows/ci.yml",
    ]);
    files.set_working(&[
        changed("src/main.rs", FileChangeStatus::Modified),
        changed(".github/workflows/ci.yml", FileChangeStatus::Modified),
        changed("gone.rs", FileChangeStatus::Deleted),
    ]);
    let listed = files.candidates("", Path::new("."));
    assert_eq!(
        labels(&listed),
        ["src/main.rs", ".github/workflows/ci.yml", "README.md", "src/lib.rs"],
        "a hidden file you are working on is still yours"
    );
    assert_eq!(listed[0].status, Some(FileChangeStatus::Modified));
    assert_eq!(listed[2].status, None);
    assert!(listed.iter().all(|c| c.path && c.insert == c.label), "a path, inserted as listed");
}

/// A match in the filename outranks the same letters strung through the folders.
#[test]
fn a_match_in_the_filename_outranks_one_spread_through_the_path() {
    let files = provider(&[
        "crates/domain/src/maintenance.rs",
        "docs/manual/index.md",
        "src/main.rs",
        "web/src/main-entry.ts",
    ]);
    let found = files.candidates("main", Path::new("."));
    assert_eq!(labels(&found)[0], "src/main.rs", "{:?}", labels(&found));
    assert!(
        labels(&found).iter().position(|l| *l == "docs/manual/index.md")
            > labels(&found).iter().position(|l| *l == "web/src/main-entry.ts"),
        "{:?}",
        labels(&found)
    );
    let lit = &found[0].label_matched;
    assert_eq!(lit, &vec![4, 5, 6, 7], "the matched letters, for the row to light");
}

/// Where the matcher scores two paths alike, the one matched in its filename leads — even
/// against the alphabet and at the same length.
#[test]
fn the_filename_bonus_decides_a_level_match() {
    let files = provider(&["main/a.rs", "z/main.rs"]);
    assert_eq!(labels(&files.candidates("main", Path::new("."))), ["z/main.rs", "main/a.rs"]);
}

/// Among matches that reach into the folders, the tighter one leads, even on a longer path.
#[test]
fn a_tight_folder_match_beats_a_scattered_one() {
    let files = provider(&["ta/ub/ix.rs", "crates/luxe-tui/src/x.rs"]);
    assert_eq!(
        labels(&files.candidates("tui", Path::new("."))),
        ["crates/luxe-tui/src/x.rs", "ta/ub/ix.rs"]
    );
}

/// Equal matches go to the shorter path, then the alphabet.
#[test]
fn a_tie_goes_to_the_shorter_path() {
    let files = provider(&["crates/core/lib.rs", "lib.rs", "b/lib.rs", "a/lib.rs"]);
    let found = files.candidates("lib.rs", Path::new("."));
    assert_eq!(labels(&found), ["lib.rs", "a/lib.rs", "b/lib.rs", "crates/core/lib.rs"]);
}

/// A hidden file is found like any other, follows a visible one that matched as well, and
/// stands level once the query starts with a `.`, which is asking for it.
#[test]
fn hidden_files_are_found_but_follow_unless_asked_for() {
    // The same length, and `.` sorts first, so only the demotion keeps the visible one ahead
    let files = provider(&["ucfg/app.toml", ".cfg/app.toml", ".github/workflows/ci.yml"]);
    assert_eq!(labels(&files.candidates("ci.yml", Path::new(".")))[0], ".github/workflows/ci.yml");
    assert_eq!(
        labels(&files.candidates("app.toml", Path::new("."))),
        ["ucfg/app.toml", ".cfg/app.toml"]
    );
    assert_eq!(labels(&files.candidates(".cfg", Path::new(".")))[0], ".cfg/app.toml");
}

/// The walk honors `.gitignore` the way `fd` does, keeps dotfiles, skips the folders never worth
/// listing, and writes forward slashes.
#[test]
fn the_walk_honors_gitignore_and_keeps_dotfiles() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for file in [
        ".gitignore",
        "ignored.txt",
        "build/out.o",
        ".github/workflows/ci.yml",
        "src/main.rs",
        "target/debug/luxe",
        "node_modules/x/index.js",
        ".git/HEAD",
    ] {
        let path = root.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "x").unwrap();
    }
    std::fs::write(root.join(".gitignore"), "ignored.txt\nbuild/\n").unwrap();
    let mut walked = list_files(root);
    walked.sort();
    assert_eq!(walked, [".github/workflows/ci.yml", ".gitignore", "src/main.rs"]);
}

/// A fresh listing replaces the one read at startup, and never a list that was given.
#[test]
fn a_fresh_listing_finds_what_was_created_since() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("first.rs"), "").unwrap();
    let mut files = FileRefProvider::new(dir.path());
    std::fs::write(dir.path().join("second.rs"), "").unwrap();
    assert_eq!(labels(&files.candidates("", dir.path())), ["first.rs"], "read at startup");
    files.set_listing(list_files(dir.path()));
    assert_eq!(labels(&files.candidates("", dir.path())), ["first.rs", "second.rs"]);

    let mut given = provider(&["only.rs"]);
    given.set_listing(list_files(dir.path()));
    assert_eq!(labels(&given.candidates("", dir.path())), ["only.rs"]);
}

#[test]
fn expand_reference_reads_file() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.rs");
    let mut file = std::fs::File::create(&file_path).unwrap();
    writeln!(file, "fn main() {{}}").unwrap();

    let r#ref = crate::mentions::MentionRef {
        kind: crate::mentions::RefKind::File,
        target: "test.rs".to_string(),
        span: 0..8,
    };

    let content = expand_reference(&r#ref, dir.path());
    assert!(content.is_some());
    assert!(content.unwrap().contains("fn main"));
}

#[test]
fn expand_reference_returns_none_for_missing() {
    let r#ref = crate::mentions::MentionRef {
        kind: crate::mentions::RefKind::File,
        target: "nonexistent.rs".to_string(),
        span: 0..14,
    };

    let content = expand_reference(&r#ref, Path::new("/tmp"));
    assert!(content.is_none());
}

/// The Files tab lists what the in-process walk finds, never what a program called `fd` on PATH
/// prints.
///
/// The break this catches: the listing shelled out to `fd` first and trusted its output, so the
/// tab depended on which `fd` a machine had (or which binary answered to the name), unbounded and
/// outside the walk the tools share.
#[cfg(unix)]
#[test]
fn the_file_list_is_the_native_walk_never_an_fd_on_path() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("real.rs"), "").unwrap();
    let bin = tempfile::tempdir().unwrap();
    let fake = bin.path().join("fd");
    std::fs::write(&fake, "#!/bin/sh\necho phantom.rs\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin.path().to_path_buf()];
    paths.extend(std::env::split_paths(&path));
    let joined = std::env::join_paths(paths).unwrap();
    let _env = crate::test_env::EnvGuard::set(&[("PATH", joined.to_str())]);
    assert_eq!(list_files(dir.path()), ["real.rs"]);
}
