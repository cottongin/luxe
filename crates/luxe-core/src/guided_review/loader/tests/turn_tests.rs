//! Tests — last-turn source capture.

use super::super::*;

#[tokio::test]
async fn last_turn_no_change_loads_an_empty_pinned_source() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    std::fs::write(project.path().join("tracked.txt"), "before\n").unwrap();
    git.run("stage tracked file", &["add", "tracked.txt"]).await.unwrap();
    let baseline = git
        .capture_and_persist_turn_baseline("session", "turn", UntrackedSourcePolicy::default())
        .await
        .unwrap();
    git.capture_and_persist_post_turn_snapshot("session", "turn", UntrackedSourcePolicy::default())
        .await
        .unwrap();

    let capture = git
        .capture_and_persist_last_turn_source("session", "turn", UntrackedSourcePolicy::default())
        .await
        .unwrap();

    assert_eq!(
        capture.source.base,
        ReviewBase::TurnBaseline { revision_id: baseline.revision_id() }
    );
    assert!(capture.artifact.canonical_bytes.is_empty());
    let diff = std::str::from_utf8(&capture.artifact.canonical_bytes).unwrap();
    assert!(parse_unified_diff(diff).unwrap().files.is_empty());
}

#[tokio::test]
async fn last_turn_preserves_preexisting_dirty_and_untracked_content() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    git.run("configure test identity", &["config", "user.email", "tests@example.invalid"])
        .await
        .unwrap();
    git.run("configure test name", &["config", "user.name", "luxe Tests"]).await.unwrap();
    std::fs::write(project.path().join("tracked.txt"), "committed\n").unwrap();
    git.run("stage fixture", &["add", "tracked.txt"]).await.unwrap();
    git.run("commit fixture", &["commit", "--quiet", "-m", "fixture"]).await.unwrap();

    std::fs::write(project.path().join("tracked.txt"), "pre-dirty\n").unwrap();
    std::fs::write(project.path().join("note.txt"), "pre-untracked\n").unwrap();
    git.capture_and_persist_turn_baseline("session", "turn", UntrackedSourcePolicy::default())
        .await
        .unwrap();

    std::fs::write(project.path().join("tracked.txt"), "post-dirty\n").unwrap();
    std::fs::write(project.path().join("note.txt"), "post-untracked\n").unwrap();
    git.capture_and_persist_post_turn_snapshot("session", "turn", UntrackedSourcePolicy::default())
        .await
        .unwrap();
    let capture = git
        .capture_and_persist_last_turn_source("session", "turn", UntrackedSourcePolicy::default())
        .await
        .unwrap();
    let diff = std::str::from_utf8(&capture.artifact.canonical_bytes).unwrap();

    assert!(diff.contains("-pre-dirty\n+post-dirty\n"));
    assert!(diff.contains("-pre-untracked\n+post-untracked\n"));
}

/// A repository holding `tracked.txt`, `doomed.txt` and a tracked file over a 16-byte limit.
async fn repository_with_an_oversized_file() -> (tempfile::TempDir, LocalGit) {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    std::fs::write(project.path().join("tracked.txt"), "before\n").unwrap();
    std::fs::write(project.path().join("doomed.txt"), "doomed\n").unwrap();
    std::fs::write(project.path().join("archive.md"), "far too long to snapshot\n").unwrap();
    git.run("stage fixture", &["add", "."]).await.unwrap();
    (project, git)
}

const SMALL: UntrackedSourcePolicy = UntrackedSourcePolicy { max_file_bytes: 16 };

/// One tracked file over the source limit leaves the rest of the turn reviewable, and the review
/// names the file it could not cover.
///
/// The break this catches: the baseline gave up whole when any tracked file was over the limit, so
/// one large journal in a repository meant "what the last turn changed" never worked there.
#[tokio::test]
async fn an_oversized_file_leaves_the_rest_of_the_turn_reviewable() {
    let (project, git) = repository_with_an_oversized_file().await;
    let baseline =
        git.capture_and_persist_turn_baseline("session", "turn", SMALL).await.expect("captured");
    let excluded = &baseline.manifest.excluded;
    assert_eq!(excluded.len(), 1);
    assert_eq!(excluded[0].path, std::path::PathBuf::from("archive.md"));
    assert_eq!(excluded[0].byte_len, Some(25), "with what its metadata said");

    std::fs::write(project.path().join("tracked.txt"), "after\n").unwrap();
    std::fs::write(project.path().join("archive.md"), "longer still, and changed\n").unwrap();
    git.capture_and_persist_post_turn_snapshot("session", "turn", SMALL).await.unwrap();
    let capture = git.capture_and_persist_last_turn_source("session", "turn", SMALL).await.unwrap();

    let diff = std::str::from_utf8(&capture.artifact.canonical_bytes).unwrap();
    assert!(diff.contains("-before\n+after\n"), "{diff}");
    assert!(!diff.contains("archive.md"), "{diff}");
    assert_eq!(capture.source.uncovered_paths, vec![std::path::PathBuf::from("archive.md")]);
}

