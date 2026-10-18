//! The in-process search, pinned to the rows ripgrep prints for the same request, which is what
//! the model and every row parser here expect.

use super::*;

fn search(cwd: &Path, search: &Search<'_>) -> Vec<String> {
    native_search(cwd, search).expect("the search runs")
}

/// A plain content search of `.`, generous enough that the cap never masks a row.
fn base(pattern: &str) -> Search<'_> {
    Search {
        pattern,
        path: Path::new("."),
        glob: None,
        before: 0,
        after: 0,
        all: false,
        invert: false,
        scopes: false,
        mode: OutputMode::Content,
        limit: 500,
    }
}

/// A tree with an ignored directory, nesting, two matches in one file, and Git's own files.
fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src/deep")).unwrap();
    std::fs::write(root.join("src/a.rs"), "fn a() {}\n// needle one\nfn b() {}\n// needle two\n")
        .unwrap();
    std::fs::write(root.join("src/deep/c.rs"), "// needle three\nlet x = 1;\n").unwrap();
    std::fs::write(root.join("notes.md"), "needle four\n").unwrap();
    std::fs::write(root.join(".gitignore"), "skipme/\n").unwrap();
    std::fs::create_dir_all(root.join("skipme")).unwrap();
    std::fs::write(root.join("skipme/d.rs"), "// needle five\n").unwrap();
    std::fs::create_dir_all(root.join(".git/refs")).unwrap();
    std::fs::write(root.join(".git/config"), "needle six\n").unwrap();
    dir
}

#[test]
fn rows_come_in_ripgreps_shape_and_path_order() {
    let dir = tree();
    let cwd = dir.path();
    let needle = base("needle");
    assert_eq!(
        search(cwd, &needle),
        [
            "notes.md:1:needle four",
            "src/a.rs:2:// needle one",
            "src/a.rs:4:// needle two",
            "src/deep/c.rs:1:// needle three"
        ]
    );
    // a match inside another's context is still a match, and each line prints once
    let context = Search { before: 1, after: 1, ..needle.clone() };
    assert_eq!(
        search(cwd, &context)[1..5],
        [
            "src/a.rs-1-fn a() {}",
            "src/a.rs:2:// needle one",
            "src/a.rs-3-fn b() {}",
            "src/a.rs:4:// needle two"
        ]
    );
    let files = Search { mode: OutputMode::Files, glob: Some("*.rs"), ..needle.clone() };
    assert_eq!(search(cwd, &files), ["src/a.rs", "src/deep/c.rs"]);
    let count = Search { mode: OutputMode::Count, ..needle.clone() };
    assert_eq!(search(cwd, &count), ["notes.md:1", "src/a.rs:2", "src/deep/c.rs:1"]);
    let all = Search { all: true, mode: OutputMode::Files, ..needle };
    assert_eq!(search(cwd, &all), ["notes.md", "skipme/d.rs", "src/a.rs", "src/deep/c.rs"]);
}

/// Spread over more files than there are workers, the rows still come back in path order.
#[test]
fn a_parallel_scan_keeps_path_order() {
    let dir = tempfile::tempdir().unwrap();
    let expected: Vec<String> = (0..200)
        .map(|index| {
            let name = format!("f{index:03}.txt");
            std::fs::write(dir.path().join(&name), "needle\n").unwrap();
            format!("{name}:1:needle")
        })
        .collect();
    assert_eq!(search(dir.path(), &base("needle")), expected);
}

/// A project with a fetched repository in it: `.luxe/` ignores everything, the clone is a git
/// repository of its own and ignores its own `node_modules/`.
fn project_with_a_clone() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let clone = root.join(".luxe/gh-cache/o/r");
    std::fs::create_dir_all(clone.join(".git")).unwrap();
    std::fs::create_dir_all(clone.join("docs")).unwrap();
    std::fs::create_dir_all(clone.join("node_modules/x")).unwrap();
    std::fs::write(root.join(".luxe/.gitignore"), "*\n").unwrap();
    std::fs::write(clone.join(".gitignore"), "node_modules/\n").unwrap();
    std::fs::write(clone.join("docs/a.md"), "needle\n").unwrap();
    std::fs::write(clone.join("node_modules/x/b.md"), "needle\n").unwrap();
    dir
}

