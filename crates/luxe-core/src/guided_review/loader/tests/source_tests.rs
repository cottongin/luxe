//! Tests — untracked classification and working-tree source assembly.

use super::super::*;

#[tokio::test]
async fn untracked_text_addition_is_deterministic_and_parseable() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    let path = PathBuf::from("with space.txt");
    std::fs::write(project.path().join(&path), "first\nsecond").unwrap();

    let diff = git
        .capture_untracked_text_addition(path.as_path(), UntrackedSourcePolicy::default())
        .await
        .unwrap();
    let parsed = super::super::parse_unified_diff(&diff).unwrap();

    assert_eq!(parsed.files[0].new_path, Some(path));
    assert_eq!(parsed.files[0].additions, Some(2));
    assert_eq!(parsed.files[0].hunks[0].lines.len(), 3);

    let empty_path = PathBuf::from("empty.txt");
    std::fs::write(project.path().join(&empty_path), "").unwrap();
    let empty_diff = git
        .capture_untracked_text_addition(empty_path.as_path(), UntrackedSourcePolicy::default())
        .await
        .unwrap();
    let empty = super::super::parse_unified_diff(&empty_diff).unwrap();
    assert_eq!(empty.files[0].additions, Some(0));
    assert!(empty.files[0].hunks.is_empty());
}

#[tokio::test]
async fn complete_head_worktree_source_has_one_tracked_stream_and_sorted_untracked_additions() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    std::fs::write(project.path().join("tracked.txt"), "base\n").unwrap();
    git.run("stage initial file", &["add", "tracked.txt"]).await.unwrap();
    git.run(
        "create initial commit",
        &[
            "-c",
            "user.name=Luxe Tests",
            "-c",
            "user.email=tests@luxe.invalid",
            "commit",
            "--quiet",
            "-m",
            "initial",
        ],
    )
    .await
    .unwrap();
    std::fs::write(project.path().join("tracked.txt"), "staged-only\n").unwrap();
    git.run("stage tracked change", &["add", "tracked.txt"]).await.unwrap();
    std::fs::write(project.path().join("tracked.txt"), "final-worktree\n").unwrap();
    std::fs::write(project.path().join("z.bin"), b"binary\0").unwrap();
    std::fs::write(project.path().join("a.txt"), "added\n").unwrap();
    let ReviewBase::Head { oid } = git.resolve_worktree_base().await.unwrap() else {
        panic!("committed repository must resolve HEAD");
    };

    let capture = git
        .capture_and_persist_head_working_tree_source(&oid, UntrackedSourcePolicy::default())
        .await
        .unwrap();
    assert_eq!(capture.source.base, ReviewBase::Head { oid: oid.clone() });
    assert!(capture.source.includes_unstaged);
    assert!(capture.source.includes_staged);
    assert_eq!(capture.source.included_untracked_files, 2);
    assert_eq!(capture.source.content_digest, capture.artifact.manifest.revision_id);
    let source = String::from_utf8(capture.artifact.canonical_bytes.clone()).unwrap();
    assert_eq!(
        ReviewSourceArtifact::load(project.path(), &capture.artifact.manifest.revision_id)
            .unwrap()
            .canonical_bytes,
        capture.artifact.canonical_bytes
    );
    let parsed = super::super::parse_unified_diff(&source).unwrap();

    assert_eq!(source.matches("diff --git a/tracked.txt b/tracked.txt").count(), 1);
    assert!(source.contains("+final-worktree"));
    assert!(!source.contains("staged-only"));
    assert_eq!(parsed.files.len(), 3);
    assert_eq!(parsed.files[0].new_path, Some(PathBuf::from("tracked.txt")));
    assert_eq!(parsed.files[1].new_path, Some(PathBuf::from("a.txt")));
    assert_eq!(parsed.files[2].new_path, Some(PathBuf::from("z.bin")));
}

