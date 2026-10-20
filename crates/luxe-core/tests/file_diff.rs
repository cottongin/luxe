//! One file's change, captured against a real repository.
//!
//! The unit tests in `luxe_core::file_diff` cover the pure parts; these run git, because the whole
//! point of the module is what git says about a path in each of the states a working tree puts it
//! in — tracked, staged, brand new, renamed, gone, or never a repository at all.

use luxe_core::external_work::ExternalBaseline;
use luxe_core::file_diff::{capture, capture_external, FileDiffFailure};
use std::path::Path;
use std::process::Command;

/// Runs one git command in `root`, failing the test with git's own complaint.
fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("could not run git {arguments:?}: {error}"));
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A repository with one committed file, and identity set so committing works anywhere.
fn repository() -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    git(root, &["init", "--quiet"]);
    git(root, &["config", "user.name", "luxe Tests"]);
    git(root, &["config", "user.email", "tests@luxe.invalid"]);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "one\ntwo\nthree\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "--quiet", "-m", "initial"]);
    project
}

#[tokio::test]
async fn a_worktree_edit_is_described_without_staging_it() {
    let project = repository();
    std::fs::write(project.path().join("src/lib.rs"), "one\nTWO\nthree\n").unwrap();

    let diff = capture(project.path(), "src/lib.rs").await.unwrap();
    let file = diff.file.expect("an edited file has a change");

    assert!(!diff.untracked);
    assert_eq!(file.additions, Some(1));
    assert_eq!(file.deletions, Some(1));
    assert_eq!(file.new_path.as_deref(), Some(Path::new("src/lib.rs")));
    let lines: Vec<&str> = file.hunks[0].lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(lines, vec!["one", "two", "TWO", "three"]);
}

/// `HEAD`, not the index: a reader who clicked a row wants the file as it stands, not a lesson in
/// which of git's two waiting rooms the edit is sitting in.
#[tokio::test]
async fn a_staged_change_and_an_unstaged_one_are_both_shown() {
    let project = repository();
    std::fs::write(project.path().join("src/lib.rs"), "one\nSTAGED\nthree\n").unwrap();
    git(project.path(), &["add", "src/lib.rs"]);
    std::fs::write(project.path().join("src/lib.rs"), "one\nSTAGED\nWORKTREE\n").unwrap();

    let file = capture(project.path(), "src/lib.rs").await.unwrap().file.expect("a change");
    let added: Vec<&str> = file.hunks[0]
        .lines
        .iter()
        .filter(|line| line.kind == luxe_core::guided_review::ReviewLineKind::Addition)
        .map(|line| line.text.as_str())
        .collect();
    assert_eq!(added, vec!["STAGED", "WORKTREE"]);
}

/// The common case in an agent session: a file that did not exist an hour ago. Git has no diff for
/// it, so without this fallback the pane would open on an empty surface for every new file.
#[tokio::test]
async fn an_untracked_file_is_shown_as_every_line_added() {
    let project = repository();
    std::fs::write(project.path().join("src/new.rs").as_path(), "fn main() {}\n").unwrap();

    let diff = capture(project.path(), "src/new.rs").await.unwrap();
    let file = diff.file.expect("an untracked file is all addition");

    assert!(diff.untracked);
    assert_eq!(file.status, luxe_core::guided_review::ReviewFileStatus::Added);
    assert_eq!(file.additions, Some(1));
    assert_eq!(file.hunks[0].lines[0].text, "fn main() {}");
}

#[tokio::test]
async fn a_deleted_file_still_has_a_change_to_show() {
    let project = repository();
    std::fs::remove_file(project.path().join("src/lib.rs")).unwrap();

    let file = capture(project.path(), "src/lib.rs").await.unwrap().file.expect("a deletion");
    assert_eq!(file.status, luxe_core::guided_review::ReviewFileStatus::Deleted);
    assert_eq!(file.deletions, Some(3));
}