/// `web_fetch` says to explore a clone, which is searchable by its own ignore rules, not `.luxe/`'s.
#[test]
fn a_fetched_repository_is_searchable_by_its_own_rules() {
    let dir = project_with_a_clone();
    let clone =
        Search { path: Path::new(".luxe/gh-cache/o/r"), mode: OutputMode::Files, ..base("needle") };
    assert_eq!(search(dir.path(), &clone), [".luxe/gh-cache/o/r/docs/a.md"]);
}

/// Only a clone searches by its own rules: a subdirectory of the project still answers to the
/// project's `.gitignore`.
#[test]
fn a_subdirectory_of_the_project_still_honors_the_projects_ignore_file() {
    let dir = project_with_a_clone();
    let cwd = dir.path();
    std::fs::write(cwd.join(".gitignore"), "src/gen/\n").unwrap();
    std::fs::create_dir_all(cwd.join("src/gen")).unwrap();
    std::fs::write(cwd.join("src/a.rs"), "needle\n").unwrap();
    std::fs::write(cwd.join("src/gen/b.rs"), "needle\n").unwrap();
    let sub = Search { path: Path::new("src"), mode: OutputMode::Files, ..base("needle") };
    assert_eq!(search(cwd, &sub), ["src/a.rs"]);
}

/// The whole-file test that skips files never rules out a line that matches: anchors read per
/// line, CRLF included, and `\A` keeps its per-line meaning.
#[test]
fn the_prefilter_never_hides_a_matching_line() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("unix.py"), "x = 1\ndef run():\n    pass\n").unwrap();
    std::fs::write(dir.path().join("dos.txt"), "first end\r\nsecond end\r\n").unwrap();
    let cwd = dir.path();
    assert_eq!(search(cwd, &base("^def")), ["unix.py:2:def run():"]);
    assert_eq!(search(cwd, &base("second end$")), ["dos.txt:2:second end"]);
    assert_eq!(search(cwd, &base(r"\Adef")), ["unix.py:2:def run():"]);
    assert_eq!(search(cwd, &base(r"pass\z")), ["unix.py:3:    pass"]);
    // a lone `\r` stays inside a line, where `.` matches it and CRLF mode would not
    std::fs::write(cwd.join("progress.log"), "10%\r20%\rdone\n").unwrap();
    assert_eq!(search(cwd, &base("10%.*done")), ["progress.log:1:10%\r20%\rdone"]);
    // an inline flag turning multi-line off makes `^` the line's start again, per line
    assert_eq!(search(cwd, &base("(?-m)^def")), ["unix.py:2:def run():"]);
}

/// A worker that crashed is a failure the caller hears about; one that was stopped is a cancel.
#[test]
fn a_crashed_worker_is_a_failure_not_a_cancel() {
    let rows = |file: &str| {
        let hit = Hit {
            file: file.into(),
            kind: HitKind::File,
            line: 0,
            text: String::new(),
            scope: None,
        };
        Ok(Some(vec![hit]))
    };
    let crashed: std::thread::Result<Option<Vec<Hit>>> = Err(Box::new("boom"));
    let failed = combine(vec![rows("a"), crashed], false).unwrap_err();
    assert!(failed.contains("worker failed"), "{failed}");
    assert_eq!(combine(vec![rows("a"), Ok(None)], false).unwrap_err(), "grep cancelled");
    let joined = combine(vec![rows("a"), rows("b")], false).unwrap();
    assert_eq!(joined.iter().map(Hit::flat).collect::<Vec<_>>(), ["a", "b"]);
}
