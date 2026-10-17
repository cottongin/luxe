//! Tests — target dispatch, retention, base/commit resolution.

use super::super::*;

#[tokio::test]
async fn target_dispatch_captures_the_requested_working_tree_source() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    std::fs::write(project.path().join("first.txt"), "first\n").unwrap();

    let review = git
        .load_target_review(
            ReviewId::new(),
            ReviewTarget::WorkingTree,
            ReviewContext {
                title: "Review".into(),
                goal: None,
                phase_index: Some(30),
                phase_title: Some("Guided review".into()),
                planning_excerpt: None,
                acceptance_excerpt: None,
                source_intent: None,
                source_commits: 0,
                pull_request: None,
            },
            UntrackedSourcePolicy::default(),
        )
        .await
        .unwrap();

    assert_eq!(review.source.base, ReviewBase::EmptyTree);
    assert_eq!(review.source.included_untracked_files, 1);
    assert_eq!(review.files[0].new_path, Some(PathBuf::from("first.txt")));

    let expected_revision = review.revision_id.clone();
    std::fs::write(project.path().join("first.txt"), "second\n").unwrap();
    let refreshed = git
        .refresh_target_review(
            &review,
            &expected_revision,
            review.context.clone(),
            UntrackedSourcePolicy::default(),
        )
        .await
        .unwrap();
    assert_eq!(refreshed.id, review.id);
    assert_ne!(refreshed.revision_id, expected_revision);
    assert_eq!(refreshed.files.len(), 1);
    assert_eq!(refreshed.files[0].new_path, Some(PathBuf::from("first.txt")));

    let stale = ReviewRevisionId::from_canonical_bytes(b"stale");
    let error = git
        .refresh_target_review(
            &review,
            &stale,
            review.context.clone(),
            UntrackedSourcePolicy::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, GuidedReviewError::StaleRevision { .. }));
}

#[tokio::test]
async fn retention_prunes_unprotected_sources_but_preserves_refresh_revisions() {
    let project = tempfile::tempdir().unwrap();
    let initial_git = LocalGit::new(project.path());
    initial_git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    std::fs::write(project.path().join("first.txt"), "first\n").unwrap();
    let context = ReviewContext {
        title: "Review".into(),
        goal: None,
        phase_index: None,
        phase_title: None,
        planning_excerpt: None,
        acceptance_excerpt: None,
        source_intent: None,
        source_commits: 0,
        pull_request: None,
    };
    let initial = initial_git
        .load_target_review(
            ReviewId::new(),
            ReviewTarget::WorkingTree,
            context.clone(),
            UntrackedSourcePolicy::default(),
        )
        .await
        .unwrap();
    let unprotected = ReviewSourceArtifact::capture(b"unprotected artifact".to_vec());
    unprotected.persist(project.path()).unwrap();

    std::fs::write(project.path().join("first.txt"), "second\n").unwrap();
    let constrained_git =
        LocalGit::new(project.path()).with_source_retention_policy(SourceRetentionPolicy {
            max_age_ms: None,
            max_total_bytes: 0,
        });
    let refreshed = constrained_git
        .refresh_target_review(
            &initial,
            &initial.revision_id,
            context,
            UntrackedSourcePolicy::default(),
        )
        .await
        .unwrap();

    assert!(matches!(
        ReviewSourceArtifact::load(project.path(), &unprotected.manifest.revision_id),
        Err(SourceStoreError::Io { error: source, .. }) if source.kind() == io::ErrorKind::NotFound
    ));
    assert!(ReviewSourceArtifact::load(project.path(), &initial.revision_id).is_ok());
    assert!(ReviewSourceArtifact::load(project.path(), &refreshed.revision_id).is_ok());
}