/// A file that outgrows the limit during the turn is uncovered, never shown as deleted.
#[tokio::test]
async fn a_file_that_outgrows_the_limit_is_not_a_deletion() {
    let (project, git) = repository_with_an_oversized_file().await;
    git.capture_and_persist_turn_baseline("session", "turn", SMALL).await.unwrap();
    std::fs::write(project.path().join("tracked.txt"), "now far too long to keep\n").unwrap();
    git.capture_and_persist_post_turn_snapshot("session", "turn", SMALL).await.unwrap();
    let capture = git.capture_and_persist_last_turn_source("session", "turn", SMALL).await.unwrap();

    let diff = std::str::from_utf8(&capture.artifact.canonical_bytes).unwrap();
    assert!(!diff.contains("tracked.txt"), "{diff}");
    let uncovered = &capture.source.uncovered_paths;
    assert!(uncovered.contains(&std::path::PathBuf::from("tracked.txt")), "{uncovered:?}");
}

/// A tracked file removed from the tree during the turn reads as deleted, not as uncovered.
#[tokio::test]
async fn a_file_removed_during_the_turn_is_a_deletion() {
    let (project, git) = repository_with_an_oversized_file().await;
    git.capture_and_persist_turn_baseline("session", "turn", SMALL).await.unwrap();
    std::fs::remove_file(project.path().join("doomed.txt")).unwrap();
    git.capture_and_persist_post_turn_snapshot("session", "turn", SMALL).await.unwrap();
    let capture = git.capture_and_persist_last_turn_source("session", "turn", SMALL).await.unwrap();

    let diff = std::str::from_utf8(&capture.artifact.canonical_bytes).unwrap();
    assert!(diff.contains("deleted file mode") && diff.contains("-doomed\n"), "{diff}");
    assert!(capture.source.uncovered_paths.is_empty(), "the untouched archive is not named");
}

/// A file too large to hold that the turn did not touch is not named: in a repository with a big
/// journal, every review would otherwise say it could not cover a file nobody changed.
#[tokio::test]
async fn an_untouched_oversized_file_is_not_named() {
    let (project, git) = repository_with_an_oversized_file().await;
    git.capture_and_persist_turn_baseline("session", "turn", SMALL).await.unwrap();
    std::fs::write(project.path().join("tracked.txt"), "after\n").unwrap();
    git.capture_and_persist_post_turn_snapshot("session", "turn", SMALL).await.unwrap();
    let capture = git.capture_and_persist_last_turn_source("session", "turn", SMALL).await.unwrap();

    let diff = std::str::from_utf8(&capture.artifact.canonical_bytes).unwrap();
    assert!(diff.contains("-before\n+after\n") && !diff.contains("archive.md"), "{diff}");
    assert!(capture.source.uncovered_paths.is_empty(), "{:?}", capture.source.uncovered_paths);
}

/// An untracked file the turn writes over the limit is named, not added, and the rest loads.
#[tokio::test]
async fn an_oversized_new_file_is_named_not_added() {
    let (project, git) = repository_with_an_oversized_file().await;
    git.capture_and_persist_turn_baseline("session", "turn", SMALL).await.unwrap();
    std::fs::write(project.path().join("dump.log"), "a new file far too long to keep\n").unwrap();
    std::fs::write(project.path().join("tracked.txt"), "after\n").unwrap();
    git.capture_and_persist_post_turn_snapshot("session", "turn", SMALL).await.unwrap();
    let capture = git.capture_and_persist_last_turn_source("session", "turn", SMALL).await.unwrap();

    let diff = std::str::from_utf8(&capture.artifact.canonical_bytes).unwrap();
    assert!(diff.contains("-before\n+after\n") && !diff.contains("dump.log"), "{diff}");
    assert_eq!(capture.source.uncovered_paths, vec![std::path::PathBuf::from("dump.log")]);
}
