//! Tests — pinning a pull request from a remote, without touching the checkout.

use super::super::*;
use std::path::Path;

/// Runs git in `dir`, failing the test with git's own words when it does not succeed.
fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn identify(dir: &Path) {
    git(dir, &["config", "user.name", "luxe Tests"]);
    git(dir, &["config", "user.email", "tests@luxe.local"]);
}

fn commit(dir: &Path, file: &str, contents: &str, subject: &str) -> String {
    std::fs::write(dir.join(file), contents).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "--quiet", "-m", subject]);
    git(dir, &["rev-parse", "HEAD"])
}

/// A remote holding a pull request the checkout has never seen.
///
/// `main` moves on after the pull request branches off, so a diff taken against the base tip would
/// show `main`'s own change as if the pull request had reverted it. The merge base is the only
/// comparison that shows the pull request alone.
struct Remote {
    _dir: tempfile::TempDir,
    seed: std::path::PathBuf,
    project: std::path::PathBuf,
    branch_point: String,
    base_tip: String,
    head: String,
}

impl Remote {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin.git");
        let seed = dir.path().join("seed");
        let project = dir.path().join("project");
        git(dir.path(), &["init", "--quiet", "--bare", "-b", "main", origin.to_str().unwrap()]);
        git(dir.path(), &["init", "--quiet", "-b", "main", seed.to_str().unwrap()]);
        identify(&seed);
        git(&seed, &["remote", "add", "origin", origin.to_str().unwrap()]);
        commit(&seed, "lib.rs", "let a = 1;\n", "Start");
        std::fs::write(seed.join("notes.md"), "notes\n").unwrap();
        git(&seed, &["add", "notes.md"]);
        git(&seed, &["commit", "--quiet", "-m", "Add notes"]);
        let branch_point = git(&seed, &["rev-parse", "HEAD"]);
        git(&seed, &["push", "--quiet", "origin", "main"]);
        git(dir.path(), &["clone", "--quiet", origin.to_str().unwrap(), project.to_str().unwrap()]);
        // What a real session finds: the CLI drops a self-ignoring `.luxe/` at startup, and a review
        // keeps its pinned source there
        std::fs::create_dir(project.join(".luxe")).unwrap();
        std::fs::write(project.join(".luxe/.gitignore"), "*\n").unwrap();

        git(&seed, &["switch", "--quiet", "-c", "feature"]);
        let head = commit(&seed, "lib.rs", "let a = 2;\n", "Raise the limit");
        git(&seed, &["push", "--quiet", "origin", "HEAD:refs/pull/1/head"]);
        git(&seed, &["switch", "--quiet", "main"]);
        let base_tip = commit(&seed, "notes.md", "notes, revised\n", "Revise notes");
        git(&seed, &["push", "--quiet", "origin", "main"]);
        Self { _dir: dir, seed, project, branch_point, base_tip, head }
    }

    fn pin(&self) -> PullRequestPin {
        PullRequestPin {
            number: 1,
            repository: "acme/app".into(),
            fetch_from: "origin".into(),
            base_ref: "main".into(),
            base_oid: self.base_tip.clone(),
            head_ref: "feature".into(),
            head_oid: self.head.clone(),
        }
    }

    /// Everything a reader could notice changing in the checkout.
    fn checkout_state(&self) -> (String, String, String, bool) {
        (
            git(&self.project, &["rev-parse", "HEAD"]),
            git(&self.project, &["status", "--porcelain"]),
            git(&self.project, &["for-each-ref", "--format=%(refname) %(objectname)"]),
            self.project.join(".git").join("FETCH_HEAD").exists(),
        )
    }
}

async fn load(
    project: &Path,
    pin: Option<PullRequestPin>,
) -> Result<GuidedReview, GuidedReviewError> {
    let git = LocalGit::new(project);
    let git = match pin {
        Some(pin) => git.with_pull_request(pin),
        None => git,
    };
    git.load_target_review(
        ReviewId::new(),
        ReviewTarget::PullRequest { number: 1, repository: None },
        crate::guided_review::ReviewContextDraft { title: "PR".into(), ..Default::default() }
            .build(),
        UntrackedSourcePolicy::default(),
    )
    .await
}

fn paths(review: &GuidedReview) -> Vec<String> {
    review
        .files
        .iter()
        .filter_map(|file| file.new_path.as_ref().map(|path| path.display().to_string()))
        .collect()
}