#[tokio::test]
async fn retention_keeps_visible_revision_while_loading_distinct_target() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    git.run("configure author", &["config", "user.email", "review@example.test"]).await.unwrap();
    git.run("configure author", &["config", "user.name", "Review Test"]).await.unwrap();
    std::fs::write(project.path().join("source.txt"), "base\n").unwrap();
    git.run("stage base", &["add", "source.txt"]).await.unwrap();
    git.run("commit base", &["commit", "--quiet", "-m", "base"]).await.unwrap();
    let oid = String::from_utf8(git.run("resolve HEAD", &["rev-parse", "HEAD"]).await.unwrap())
        .unwrap()
        .trim()
        .to_owned();
    std::fs::write(project.path().join("source.txt"), "visible review\n").unwrap();
    let context = ReviewContext {
        title: "Review".into(),
        goal: None,
        phase_index: None,
        phase_title: None,
        planning_excerpt: None,
        acceptance_excerpt: None,
        source_intent: None,
        source_commits: 0,
        pull_request: None,
    };
    let visible = git
        .load_target_review(
            ReviewId::new(),
            ReviewTarget::WorkingTree,
            context.clone(),
            UntrackedSourcePolicy::default(),
        )
        .await
        .unwrap();

    let replacement = LocalGit::new(project.path())
        .with_source_retention_policy(SourceRetentionPolicy {
            max_age_ms: None,
            max_total_bytes: 0,
        })
        .preserving_revision(visible.revision_id.clone())
        .load_target_review(
            ReviewId::new(),
            ReviewTarget::Commit { oid },
            context,
            UntrackedSourcePolicy::default(),
        )
        .await
        .unwrap();

    assert!(ReviewSourceArtifact::load(project.path(), &visible.revision_id).is_ok());
    assert!(ReviewSourceArtifact::load(project.path(), &replacement.revision_id).is_ok());
}

#[tokio::test]
async fn base_target_resolves_a_local_merge_base_and_rejects_unsafe_input() {
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
    let ReviewBase::Head { oid } = git.resolve_worktree_base().await.unwrap() else {
        panic!("committed worktree must resolve HEAD");
    };

    assert_eq!(
        git.resolve_base_merge_base("HEAD").await.unwrap(),
        ReviewBase::MergeBase { reference: "HEAD".into(), oid: oid.clone() }
    );
    let capture = git
        .capture_and_persist_base_working_tree_source("HEAD", UntrackedSourcePolicy::default())
        .await
        .unwrap();
    assert_eq!(capture.source.base, ReviewBase::MergeBase { reference: "HEAD".into(), oid });
    assert!(capture.artifact.canonical_bytes.is_empty());
    assert!(matches!(
        git.resolve_base_merge_base("bad\nref").await,
        Err(GuidedReviewError::InvalidReference { .. })
    ));
}

#[tokio::test]
async fn target_dispatch_rejects_unsafe_base_and_commit_references() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    let policy = UntrackedSourcePolicy::default();

    for target in [
        ReviewTarget::Base { reference: "bad\nref".into() },
        ReviewTarget::Commit { oid: "bad\ncommit".into() },
    ] {
        assert!(matches!(
            git.capture_and_persist_target_source(&target, policy).await,
            Err(GuidedReviewError::InvalidReference { .. })
        ));
    }
}

#[tokio::test]
async fn commit_target_records_first_parent_and_root_semantics() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    std::fs::write(project.path().join("tracked.txt"), "one\n").unwrap();
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
    let ReviewBase::Head { oid: root_oid } = git.resolve_worktree_base().await.unwrap() else {
        panic!("committed worktree must resolve HEAD");
    };
    assert_eq!(
        git.resolve_commit_first_parent(&root_oid).await.unwrap(),
        ReviewBase::CommitParent { commit_oid: root_oid.clone(), parent_oid: None }
    );
    let root_capture = git.capture_and_persist_commit_source(&root_oid).await.unwrap();
    assert_eq!(
        root_capture.source.base,
        ReviewBase::CommitParent { commit_oid: root_oid.clone(), parent_oid: None }
    );
    let root_diff = std::str::from_utf8(&root_capture.artifact.canonical_bytes).unwrap();
    assert!(root_diff.contains("+one"));
    assert!(!root_diff.contains("-one"));

    std::fs::write(project.path().join("tracked.txt"), "two\n").unwrap();
    git.run("stage follow-up file", &["add", "tracked.txt"]).await.unwrap();
    git.run(
        "create follow-up commit",
        &[
            "-c",
            "user.name=Luxe Tests",
            "-c",
            "user.email=tests@luxe.invalid",
            "commit",
            "--quiet",
            "-m",
            "follow-up",
        ],
    )
    .await
    .unwrap();
    let ReviewBase::Head { oid: commit_oid } = git.resolve_worktree_base().await.unwrap() else {
        panic!("committed worktree must resolve HEAD");
    };
    assert_eq!(
        git.resolve_commit_first_parent(&commit_oid).await.unwrap(),
        ReviewBase::CommitParent {
            commit_oid: commit_oid.clone(),
            parent_oid: Some(root_oid.clone())
        }
    );
    let capture = git.capture_and_persist_commit_source(&commit_oid).await.unwrap();
    assert_eq!(
        capture.source.base,
        ReviewBase::CommitParent { commit_oid, parent_oid: Some(root_oid) }
    );
    let diff = std::str::from_utf8(&capture.artifact.canonical_bytes).unwrap();
    assert!(diff.contains("-one"));
    assert!(diff.contains("+two"));
}