#[tokio::test]
async fn untracked_binary_addition_is_explicit_and_has_unknown_line_counts() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    let path = PathBuf::from("asset.bin");
    std::fs::write(project.path().join(&path), b"asset\0").unwrap();

    let diff = git
        .capture_untracked_binary_addition(path.as_path(), UntrackedSourcePolicy::default())
        .await
        .unwrap();
    let parsed = super::super::parse_unified_diff(&diff).unwrap();

    assert_eq!(parsed.files[0].new_path, Some(path));
    assert_eq!(parsed.files[0].status, crate::guided_review::ReviewFileStatus::Added);
    assert_eq!(parsed.files[0].additions, None);
    assert!(parsed.files[0].binary);
}

#[tokio::test]
async fn untracked_classification_respects_size_and_binary_guards() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    std::fs::write(project.path().join("text.txt"), "text\n").unwrap();
    std::fs::write(project.path().join("binary.bin"), b"binary\0").unwrap();
    std::fs::write(project.path().join("large.txt"), "too long").unwrap();

    assert_eq!(
        git.classify_untracked_file(
            PathBuf::from("text.txt").as_path(),
            UntrackedSourcePolicy::default()
        )
        .await
        .unwrap(),
        UntrackedFileKind::Text
    );
    assert_eq!(
        git.classify_untracked_file(
            PathBuf::from("binary.bin").as_path(),
            UntrackedSourcePolicy::default()
        )
        .await
        .unwrap(),
        UntrackedFileKind::Binary
    );
    let error = git
        .classify_untracked_file(
            PathBuf::from("large.txt").as_path(),
            UntrackedSourcePolicy { max_file_bytes: 3 },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, GuidedReviewError::Unavailable(_)));
}

#[tokio::test]
async fn tracked_listing_is_nul_safe_and_repository_relative() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    std::fs::create_dir_all(project.path().join("src")).unwrap();
    std::fs::write(project.path().join("src/main file.rs"), "fn main() {}\n").unwrap();
    git.run("stage tracked file", &["add", "src/main file.rs"]).await.unwrap();

    assert_eq!(
        git.tracked_worktree_paths().await.unwrap(),
        vec![PathBuf::from("src/main file.rs")]
    );
    std::fs::write(project.path().join("note.txt"), "pre-turn\n").unwrap();
    let baseline = git
        .capture_and_persist_turn_baseline("session", "turn", UntrackedSourcePolicy::default())
        .await
        .unwrap();
    assert_eq!(baseline.manifest.base, ReviewBase::EmptyTree);
    assert_eq!(baseline.manifest.files.len(), 2);
    assert_eq!(
        TurnBaselineArtifact::load(project.path(), &baseline.manifest.id).unwrap(),
        baseline
    );

    std::fs::write(project.path().join("src/main file.rs"), "fn main() { println!(\"now\"); }\n")
        .unwrap();
    std::fs::write(project.path().join("note.txt"), "post-turn\n").unwrap();
    std::fs::write(project.path().join("added.txt"), "new\n").unwrap();
    let post_turn = git
        .capture_and_persist_post_turn_snapshot("session", "turn", UntrackedSourcePolicy::default())
        .await
        .unwrap();
    assert!(post_turn.manifest.includes_staged);
    assert!(post_turn.manifest.includes_unstaged);
    assert_eq!(post_turn.manifest.included_untracked_files, 2);
    std::fs::write(project.path().join("later.txt"), "not part of the turn\n").unwrap();
    let capture = git
        .capture_and_persist_last_turn_source("session", "turn", UntrackedSourcePolicy::default())
        .await
        .unwrap();

    assert_eq!(
        capture.source.base,
        ReviewBase::TurnBaseline { revision_id: baseline.revision_id() }
    );
    assert!(capture.source.includes_staged);
    assert!(capture.source.includes_unstaged);
    assert_eq!(capture.source.included_untracked_files, 2);
    let source = String::from_utf8(capture.artifact.canonical_bytes.clone()).unwrap();
    assert!(!source.contains("a/before/"));
    assert!(!source.contains("later.txt"));
    assert_eq!(
        ReviewSourceArtifact::load(project.path(), &capture.artifact.manifest.revision_id)
            .unwrap()
            .canonical_bytes,
        capture.artifact.canonical_bytes
    );
}

