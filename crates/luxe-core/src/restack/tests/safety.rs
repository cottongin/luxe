//! Work done here between the preview and the push is never thrown away, and a failure after the
//! push says what moved.

use super::*;

/// The fix-low-in-the-stack scenario with `two` checked out in a worktree and `three` a local
/// branch behind GitHub; answers the plan and the worktree.
async fn planned_with_checkout() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, Planned) {
    let (dir, project, origin) = stacked();
    push_to(&origin, "one", "fix.txt", "fixed\n");
    let wt = dir.path().join("wt-two");
    sh(&project, &["worktree", "add", "-q", wt.to_str().unwrap(), "two"]);
    let planned = plan(&project, "origin", &layers(), None).await.unwrap();
    assert_eq!(planned.steps.len(), 2, "two and three move");
    (dir, project, origin, wt, planned)
}

/// Every ref in a repository, for "nothing changed" checks.
fn refs(repo: &Path) -> String {
    sh(repo, &["for-each-ref", "--format=%(refname) %(objectname)"])
}

/// A refusal before publication: nothing on the remote moved, nothing here moved.
fn assert_refused(outcome: Result<(), ApplyError>, origin_before: &str, origin: &Path) {
    assert!(matches!(outcome, Err(ApplyError::Refused(_))), "{outcome:?}");
    assert_eq!(refs(origin), origin_before, "nothing was pushed");
}

/// A commit made in the checkout after the preview is not reset away.
#[tokio::test]
async fn a_commit_after_planning_refuses_and_keeps_it() {
    let (_dir, project, origin, wt, planned) = planned_with_checkout().await;
    let made = commit(&wt, "later.txt", "later\n", "Later");
    let before = refs(&origin);
    assert_refused(apply(&project, &planned).await, &before, &origin);
    assert_eq!(sh(&wt, &["rev-parse", "HEAD"]), made);
    assert_eq!(std::fs::read_to_string(wt.join("later.txt")).unwrap(), "later\n");
}

/// A local branch nobody has checked out, moved after the preview, is not replaced.
#[tokio::test]
async fn a_moved_unchecked_branch_refuses_and_keeps_it() {
    let (_dir, project, origin, _wt, planned) = planned_with_checkout().await;
    sh(&project, &["branch", "-qf", "three", "three~1"]);
    let moved = sh(&project, &["rev-parse", "three"]);
    let before = refs(&origin);
    assert_refused(apply(&project, &planned).await, &before, &origin);
    assert_eq!(sh(&project, &["rev-parse", "three"]), moved);
}

/// A checkout switched to another branch after the preview: that other branch is not reset.
#[tokio::test]
async fn a_switched_checkout_refuses_and_keeps_the_other_branch() {
    let (_dir, project, origin, wt, planned) = planned_with_checkout().await;
    sh(&wt, &["switch", "-qc", "side"]);
    let side = sh(&wt, &["rev-parse", "HEAD"]);
    let before = refs(&origin);
    assert_refused(apply(&project, &planned).await, &before, &origin);
    assert_eq!(sh(&project, &["rev-parse", "side"]), side);
}

/// A branch checked out after the preview, which the plan did not know to move, is refused.
#[tokio::test]
async fn a_new_checkout_after_planning_refuses() {
    let (dir, project, origin, _wt, planned) = planned_with_checkout().await;
    let wt3 = dir.path().join("wt-three");
    sh(&project, &["worktree", "add", "-q", wt3.to_str().unwrap(), "three"]);
    let before = refs(&origin);
    assert_refused(apply(&project, &planned).await, &before, &origin);
}

/// A checkout dirtied after the preview stops the push itself, not just its own reset.
#[tokio::test]
async fn a_checkout_dirtied_after_the_preview_pushes_nothing() {
    let (_dir, project, origin, wt, planned) = planned_with_checkout().await;
    std::fs::write(wt.join("two.txt"), "edited\n").unwrap();
    let before = refs(&origin);
    assert_refused(apply(&project, &planned).await, &before, &origin);
    assert_eq!(std::fs::read_to_string(wt.join("two.txt")).unwrap(), "edited\n");
}

/// A status probe that fails is not taken for a clean checkout.
#[tokio::test]
async fn a_failing_status_probe_refuses() {
    let (_dir, project, origin, wt, planned) = planned_with_checkout().await;
    let gitdir = sh(&wt, &["rev-parse", "--absolute-git-dir"]);
    std::fs::write(Path::new(&gitdir).join("index"), "not an index").unwrap();
    let before = refs(&origin);
    assert_refused(apply(&project, &planned).await, &before, &origin);
}

/// A failure after the push is reported as one: the branches moved on the remote, and the local
/// change made meanwhile is kept rather than reset away.
#[cfg(unix)] // the pre-push hook is a shell script
#[tokio::test]
async fn a_failure_after_the_push_says_the_push_happened() {
    let (_dir, project, origin, wt, planned) = planned_with_checkout().await;
    // the push runs this, after every check before it passed; set on the repository, so a global
    // `core.hooksPath` cannot hide it
    let hooks = project.parent().unwrap().join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    sh(&project, &["config", "core.hooksPath", hooks.to_str().unwrap()]);
    let hook = hooks.join("pre-push");
    std::fs::write(&hook, format!("#!/bin/sh\necho edited > '{}'\n", wt.join("two.txt").display()))
        .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let outcome = apply(&project, &planned).await;
    assert!(matches!(outcome, Err(ApplyError::AfterPush(_))), "{outcome:?}");
    assert_eq!(sh(&origin, &["rev-parse", "two"]), planned.steps[0].new, "the push landed");
    assert_eq!(std::fs::read_to_string(wt.join("two.txt")).unwrap(), "edited\n");
    assert_eq!(sh(&wt, &["rev-parse", "HEAD"]), planned.steps[0].old, "not reset over the edit");
}
