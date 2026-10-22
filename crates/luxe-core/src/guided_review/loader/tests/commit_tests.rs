//! Tests — turning reviewed chapters into commits, and refusing when that is not safe.

use super::super::*;
use crate::guided_review::{chapter_commits, ChapterCommit};

/// A repository with one commit and a two-file working-tree change.
async fn project_with_changes() -> (tempfile::TempDir, LocalGit) {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    // In the repository, not as `-c` flags on the setup commit: the commit under test is made by
    // *product* code, which rightly never invents an author. A machine with no global identity — a
    // CI runner, a fresh container — then fails at the assertion rather than at the fixture.
    git.run("configure author", &["config", "user.name", "luxe Tests"]).await.unwrap();
    git.run("configure author", &["config", "user.email", "tests@luxe.local"]).await.unwrap();
    std::fs::write(project.path().join("lib.rs"), "let a = 1;\n").unwrap();
    std::fs::write(project.path().join("notes.md"), "old\n").unwrap();
    git.run("stage initial files", &["add", "."]).await.unwrap();
    git.run("create initial commit", &["commit", "--quiet", "-m", "base"]).await.unwrap();
    std::fs::write(project.path().join("lib.rs"), "let a = 2;\n").unwrap();
    std::fs::write(project.path().join("notes.md"), "new\n").unwrap();
    (project, git)
}

async fn working_tree_review(git: &LocalGit) -> GuidedReview {
    git.load_target_review(
        ReviewId::new(),
        ReviewTarget::WorkingTree,
        crate::guided_review::ReviewContextDraft {
            title: "Reviewed change".into(),
            ..Default::default()
        }
        .build(),
        UntrackedSourcePolicy::default(),
    )
    .await
    .expect("the working tree loads")
}

async fn log_subjects(git: &LocalGit) -> Vec<String> {
    let out = git.run("read log", &["log", "--format=%s"]).await.unwrap();
    String::from_utf8_lossy(&out).lines().map(str::to_owned).collect()
}

/// Chapters are a hunk-exact partition of the change, so each becomes its own commit carrying the
/// reasoning that made it legible — the part of a review that otherwise evaporates.
#[tokio::test]
async fn each_chapter_commits_separately_with_its_rationale() {
    let (project, git) = project_with_changes().await;
    let review = working_tree_review(&git).await;
    let commits = chapter_commits(&review, &[]).expect("chapters render");
    assert!(commits.len() >= 2, "core and supporting changes are separate");

    let created = git.commit_review_chapters(&review, &commits, false).await.expect("commits land");

    assert_eq!(created.len(), commits.len());
    let subjects = log_subjects(&git).await;
    assert_eq!(subjects[0], "Supporting changes");
    assert_eq!(subjects[1], "Core implementation");
    assert_eq!(subjects[2], "base");
    // Nothing reviewed is left behind. (`.luxe/` holds the pinned source cache and is never part
    // of a review, so the check names the reviewed paths.)
    let status = git
        .run("read status", &["status", "--porcelain", "--", "lib.rs", "notes.md"])
        .await
        .unwrap();
    assert!(
        status.is_empty(),
        "every reviewed change reached a commit; left: {}",
        String::from_utf8_lossy(&status)
    );
    let body = git.run("read body", &["log", "-1", "--format=%b", "HEAD~1"]).await.unwrap();
    let body = String::from_utf8_lossy(&body);
    assert!(body.contains("Groups source-backed implementation changes."));
    assert!(!body.contains("Luxe-"), "no luxe ids in the message: {body}");
    drop(project);
}

/// A pre-staged change would be swept into a chapter commit, so the flow refuses instead.
#[tokio::test]
async fn a_dirty_index_refuses_the_commit() {
    let (project, git) = project_with_changes().await;
    let review = working_tree_review(&git).await;
    let commits = chapter_commits(&review, &[]).expect("chapters render");
    git.run("stage something else", &["add", "notes.md"]).await.unwrap();

    let error = git.commit_review_chapters(&review, &commits, false).await.unwrap_err();

    assert!(matches!(error, GuidedReviewError::CommitRefused { .. }), "{error}");
    assert_eq!(log_subjects(&git).await, ["base"], "nothing was committed");
    drop(project);
}

/// Commits must contain what the reviewer actually read, so a moved worktree is refused.
#[tokio::test]
async fn a_worktree_that_moved_on_refuses_the_commit() {
    let (project, git) = project_with_changes().await;
    let review = working_tree_review(&git).await;
    let commits = chapter_commits(&review, &[]).expect("chapters render");
    std::fs::write(project.path().join("lib.rs"), "let a = 3; // changed after review\n").unwrap();

    let error = git.commit_review_chapters(&review, &commits, false).await.unwrap_err();

    assert!(matches!(error, GuidedReviewError::CommitRefused { .. }), "{error}");
    assert_eq!(log_subjects(&git).await, ["base"], "nothing was committed");
    drop(project);
}