#[tokio::test]
async fn snapshot_diff_uses_pinned_bytes_without_a_repository() {
    let root = tempfile::tempdir().unwrap();
    let git = LocalGit::new(root.path());
    let before = TurnBaselineArtifact::capture(
        TurnBaselineId::from_session_turn("session", "before"),
        ReviewBase::EmptyTree,
        1,
        false,
        false,
        0,
        vec![(PathBuf::from("src/lib.rs"), b"pub fn answer() -> u8 { 41 }\n".to_vec())],
    )
    .unwrap();
    let after = TurnBaselineArtifact::capture(
        TurnBaselineId::from_session_turn("session", "after"),
        ReviewBase::EmptyTree,
        2,
        false,
        false,
        0,
        vec![(PathBuf::from("src/lib.rs"), b"pub fn answer() -> u8 { 42 }\n".to_vec())],
    )
    .unwrap();

    let source = git.diff_turn_snapshots(&before, &after).await.unwrap();
    let parsed = parse_unified_diff(&source).unwrap();
    assert_eq!(parsed.files[0].new_path, Some(PathBuf::from("src/lib.rs")));
    assert!(matches!(
        git.capture_and_persist_last_turn_source(
            "missing",
            "turn",
            UntrackedSourcePolicy::default()
        )
        .await,
        Err(GuidedReviewError::Unavailable(ReviewSourceUnavailable::MissingTurnBaseline))
    ));
}

#[tokio::test]
async fn untracked_listing_is_nul_safe_and_excludes_ignored_paths() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    std::fs::write(project.path().join("with space.txt"), "included\n").unwrap();
    std::fs::write(project.path().join("ignored.txt"), "ignored\n").unwrap();
    std::fs::write(project.path().join(".gitignore"), "ignored.txt\n").unwrap();

    assert_eq!(
        git.eligible_untracked_paths().await.unwrap(),
        vec![PathBuf::from(".gitignore"), PathBuf::from("with space.txt")]
    );
}

#[test]
fn normalizes_only_no_index_snapshot_path_headers() {
    let source = "diff --git a/before/src/lib.rs b/after/src/lib.rs\n--- a/before/src/lib.rs\n+++ b/after/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/before/logo.bin b/after/logo.bin\nBinary files a/before/logo.bin and b/after/logo.bin differ\n";
    assert_eq!(
            normalize_no_index_snapshot_paths(source, "before", "after").unwrap(),
            "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/logo.bin b/logo.bin\nBinary files a/logo.bin and b/logo.bin differ\n"
        );
    assert!(matches!(
        normalize_no_index_snapshot_paths(
            "diff --git a/before/file b/wrong/file\n",
            "before",
            "after"
        ),
        Err(GuidedReviewError::MalformedGitOutput { .. })
    ));
}

#[test]
fn malformed_untracked_path_output_is_rejected() {
    let error = parse_untracked_paths(b"../escape\0").unwrap_err();

    assert!(matches!(error, GuidedReviewError::MalformedGitOutput { .. }));
}

#[tokio::test]
async fn provenance_marks_staged_and_unstaged_changes_without_a_patch() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    std::fs::write(project.path().join("tracked.txt"), "base\n").unwrap();
    git.run("stage initial file", &["add", "tracked.txt"]).await.unwrap();
    git.run(
        "create initial commit",
        &[
            "-c",
            "user.name=Luxe Tests",
            "-c",
            "user.email=tests@luxe.invalid",
            "commit",
            "--quiet",
            "-m",
            "initial",
        ],
    )
    .await
    .unwrap();
    std::fs::write(project.path().join("tracked.txt"), "staged\n").unwrap();
    git.run("stage change", &["add", "tracked.txt"]).await.unwrap();
    std::fs::write(project.path().join("tracked.txt"), "unstaged\n").unwrap();

    assert_eq!(
        git.worktree_provenance().await.unwrap(),
        WorktreeProvenance { includes_staged: true, includes_unstaged: true }
    );
}

