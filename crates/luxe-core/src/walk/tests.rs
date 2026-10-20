use super::*;

/// A model on Windows writes `src\*.py`, and globset read the backslash as an escape, so the
/// pattern matched nothing.
#[cfg(windows)]
#[test]
fn a_backslash_in_a_windows_glob_is_a_folder() {
    let matcher = matcher(r"src\*.py").unwrap();
    assert!(matcher.is_match(Path::new("src/a.py")));
    assert!(!matcher.is_match(Path::new("lib/a.py")));
}

/// A leading `!` excludes what the rest of the pattern matches, the way `rg -g` reads it.
///
/// The break this catches: `!` read as a literal character, so `!.pi/**` matched only paths
/// under a folder named `!.pi` and a grep with it found nothing at all.
#[test]
fn a_leading_bang_excludes_what_the_rest_matches() {
    let folder = matcher("!.pi/**").unwrap();
    assert!(!folder.is_match(Path::new(".pi/notes.md")));
    assert!(!folder.is_match(Path::new("sub/.pi/notes.md")), "at any depth, like the rest");
    assert!(folder.is_match(Path::new("src/main.rs")));
    let name = matcher("!*.md").unwrap();
    assert!(!name.is_match(Path::new("docs/guide.md")));
    assert!(name.is_match(Path::new("docs/build.rs")));
}

/// A walk that stops at its ceiling must say so, or a search over a huge tree reads as complete.
#[test]
fn a_walk_that_hits_its_ceiling_says_so() {
    let dir = repo();
    let walk = |limit| walk_within(dir.path(), Path::new("."), (true, None), || false, limit);
    let walked = walk(2);
    assert_eq!(walked.files.len(), 2);
    assert!(walked.capped);
    let whole = walk(100);
    assert!(!whole.capped, "{:?}", whole.files);
}

/// An index walk that stops at its limit keeps the same files every time: the first by name.
#[test]
fn a_capped_index_walk_keeps_the_first_files_by_name() {
    let dir = tempfile::tempdir().unwrap();
    for index in 0..40 {
        let sub = dir.path().join(format!("d{}", index % 4));
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(format!("f{index:02}.rs")), "").unwrap();
    }
    std::fs::create_dir_all(dir.path().join(".hidden")).unwrap();
    std::fs::write(dir.path().join(".hidden/a.rs"), "").unwrap();
    let walk = || index_files(dir.path(), &[], |path| path.extension().is_some(), 10);
    let first = walk();
    assert!(first.capped);
    assert_eq!(first.files.len(), 10);
    assert!(first.files.iter().all(|path| path.starts_with("d0") || path.starts_with("d1")));
    assert_eq!(first.files, walk().files, "the same ten each time");
    assert!(!first.files.iter().any(|path| path.starts_with(".hidden")));
}

/// A listing keeps hidden files, honors the ignore file, and never enters a skipped folder,
/// ignored or not.
#[test]
fn a_listing_keeps_dotfiles_and_skips_the_named_folders() {
    let dir = repo();
    std::fs::create_dir_all(dir.path().join(".github")).unwrap();
    std::fs::write(dir.path().join(".github/ci.yml"), "").unwrap();
    std::fs::create_dir_all(dir.path().join("vendor")).unwrap();
    std::fs::write(dir.path().join("vendor/lib.rs"), "").unwrap();
    let listed = listing(dir.path(), &["vendor"]);
    assert!(!listed.capped);
    let dotfile = Path::new(".github").join("ci.yml");
    assert!(listed.files.contains(&dotfile), "{:?}", listed.files);
    assert!(listed.files.iter().all(|path| !path.starts_with("vendor")));
    assert!(listed.files.iter().all(|path| !path.starts_with("target")));
}

/// A listing that stops at its limit keeps the same files every time: the first by name.
#[test]
fn a_capped_listing_keeps_the_first_files_by_name() {
    let dir = tempfile::tempdir().unwrap();
    for index in 0..40 {
        let sub = dir.path().join(format!("d{}", index % 4));
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(format!("f{index:02}.rs")), "").unwrap();
    }
    let first = listing_within(dir.path(), &[], 10);
    assert!(first.capped);
    assert_eq!(first.files.len(), 10);
    assert!(first.files.iter().all(|path| path.starts_with("d0")), "{:?}", first.files);
    assert_eq!(first.files, listing_within(dir.path(), &[], 10).files);
}

/// A repo with one real source file and a build directory full of look-alikes.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
    std::fs::create_dir_all(dir.path().join("src").join("deep")).unwrap();
    std::fs::write(dir.path().join("src").join("main.rs"), "fn main() {}").unwrap();
    std::fs::write(dir.path().join("src").join("deep").join("nested.rs"), "").unwrap();
    std::fs::create_dir_all(dir.path().join("target").join("debug")).unwrap();
    std::fs::write(dir.path().join("target").join("debug").join("gen.rs"), "").unwrap();
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    std::fs::write(dir.path().join(".git").join("config"), "").unwrap();
    dir
}

fn names(paths: &[PathBuf]) -> Vec<String> {
    paths.iter().map(|path| crate::paths::display(path)).collect()
}

#[test]
fn the_walk_honors_ignore_files_and_never_enters_dot_git() {
    let dir = repo();
    let found = names(&files(dir.path(), Path::new("."), false));
    assert!(found.contains(&"src/main.rs".to_string()), "{found:?}");
    assert!(found.contains(&"src/deep/nested.rs".to_string()), "{found:?}");
    assert!(!found.iter().any(|path| path.contains("target/")), "ignored: {found:?}");
    assert!(!found.iter().any(|path| path.contains(".git/")), "never the object store: {found:?}");
}

#[test]
fn all_reaches_ignored_files_but_still_not_dot_git() {
    let dir = repo();
    let found = names(&files(dir.path(), Path::new("."), true));
    assert!(found.contains(&"target/debug/gen.rs".to_string()), "{found:?}");
    assert!(!found.iter().any(|path| path.contains(".git/")), "{found:?}");
}

/// The bug the `find` fallback shipped with: `**/` means *zero or more* directories, so
/// `src/**/*.rs` has to match `src/main.rs` — a file sitting directly under the globbed
/// directory, which is the common case.
#[test]
fn a_directory_spanning_glob_matches_zero_intermediate_directories() {
    let glob = matcher("src/**/*.rs").unwrap();
    assert!(glob.is_match(Path::new("src/main.rs")), "zero directories in between");
    assert!(glob.is_match(Path::new("src/deep/nested.rs")), "and any number of them");
    assert!(glob.is_match(Path::new("crates/x/src/main.rs")), "anchored to match at any depth");
    assert!(!glob.is_match(Path::new("tests/main.rs")));
}

#[test]
fn a_bare_pattern_matches_the_file_name_at_any_depth() {
    let glob = matcher("*.rs").unwrap();
    assert!(glob.is_match(Path::new("main.rs")));
    assert!(glob.is_match(Path::new("src/deep/nested.rs")));
    assert!(!glob.is_match(Path::new("src/main.py")));
}

#[test]
fn a_malformed_glob_is_an_error_rather_than_a_silent_no_match() {
    assert!(matcher("src/[").is_err());
}