/// A patch that cannot apply leaves the index exactly as it was found.
#[tokio::test]
async fn a_failed_chapter_leaves_the_index_untouched() {
    let (project, git) = project_with_changes().await;
    let review = working_tree_review(&git).await;
    let broken = ChapterCommit {
        chapter_id: review.chapters[0].id.clone(),
        subject: "Broken".into(),
        message: "Broken\n".into(),
        patch: "diff --git a/missing.rs b/missing.rs\n--- a/missing.rs\n+++ b/missing.rs\n@@ -1,1 +1,1 @@\n-nope\n+nope2\n".into(),
        skipped: Vec::new(),
    };

    let error = git.commit_review_chapters(&review, &[broken], false).await.unwrap_err();

    assert!(matches!(error, GuidedReviewError::GitFailed { .. }), "{error}");
    let staged = git.run("read index", &["diff", "--cached", "--name-only"]).await.unwrap();
    assert!(staged.is_empty(), "a failed apply does not leave a half-staged index");
    assert_eq!(log_subjects(&git).await, ["base"]);
    drop(project);
}

/// A file the change adds — untracked, as an agent leaves it — and one it deletes each commit
/// alone. Git reads `/dev/null` as "no file" only after a mode line says so: without one a lone
/// addition was refused as `dev/null: does not exist in index`, and a deletion committed a file
/// named `dev/null`.
#[tokio::test]
async fn an_added_or_a_deleted_file_commits_alone() {
    for change in ["add", "delete"] {
        let (project, git) = project_with_changes().await;
        git.run("drop the other changes", &["checkout", "--", "."]).await.unwrap();
        match change {
            "add" => {
                std::fs::write(project.path().join("added.md"), "Written by the agent.\n").unwrap()
            }
            _ => std::fs::remove_file(project.path().join("notes.md")).unwrap(),
        }
        let review = working_tree_review(&git).await;
        let commits = chapter_commits(&review, &[]).expect("chapters render");
        assert_eq!(commits.len(), 1, "{change}");

        git.commit_review_chapters(&review, &commits, false).await.expect(change);

        let status = git
            .run("read status", &["status", "--porcelain", "--", "notes.md", "added.md"])
            .await
            .unwrap();
        assert!(status.is_empty(), "{change} left: {}", String::from_utf8_lossy(&status));
        // Without the mode line a deletion "works", and stages a file named `dev/null` beside it
        let tree = git.run("read tree", &["ls-tree", "-r", "--name-only", "HEAD"]).await.unwrap();
        let tree = String::from_utf8_lossy(&tree);
        assert!(!tree.contains("dev/null"), "{change}: {tree}");
        drop(project);
    }
}

/// A chapter patch bigger than a pipe holds stages and commits: the write and the wait on Git
/// happen together, so neither waits on the other.
#[tokio::test]
async fn a_chapter_patch_larger_than_a_pipe_still_stages() {
    let (project, git) = project_with_changes().await;
    let big: String = (0..40_000).map(|n| format!("line {n} of a long generated file\n")).collect();
    std::fs::write(project.path().join("generated.txt"), &big).unwrap();
    let patch = format!(
        "diff --git a/generated.txt b/generated.txt\nnew file mode 100644\n--- /dev/null\n+++ b/generated.txt\n@@ -0,0 +1,40000 @@\n{}",
        big.lines().map(|line| format!("+{line}\n")).collect::<String>()
    );
    assert!(patch.len() > 1 << 20);
    std::fs::remove_file(project.path().join("generated.txt")).unwrap();

    git.stage_patch(&patch).await.expect("the patch stages");

    let staged = git.run("list staged", &["diff", "--cached", "--name-only"]).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&staged).trim(), "generated.txt");
}

/// Staging gives up at the runner's deadline, as every other review command does, and says Git
/// timed out.
///
/// The hang is real: Git opens `.git/index` to read it, and a named pipe with no writer blocks
/// that open for good. The break this catches: `stage_patch` spawned `git apply` on its own with
/// a fixed wait of its own, so the loader's deadline never reached it and a stall there surfaced
/// as an untyped failure after the full wait instead of [`GuidedReviewError::GitTimeout`].
#[cfg(unix)]
#[tokio::test]
async fn staging_a_patch_honors_the_runners_deadline() {
    let (project, git) = project_with_changes().await;
    let index = project.path().join(".git").join("index");
    std::fs::remove_file(&index).unwrap();
    assert!(std::process::Command::new("mkfifo").arg(&index).status().unwrap().success());
    let git = git.with_git_deadline(std::time::Duration::from_millis(500));
    let patch = "diff --git a/lib.rs b/lib.rs\n--- a/lib.rs\n+++ b/lib.rs\n@@ -1 +1 @@\n-let a = 1;\n+let a = 2;\n";

    let started = std::time::Instant::now();
    let error = git.stage_patch(patch).await.expect_err("it gives up");

    assert!(matches!(error, GuidedReviewError::GitTimeout { .. }), "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}