#[tokio::test]
async fn index_delete_and_worktree_recreate_remains_a_final_state_noop() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    std::fs::write(project.path().join("recreated.txt"), "same\n").unwrap();
    git.run("stage initial file", &["add", "recreated.txt"]).await.unwrap();
    git.run(
        "create initial commit",
        &[
            "-c",
            "user.name=Luxe Tests",
            "-c",
            "user.email=tests@luxe.invalid",
            "commit",
            "--quiet",
            "-m",
            "initial",
        ],
    )
    .await
    .unwrap();
    git.run("remove path from index", &["rm", "--cached", "recreated.txt"]).await.unwrap();
    let ReviewBase::Head { oid } = git.resolve_worktree_base().await.unwrap() else {
        panic!("committed worktree must resolve HEAD");
    };

    let capture = git
        .capture_and_persist_head_working_tree_source(&oid, UntrackedSourcePolicy::default())
        .await
        .unwrap();

    assert!(capture.artifact.canonical_bytes.is_empty());
    assert_eq!(capture.source.included_untracked_files, 0);
    let status =
        String::from_utf8(git.run("read status", &["status", "--short"]).await.unwrap()).unwrap();
    assert!(status.contains("D  recreated.txt\n"));
    assert!(status.contains("?? recreated.txt\n"));
}

#[tokio::test]
async fn tracked_capture_compares_the_head_to_the_final_worktree_state() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    std::fs::write(project.path().join("tracked.txt"), "before\n").unwrap();
    git.run("stage initial file", &["add", "tracked.txt"]).await.unwrap();
    git.run(
        "create initial commit",
        &[
            "-c",
            "user.name=Luxe Tests",
            "-c",
            "user.email=tests@luxe.invalid",
            "commit",
            "--quiet",
            "-m",
            "initial",
        ],
    )
    .await
    .unwrap();
    let ReviewBase::Head { oid } = git.resolve_worktree_base().await.unwrap() else {
        panic!("committed worktree must resolve HEAD");
    };
    std::fs::write(project.path().join("tracked.txt"), "after\n").unwrap();

    let diff = String::from_utf8(git.capture_tracked_diff(&oid).await.unwrap()).unwrap();

    assert!(diff.contains("diff --git a/tracked.txt b/tracked.txt"));
    assert!(diff.contains("-before"));
    assert!(diff.contains("+after"));
}

#[test]
fn stable_capture_rejects_a_changed_source_or_provenance() {
    let initial = CapturedWorktreeSource {
        provenance: WorktreeProvenance { includes_staged: false, includes_unstaged: true },
        canonical_source: "initial source".into(),
        included_untracked_files: 0,
        uncovered_paths: Vec::new(),
    };
    let changed_source = CapturedWorktreeSource {
        provenance: initial.provenance,
        canonical_source: "changed source".into(),
        included_untracked_files: 0,
        uncovered_paths: Vec::new(),
    };
    let changed_provenance = CapturedWorktreeSource {
        provenance: WorktreeProvenance { includes_staged: true, ..initial.provenance },
        canonical_source: initial.canonical_source.clone(),
        included_untracked_files: 0,
        uncovered_paths: Vec::new(),
    };

    assert!(matches!(
        ensure_stable_worktree_capture(initial, changed_source),
        Err(GuidedReviewError::Unavailable(ReviewSourceUnavailable::StaleCapture))
    ));
    assert!(matches!(
        ensure_stable_worktree_capture(
            CapturedWorktreeSource {
                provenance: WorktreeProvenance { includes_staged: false, includes_unstaged: true },
                canonical_source: "initial source".into(),
                included_untracked_files: 0,
                uncovered_paths: Vec::new(),
            },
            changed_provenance,
        ),
        Err(GuidedReviewError::Unavailable(ReviewSourceUnavailable::StaleCapture))
    ));
}