#[tokio::test]
async fn merge_commit_target_uses_only_its_first_parent() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    std::fs::write(project.path().join("base.txt"), "base\n").unwrap();
    git.run("stage base", &["add", "base.txt"]).await.unwrap();
    git.run(
        "commit base",
        &[
            "-c",
            "user.name=Luxe Tests",
            "-c",
            "user.email=tests@luxe.invalid",
            "commit",
            "--quiet",
            "-m",
            "base",
        ],
    )
    .await
    .unwrap();
    git.run("create feature branch", &["checkout", "--quiet", "-b", "feature"]).await.unwrap();
    std::fs::write(project.path().join("feature.txt"), "feature\n").unwrap();
    git.run("stage feature", &["add", "feature.txt"]).await.unwrap();
    git.run(
        "commit feature",
        &[
            "-c",
            "user.name=Luxe Tests",
            "-c",
            "user.email=tests@luxe.invalid",
            "commit",
            "--quiet",
            "-m",
            "feature",
        ],
    )
    .await
    .unwrap();
    git.run("return to default branch", &["checkout", "--quiet", "-"]).await.unwrap();
    std::fs::write(project.path().join("main.txt"), "main\n").unwrap();
    git.run("stage main", &["add", "main.txt"]).await.unwrap();
    git.run(
        "commit main",
        &[
            "-c",
            "user.name=Luxe Tests",
            "-c",
            "user.email=tests@luxe.invalid",
            "commit",
            "--quiet",
            "-m",
            "main",
        ],
    )
    .await
    .unwrap();
    let ReviewBase::Head { oid: first_parent } = git.resolve_worktree_base().await.unwrap() else {
        panic!("main commit must resolve HEAD");
    };
    git.run(
        "merge feature",
        &[
            "-c",
            "user.name=Luxe Tests",
            "-c",
            "user.email=tests@luxe.invalid",
            "merge",
            "--quiet",
            "--no-ff",
            "--no-edit",
            "feature",
        ],
    )
    .await
    .unwrap();
    let ReviewBase::Head { oid: merge_oid } = git.resolve_worktree_base().await.unwrap() else {
        panic!("merge commit must resolve HEAD");
    };

    assert_eq!(
        git.resolve_commit_first_parent(&merge_oid).await.unwrap(),
        ReviewBase::CommitParent { commit_oid: merge_oid.clone(), parent_oid: Some(first_parent) }
    );
    let capture = git.capture_and_persist_commit_source(&merge_oid).await.unwrap();
    let diff = std::str::from_utf8(&capture.artifact.canonical_bytes).unwrap();
    assert!(diff.contains("+feature"));
    assert!(!diff.contains("+main"));
}

/// A commit-backed review used to open under the words "Guided Review" with an empty Brief, while
/// the commit messages — the only authored intent that exists for finished work — sat unread.
#[tokio::test]
async fn a_commit_review_names_itself_from_the_commit_it_reviews() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    std::fs::write(project.path().join("lib.rs"), "let a = 1;\n").unwrap();
    git.run("stage initial file", &["add", "."]).await.unwrap();
    commit(&git, "seed the module").await;
    std::fs::write(project.path().join("lib.rs"), "let a = 2;\n").unwrap();
    git.run("stage change", &["add", "."]).await.unwrap();
    commit(&git, "Bound the retry loop\n\nAn unbounded retry hammered the provider on a 429.")
        .await;
    let ReviewBase::Head { oid } = git.resolve_worktree_base().await.unwrap() else {
        panic!("committed worktree must resolve HEAD");
    };

    let review = git
        .load_target_review(
            ReviewId::new(),
            ReviewTarget::Commit { oid: oid.clone() },
            generic_context(),
            UntrackedSourcePolicy::default(),
        )
        .await
        .expect("the commit loads");

    assert_eq!(review.context.title, "Bound the retry loop");
    let intent = review.context.source_intent.as_deref().expect("the commit speaks for itself");
    assert!(intent.contains("hammered the provider"), "body carried: {intent}");
    // The subject is already the title; repeating it as a bullet would be a heading with an echo.
    assert!(!intent.contains("- Bound the retry loop"), "subject not echoed: {intent}");
    assert_eq!(review.context.source_commits, 1, "read from the reviewed range only");
}