/// A rename is one record under two names, and the pane may be asked for either — the dock lists
/// the new path, a stale click carries the old one.
#[tokio::test]
async fn a_rename_answers_to_both_of_its_names() {
    let project = repository();
    git(project.path(), &["mv", "src/lib.rs", "src/renamed.rs"]);

    let by_new = capture(project.path(), "src/renamed.rs").await.unwrap();
    let by_old = capture(project.path(), "src/lib.rs").await.unwrap();
    assert_eq!(
        by_new.file.expect("the new name").new_path.as_deref(),
        Some(Path::new("src/renamed.rs"))
    );
    assert_eq!(
        by_old.file.expect("the old name").old_path.as_deref(),
        Some(Path::new("src/lib.rs"))
    );
}

/// A diff is a rendering, and a person's `~/.gitconfig` is entitled to change it. Every path in
/// the parsed record comes back wrong under `diff.mnemonicprefix`, which is a real setting real
/// people have — it was set on the machine this was written on.
#[tokio::test]
async fn a_hostile_local_git_configuration_cannot_change_the_output() {
    let project = repository();
    git(project.path(), &["config", "diff.mnemonicprefix", "true"]);
    git(project.path(), &["config", "diff.noprefix", "true"]);
    std::fs::write(project.path().join("src/lib.rs"), "one\nTWO\nthree\n").unwrap();

    let file = capture(project.path(), "src/lib.rs").await.unwrap().file.expect("a change");
    assert_eq!(file.new_path.as_deref(), Some(Path::new("src/lib.rs")));
    assert_eq!(file.old_path.as_deref(), Some(Path::new("src/lib.rs")));
}

/// "No change" and "no answer" are different facts, and collapsing them would have the pane
/// report a clean file as an error.
#[tokio::test]
async fn an_unchanged_file_reports_no_change_rather_than_a_failure() {
    let project = repository();
    let diff = capture(project.path(), "src/lib.rs").await.unwrap();
    assert!(diff.file.is_none());
    assert!(!diff.untracked);
}

#[tokio::test]
async fn a_repository_with_no_commits_yet_still_describes_its_files() {
    let project = tempfile::tempdir().unwrap();
    git(project.path(), &["init", "--quiet"]);
    std::fs::write(project.path().join("first.txt"), "hello\n").unwrap();

    let diff = capture(project.path(), "first.txt").await.unwrap();
    assert!(diff.untracked, "nothing is tracked before the first commit");
    assert_eq!(diff.file.expect("a change").additions, Some(1));
}

#[tokio::test]
async fn a_path_outside_the_project_is_refused_before_git_is_run() {
    let project = repository();
    for escape in ["../etc/passwd", "/etc/passwd", ""] {
        assert_eq!(
            capture(project.path(), escape).await.unwrap_err(),
            FileDiffFailure::UnsafePath,
            "{escape:?} should not be reachable"
        );
    }
}

#[tokio::test]
async fn a_directory_that_is_not_a_repository_says_so() {
    let plain = tempfile::tempdir().unwrap();
    std::fs::write(plain.path().join("notes.md"), "hello\n").unwrap();
    assert_eq!(capture(plain.path(), "notes.md").await.unwrap_err(), FileDiffFailure::NotAWorktree);
}

/// The path a peek asks with is the path it gets back, whatever git called the file internally.
///
/// The pane discards a capture whose path is not the one it is looking at, so a worktree answer
/// that came back under its own root-relative name would be thrown away by the surface that asked
/// for it — a click that quietly did nothing.
#[tokio::test]
async fn a_file_in_another_worktree_is_diffed_against_that_worktrees_head() {
    let elsewhere = repository();
    let file = elsewhere.path().join("src/lib.rs");
    std::fs::write(&file, "one\nTWO\nthree\n").unwrap();
    let requested = file.to_string_lossy().into_owned();

    let diff = capture_external(&requested, None).await.unwrap();
    assert_eq!(diff.path, requested, "answered under the name it was asked for");
    let change = diff.file.expect("another worktree answers the same question the project does");
    assert_eq!((change.additions, change.deletions), (Some(1), Some(1)));
    // Nothing was kept about this file and nothing had to be: Git is the witness here.
    let lines: Vec<&str> = change.hunks[0].lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(lines, vec!["one", "two", "TWO", "three"]);
}