#[tokio::test]
async fn unborn_worktree_resolves_to_the_empty_tree() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();

    assert_eq!(git.resolve_worktree_base().await.unwrap(), ReviewBase::EmptyTree);
    std::fs::write(project.path().join("first.txt"), "first\n").unwrap();

    let capture = git
        .capture_and_persist_working_tree_source(UntrackedSourcePolicy::default())
        .await
        .unwrap();

    assert_eq!(capture.source.base, ReviewBase::EmptyTree);
    assert_eq!(capture.source.included_untracked_files, 1);
    assert_eq!(
        super::super::parse_unified_diff(
            std::str::from_utf8(&capture.artifact.canonical_bytes).unwrap()
        )
        .unwrap()
        .files[0]
            .new_path,
        Some(PathBuf::from("first.txt"))
    );
}

/// Luxe's own directory is a byproduct of the tool running, never the user's change.
///
/// It arrived in reviews two ways: as an untracked path (`.luxe/.gitignore` used to un-ignore
/// itself, which is all Git needs to report the whole directory), and — for anyone who committed
/// it once — as tracked churn in the diff. Neither reaches a review now.
#[tokio::test]
async fn luxes_own_directory_never_reaches_a_review() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    std::fs::write(project.path().join("src.rs"), "fn main() {}\n").unwrap();
    std::fs::create_dir_all(project.path().join(".luxe/screenshots")).unwrap();
    std::fs::write(project.path().join(".luxe/.gitignore"), "*\n").unwrap();
    std::fs::write(project.path().join(".luxe/screenshots/shot.txt"), "not code\n").unwrap();

    let untracked = git.eligible_untracked_paths().await.unwrap();

    assert!(untracked.iter().any(|path| path == std::path::Path::new("src.rs")));
    assert!(
        !untracked.iter().any(|path| path.starts_with(".luxe")),
        "luxe's own artifacts are not the user's work: {untracked:?}"
    );
}

/// An untracked file too big to hold, or not a regular file, costs the review that file and not
/// the review: it is left out and named, as the last turn's review names what it could not cover.
#[tokio::test]
async fn an_untracked_file_the_policy_cannot_hold_is_left_out_and_named() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    std::fs::write(project.path().join("tracked.txt"), "base\n").unwrap();
    git.run("stage initial file", &["add", "tracked.txt"]).await.unwrap();
    git.run(
        "commit initial file",
        &["-c", "user.name=Luxe", "-c", "user.email=luxe@example.invalid", "commit", "-qm", "base"],
    )
    .await
    .unwrap();
    std::fs::write(project.path().join("tracked.txt"), "changed\n").unwrap();
    std::fs::write(project.path().join("note.txt"), "small\n").unwrap();
    std::fs::write(project.path().join("dump.log"), "far more than sixteen bytes\n").unwrap();

    let policy = UntrackedSourcePolicy { max_file_bytes: 16 };
    let capture = git.capture_and_persist_working_tree_source(policy).await.unwrap();

    let source = String::from_utf8(capture.artifact.canonical_bytes.clone()).unwrap();
    assert!(source.contains("+changed") && source.contains("b/note.txt"), "{source}");
    assert!(!source.contains("dump.log"), "{source}");
    assert_eq!(capture.source.included_untracked_files, 1);
    assert_eq!(capture.source.uncovered_paths, vec![PathBuf::from("dump.log")]);
}

/// A new symlink is not a regular file, so it is left out and named rather than followed.
#[cfg(unix)]
#[tokio::test]
async fn an_untracked_symlink_is_left_out_and_named() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    std::fs::write(project.path().join("note.txt"), "small\n").unwrap();
    std::os::unix::fs::symlink("note.txt", project.path().join("link.txt")).unwrap();

    let capture = git
        .capture_and_persist_working_tree_source(UntrackedSourcePolicy::default())
        .await
        .unwrap();

    assert_eq!(capture.source.included_untracked_files, 1);
    assert_eq!(capture.source.uncovered_paths, vec![PathBuf::from("link.txt")]);
}