/// A one-commit review whose message is just a subject has nothing left to say, so the section must
/// not render as a heading over a repeat of the title.
#[tokio::test]
async fn a_subject_only_commit_adds_no_intent_section() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    std::fs::write(project.path().join("lib.rs"), "let a = 1;\n").unwrap();
    git.run("stage initial file", &["add", "."]).await.unwrap();
    commit(&git, "seed the module").await;
    std::fs::write(project.path().join("lib.rs"), "let a = 2;\n").unwrap();
    git.run("stage change", &["add", "."]).await.unwrap();
    commit(&git, "Bound the retry loop").await;
    let ReviewBase::Head { oid } = git.resolve_worktree_base().await.unwrap() else {
        panic!("committed worktree must resolve HEAD");
    };

    let review = git
        .load_target_review(
            ReviewId::new(),
            ReviewTarget::Commit { oid },
            generic_context(),
            UntrackedSourcePolicy::default(),
        )
        .await
        .expect("the commit loads");

    assert_eq!(review.context.title, "Bound the retry loop", "the subject still names the review");
    assert!(review.context.source_intent.is_none(), "nothing to say beyond the title");
    assert_eq!(review.context.source_commits, 1);
}

/// A range review lists what it covers, and says how many commits that is — the question "which
/// commits?" has to be answerable from the Brief alone.
#[tokio::test]
async fn a_range_review_lists_and_counts_its_commits() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    std::fs::write(project.path().join("lib.rs"), "let a = 1;\n").unwrap();
    git.run("stage initial file", &["add", "."]).await.unwrap();
    commit(&git, "seed the module").await;
    let base = git.run("read the base oid", &["rev-parse", "HEAD"]).await.unwrap();
    let base = String::from_utf8_lossy(&base).trim().to_owned();
    for (index, subject) in ["Bound the retry loop", "Redact the log head"].iter().enumerate() {
        std::fs::write(project.path().join("lib.rs"), format!("let a = {};\n", index + 2)).unwrap();
        git.run("stage change", &["add", "."]).await.unwrap();
        commit(&git, subject).await;
    }

    let review = git
        .load_target_review(
            ReviewId::new(),
            ReviewTarget::Base { reference: base },
            generic_context(),
            UntrackedSourcePolicy::default(),
        )
        .await
        .expect("the range loads");

    let intent = review.context.source_intent.as_deref().expect("the range speaks for itself");
    assert!(intent.contains("- Redact the log head"), "newest listed: {intent}");
    assert!(intent.contains("- Bound the retry loop"), "older listed: {intent}");
    // Two, not three: the seed commit is the base, outside the reviewed range.
    assert_eq!(review.context.source_commits, 2, "the range, not the project's history");
    assert!(
        review.context.title.contains("(+1 more)"),
        "title says more rides along: {}",
        review.context.title
    );
}

/// A working-tree review has a session to explain it, so it must not be renamed after whatever
/// happened to be committed last.
#[tokio::test]
async fn a_working_tree_review_keeps_its_own_title() {
    let project = tempfile::tempdir().unwrap();
    let git = LocalGit::new(project.path());
    git.run("initialize repository", &["init", "--quiet"]).await.unwrap();
    std::fs::write(project.path().join("lib.rs"), "let a = 1;\n").unwrap();
    git.run("stage initial file", &["add", "."]).await.unwrap();
    commit(&git, "seed the module").await;
    std::fs::write(project.path().join("lib.rs"), "let a = 2;\n").unwrap();

    let review = git
        .load_target_review(
            ReviewId::new(),
            ReviewTarget::WorkingTree,
            generic_context(),
            UntrackedSourcePolicy::default(),
        )
        .await
        .expect("the working tree loads");

    assert_eq!(review.context.title, "Guided Review");
    assert!(review.context.source_intent.is_none());
}

fn generic_context() -> ReviewContext {
    crate::guided_review::ReviewContextDraft { title: "Guided Review".into(), ..Default::default() }
        .build()
}

async fn commit(git: &LocalGit, message: &str) {
    git.run(
        "create commit",
        &[
            "-c",
            "user.name=Luxe Tests",
            "-c",
            "user.email=tests@luxe.invalid",
            "commit",
            "--quiet",
            "-m",
            message,
        ],
    )
    .await
    .unwrap();
}