/// A file in no repository has no `HEAD` to be measured against, so the copy the session retained
/// before it first wrote there is the only honest base.
#[tokio::test]
async fn a_loose_file_is_measured_against_what_the_session_first_saw() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("ghostty.config");
    std::fs::write(&file, "font-size = 13\ntheme = nord\n").unwrap();
    let requested = file.to_string_lossy().into_owned();
    let baseline = ExternalBaseline::Text { content: "font-size = 12\ntheme = nord\n".into() };

    let diff = capture_external(&requested, Some(&baseline)).await.unwrap();
    assert_eq!(diff.path, requested);
    let change = diff.file.expect("a retained copy is something to compare against");
    assert_eq!((change.additions, change.deletions), (Some(1), Some(1)));
    let lines: Vec<&str> = change.hunks[0].lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(lines, vec!["font-size = 12", "font-size = 13", "theme = nord"]);
}

/// The three other states a loose file can be in, each of which means something different.
#[tokio::test]
async fn a_loose_files_other_states_each_read_as_themselves() {
    use luxe_core::guided_review::ReviewFileStatus;
    let directory = tempfile::tempdir().unwrap();
    let created = directory.path().join("created.toml");
    std::fs::write(&created, "new = true\n").unwrap();
    let gone = directory.path().join("gone.toml");
    let requested = |path: &Path| path.to_string_lossy().into_owned();

    // Created by this session: every line of it is an addition.
    let addition = capture_external(&requested(&created), Some(&ExternalBaseline::Absent))
        .await
        .unwrap()
        .file
        .expect("a file the session created");
    assert_eq!(addition.status, ReviewFileStatus::Added);
    assert_eq!(addition.hunks[0].lines[0].text, "new = true");

    // Removed by this session: all that is left of it is what was retained.
    let held = ExternalBaseline::Text { content: "was = here\nand = gone\n".into() };
    let deletion = capture_external(&requested(&gone), Some(&held))
        .await
        .unwrap()
        .file
        .expect("a file the session removed");
    assert_eq!(deletion.status, ReviewFileStatus::Deleted);
    assert_eq!(deletion.deletions, Some(2));

    // Touched and left exactly as it was found: not a change, and not a failure either.
    let same = ExternalBaseline::Text { content: "new = true\n".into() };
    assert!(capture_external(&requested(&created), Some(&same)).await.unwrap().file.is_none());

    // Created and removed again inside one session: the filesystem is where it started.
    let vanished =
        capture_external(&requested(&gone), Some(&ExternalBaseline::Absent)).await.unwrap().file;
    assert!(vanished.is_none());
}

/// A file too large or too binary to retain, and one luxe never saw, are the same answer: it
/// cannot say what changed, so it must not present the file as new.
#[tokio::test]
async fn a_loose_file_with_no_retained_copy_refuses_to_guess() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("huge.log");
    std::fs::write(&file, "a line\n").unwrap();
    let requested = file.to_string_lossy().into_owned();

    for baseline in [None, Some(&ExternalBaseline::Unretained)] {
        assert_eq!(
            capture_external(&requested, baseline).await.unwrap_err(),
            FileDiffFailure::NoBaseline
        );
    }
}

#[tokio::test]
async fn an_external_peek_refuses_a_path_that_is_not_absolute_and_plain() {
    for escape in ["src/lib.rs", "", "/tmp/../etc/passwd"] {
        assert_eq!(
            capture_external(escape, Some(&ExternalBaseline::Absent)).await.unwrap_err(),
            FileDiffFailure::UnsafePath,
            "{escape:?} should not be reachable"
        );
    }
}

/// Binary content has no lines, and inventing some would be worse than saying so.
#[tokio::test]
async fn a_binary_change_is_reported_as_a_non_text_record() {
    let project = repository();
    std::fs::write(project.path().join("logo.png"), [0u8, 1, 2, 3]).unwrap();
    git(project.path(), &["add", "logo.png"]);
    git(project.path(), &["commit", "--quiet", "-m", "add binary"]);
    std::fs::write(project.path().join("logo.png"), [9u8, 9, 9, 9]).unwrap();

    let file = capture(project.path(), "logo.png").await.unwrap().file.expect("a binary change");
    assert!(!file.is_text());
    assert!(file.non_text.is_some(), "the reason is named rather than rendered as zero lines");
}