/// The pull request's own change and nothing of `main`'s, with the exact commits named, and the
/// checkout exactly as it was: same `HEAD`, clean status, no new ref, no `FETCH_HEAD`.
#[tokio::test]
async fn a_pull_request_is_pinned_from_its_merge_base_without_touching_the_checkout() {
    let remote = Remote::new();
    let before = remote.checkout_state();

    let review = load(&remote.project, Some(remote.pin())).await.expect("the PR loads");

    assert_eq!(paths(&review), vec!["lib.rs"], "main's own change is not the PR's");
    assert_eq!(
        review.source.base,
        ReviewBase::PullRequest {
            number: 1,
            repository: "acme/app".into(),
            base_ref: "main".into(),
            head_ref: "feature".into(),
            merge_base: remote.branch_point.clone(),
            head_oid: remote.head.clone(),
        }
    );
    assert_eq!(review.context.title, "Raise the limit", "the commits in range name the change");
    assert_eq!(review.context.source_commits, 1, "counted from the range, not from main");
    assert_eq!(remote.checkout_state(), before, "the checkout is untouched");
}

/// Commits already here are not fetched again, so a second look works with the remote gone.
#[tokio::test]
async fn a_second_look_at_the_same_revision_needs_no_network() {
    let remote = Remote::new();
    load(&remote.project, Some(remote.pin())).await.expect("the first load fetches");
    git(&remote.project, &["remote", "set-url", "origin", "/nonexistent/luxe/remote.git"]);

    let review = load(&remote.project, Some(remote.pin())).await.expect("no fetch needed");

    assert_eq!(paths(&review), vec!["lib.rs"]);
}

/// A push is a new revision, never a change under an open review.
#[tokio::test]
async fn a_push_between_two_looks_is_a_new_revision() {
    let remote = Remote::new();
    let first = load(&remote.project, Some(remote.pin())).await.unwrap();

    git(&remote.seed, &["switch", "--quiet", "feature"]);
    let pushed = commit(&remote.seed, "extra.rs", "let b = 1;\n", "Add a helper");
    git(&remote.seed, &["push", "--quiet", "origin", "HEAD:refs/pull/1/head"]);
    let second = load(&remote.project, Some(PullRequestPin { head_oid: pushed, ..remote.pin() }))
        .await
        .unwrap();

    assert_ne!(first.revision_id, second.revision_id);
    assert_eq!(paths(&second), vec!["extra.rs", "lib.rs"]);
}

/// A head the remote does not have is a fetch failure that says so, not an empty review.
#[tokio::test]
async fn a_head_the_remote_does_not_have_is_a_fetch_failure() {
    let remote = Remote::new();
    let missing = "0123456789abcdef0123456789abcdef01234567".to_string();

    let error = load(&remote.project, Some(PullRequestPin { head_oid: missing, ..remote.pin() }))
        .await
        .expect_err("nothing to review");

    assert!(
        matches!(
            error,
            GuidedReviewError::Unavailable(ReviewSourceUnavailable::PullRequestFetch {
                number: 1,
                ref detail,
            }) if !detail.is_empty()
        ),
        "{error:?}"
    );
}

/// A target that reaches the capture unresolved is refused rather than guessed at, and so is a pin
/// for a different pull request.
#[tokio::test]
async fn an_unresolved_pull_request_is_refused() {
    let remote = Remote::new();
    for pin in [None, Some(PullRequestPin { number: 2, ..remote.pin() })] {
        let error = load(&remote.project, pin).await.expect_err("unresolved");
        assert!(
            matches!(
                error,
                GuidedReviewError::Unavailable(ReviewSourceUnavailable::PullRequestNotResolved {
                    number: 1
                })
            ),
            "{error:?}"
        );
    }
}

/// A SHA from GitHub is still checked before it reaches git's argument list.
#[tokio::test]
async fn a_malformed_commit_id_never_reaches_git() {
    let remote = Remote::new();
    let error = load(
        &remote.project,
        Some(PullRequestPin { head_oid: "--upload-pack=touch /tmp/pwned".into(), ..remote.pin() }),
    )
    .await
    .expect_err("refused");
    assert!(matches!(error, GuidedReviewError::InvalidReference { .. }), "{error:?}");
}

/// A pull request is captured in Git's own rendering whatever the checkout's config says.
///
/// The break this catches: the capture ran plain `git diff`, so `color.ui=always` put escape codes
/// in the pinned source and the review failed to parse.
#[tokio::test]
async fn a_pull_request_capture_ignores_the_checkouts_diff_config() {
    let remote = Remote::new();
    git(&remote.project, &["config", "color.ui", "always"]);
    git(&remote.project, &["config", "diff.external", "false"]);

    let review = load(&remote.project, Some(remote.pin())).await.expect("the PR loads");

    assert_eq!(paths(&review), vec!["lib.rs"]);
    let added: Vec<&str> = review.files[0].hunks[0]
        .lines
        .iter()
        .filter(|line| line.kind == crate::guided_review::ReviewLineKind::Addition)
        .map(|line| line.text.as_str())
        .collect();
    assert_eq!(added, ["let a = 2;"]);
}
