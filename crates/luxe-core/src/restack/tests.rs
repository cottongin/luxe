use super::*;

mod safety;

fn sh(dir: &Path, args: &[&str]) -> String {
    // The same system config the code under test reads, or line endings disagree on Windows
    let output = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(output.status.success(), "{args:?}: {output:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str, contents: &str, subject: &str) -> String {
    std::fs::write(dir.join(file), contents).unwrap();
    sh(dir, &["add", file]);
    sh(dir, &["commit", "-qm", subject]);
    sh(dir, &["rev-parse", "HEAD"])
}

/// A clone of a bare origin carrying a three-layer stack: `one` on `main`, `two` on `one`,
/// `three` on `two`, each adding its own file, all pushed. The clone sits on `main`.
fn stacked() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    let project = dir.path().join("project");
    sh(dir.path(), &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
    sh(dir.path(), &["clone", "-q", origin.to_str().unwrap(), project.to_str().unwrap()]);
    sh(&project, &["config", "user.name", "t"]);
    sh(&project, &["config", "user.email", "t@t"]);
    sh(&project, &["switch", "-qc", "main"]);
    commit(&project, "base.txt", "base\n", "Start");
    sh(&project, &["push", "-q", "origin", "main"]);
    let mut below = "main".to_string();
    for layer in ["one", "two", "three"] {
        sh(&project, &["switch", "-qc", layer, &below]);
        commit(&project, &format!("{layer}.txt"), &format!("{layer}\n"), layer);
        if layer == "three" {
            commit(&project, "three-more.txt", "more\n", "three, more");
        }
        sh(&project, &["push", "-q", "origin", layer]);
        below = layer.to_string();
    }
    sh(&project, &["switch", "-q", "main"]);
    (dir, project, origin)
}

fn layers() -> Vec<Layer> {
    [(1, "one", "main"), (2, "two", "one"), (3, "three", "two")]
        .into_iter()
        .map(|(number, head, base)| Layer { number, head_ref: head.into(), base_ref: base.into() })
        .collect()
}

/// Adds a commit to `branch` on origin from elsewhere, as a review fix pushed from another
/// machine would, and answers its tip.
fn push_to(origin: &Path, branch: &str, file: &str, contents: &str) -> String {
    let elsewhere = origin.parent().unwrap().join(format!("elsewhere-{branch}"));
    sh(
        origin.parent().unwrap(),
        &["clone", "-q", "-b", branch, origin.to_str().unwrap(), elsewhere.to_str().unwrap()],
    );
    sh(&elsewhere, &["config", "user.name", "e"]);
    sh(&elsewhere, &["config", "user.email", "e@e"]);
    let tip = commit(&elsewhere, file, contents, "Review fix");
    sh(&elsewhere, &["push", "-q", "origin", branch]);
    tip
}

fn files_at(project: &Path, rev: &str) -> Vec<String> {
    sh(project, &["ls-tree", "--name-only", rev]).lines().map(str::to_string).collect()
}

#[tokio::test]
async fn a_stack_that_sits_right_has_nothing_to_do() {
    let (_dir, project, _origin) = stacked();
    let plan = plan(&project, "origin", &layers(), None).await.unwrap();
    assert!(plan.steps.is_empty(), "{plan:?}");
    assert!(!sh(&project, &["for-each-ref", SCRATCH_PREFIX]).contains("luxe-restack"));
}

/// A fix pushed to the bottom leaves the two above on its old tip. Both are replayed, each
/// carrying only its own commit; the push lands them on origin together; the local branches
/// follow, the checked-out one with its checkout.
#[tokio::test]
async fn a_fix_low_in_the_stack_moves_every_branch_above_it() {
    let (dir, project, origin) = stacked();
    let fixed = push_to(&origin, "one", "fix.txt", "fixed\n");
    // `two` is checked out, clean, in a worktree of its own
    let wt = dir.path().join("wt-two");
    sh(&project, &["worktree", "add", "-q", wt.to_str().unwrap(), "two"]);
    // `three` here is behind GitHub, which is no reason to refuse
    sh(&project, &["branch", "-qf", "three", "three~1"]);

    let plan = plan(&project, "origin", &layers(), None).await.unwrap();
    let moved: Vec<(u64, &str, usize)> =
        plan.steps.iter().map(|s| (s.number, s.onto.as_str(), s.commits)).collect();
    assert_eq!(moved, [(2, "one", 1), (3, "two", 2)]);
    assert_eq!(plan.steps[0].checked_out.as_deref().map(same), Some(same(&wt)));
    assert!(plan.steps[1].local && plan.steps[1].checked_out.is_none());
    assert_eq!(sh(&project, &["rev-parse", "two"]), plan.steps[0].old, "nothing moved yet");

    apply(&project, &plan).await.unwrap();
    let two = sh(&origin, &["rev-parse", "two"]);
    let three = sh(&origin, &["rev-parse", "three"]);
    assert_eq!(
        (two.as_str(), three.as_str()),
        (plan.steps[0].new.as_str(), plan.steps[1].new.as_str())
    );
    assert_eq!(sh(&origin, &["rev-parse", "two~1"]), fixed, "on the fix");
    assert_eq!(sh(&project, &["rev-parse", "three"]), three, "the local branch followed");
    assert_eq!(sh(&origin, &["rev-parse", "three~2"]), two, "both of its commits went");
    assert_eq!(sh(&wt, &["rev-parse", "HEAD"]), two, "and the checkout of `two`");
    assert!(wt.join("fix.txt").exists(), "the checkout has the fix's file");
    let status = sh(&wt, &["status", "--porcelain"]);
    assert!(status.is_empty(), "and nothing else changed: {status}");
}

/// The bottom squash-merged into `main`: the next one still carries its commit. Only its own
/// commit is replayed onto `main`, its pull request is retargeted there, and the top follows.
#[tokio::test]
async fn after_a_squash_merge_the_next_one_drops_what_merged_and_moves_to_trunk() {
    let (_dir, project, origin) = stacked();
    let merged_head = sh(&project, &["rev-parse", "one"]);
    // What GitHub's squash merge makes: one commit on main with `one`'s changes
    sh(&project, &["switch", "-q", "main"]);
    sh(&project, &["merge", "-q", "--squash", "one"]);
    sh(&project, &["commit", "-qm", "One (#1)"]);
    sh(&project, &["push", "-q", "origin", "main"]);
    let stack = &layers()[1..];
    let merged = MergedBelow { number: 1, head_oid: merged_head.clone(), base_ref: "main".into() };
    // Only on GitHub, not here: it moves there and stays away from here
    sh(&project, &["branch", "-qD", "three"]);

    let plan = plan(&project, "origin", stack, Some(&merged)).await.unwrap();
    assert_eq!(plan.steps.len(), 2);
    let bottom = &plan.steps[0];
    assert_eq!((bottom.number, bottom.onto.as_str(), bottom.commits), (2, "main", 1));
    assert_eq!(bottom.retarget.as_deref(), Some("main"));
    apply(&project, &plan).await.unwrap();
    let main = sh(&origin, &["rev-parse", "main"]);
    assert_eq!(sh(&origin, &["rev-parse", "two~1"]), main, "on main, not on `one`");
    let carried = std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", &merged_head, "two"])
        .current_dir(&origin)
        .status()
        .unwrap();
    assert!(!carried.success(), "the merged commit is gone from it");
    let two_log = sh(&origin, &["log", "--format=%s", "main..two"]);
    assert_eq!(two_log, "two", "only its own commit");
    assert_eq!(
        files_at(&origin, "three"),
        ["base.txt", "one.txt", "three-more.txt", "three.txt", "two.txt"]
    );
    assert!(!plan.steps[1].local);
    let branches = sh(&project, &["branch", "--list", "three"]);
    assert!(branches.is_empty(), "no local branch was made for it: {branches}");
}

/// Merged with a merge commit, trunk has the very commits: nothing is replayed, and only the
/// base moves.
#[tokio::test]
async fn after_a_merge_commit_only_the_base_moves() {
    let (_dir, project, origin) = stacked();
    let merged_head = sh(&project, &["rev-parse", "one"]);
    sh(&project, &["merge", "-q", "--no-ff", "-m", "Merge #1", "one"]);
    sh(&project, &["push", "-q", "origin", "main"]);
    let merged = MergedBelow { number: 1, head_oid: merged_head, base_ref: "main".into() };
    let plan = plan(&project, "origin", &layers()[1..], Some(&merged)).await.unwrap();
    assert_eq!(plan.steps.len(), 1, "{plan:?}");
    assert_eq!(plan.steps[0].new, plan.steps[0].old);
    assert_eq!(plan.steps[0].retarget.as_deref(), Some("main"));
    let before = sh(&origin, &["rev-parse", "two"]);
    apply(&project, &plan).await.unwrap();
    assert_eq!(sh(&origin, &["rev-parse", "two"]), before, "no push for a base alone");
}

/// A branch whose commits no longer apply to the one below is named, and nothing anywhere
/// changes; neither does anything when a checkout of a moving branch has changes, or a local
/// branch has commits GitHub has not.
#[tokio::test]
async fn a_conflict_or_work_at_risk_refuses_and_changes_nothing() {
    let (dir, project, origin) = stacked();
    push_to(&origin, "one", "two.txt", "clashes with two\n");
    let before = sh(&origin, &["for-each-ref", "--format=%(objectname)"]);
    let refused = plan(&project, "origin", &layers(), None).await;
    assert!(
        matches!(&refused, Err(RestackError::Conflict { number: 2, onto, .. }) if onto == "one"),
        "{refused:?}"
    );
    let Err(RestackError::Conflict { detail, .. }) = &refused else { unreachable!() };
    assert_eq!(detail, "\"two\" conflicts with it in `two.txt`", "which commit, which file");
    assert_eq!(sh(&origin, &["for-each-ref", "--format=%(objectname)"]), before);
    assert!(!sh(&project, &["for-each-ref"]).contains("luxe-restack"), "no scratch left");

    let (dir2, project2, origin2) = stacked();
    push_to(&origin2, "one", "fix.txt", "fixed\n");
    let wt = dir2.path().join("wt-three");
    sh(&project2, &["worktree", "add", "-q", wt.to_str().unwrap(), "three"]);
    std::fs::write(wt.join("three.txt"), "edited\n").unwrap();
    let dirty = plan(&project2, "origin", &layers(), None).await;
    assert!(
        matches!(&dirty, Err(RestackError::Dirty { head_ref, .. }) if head_ref == "three"),
        "{dirty:?}"
    );

    std::fs::write(wt.join("three.txt"), "three\n").unwrap();
    commit(&wt, "local.txt", "not pushed\n", "Local");
    let unpushed = plan(&project2, "origin", &layers(), None).await;
    assert!(
        matches!(&unpushed, Err(RestackError::Unpushed(branch)) if branch == "three"),
        "{unpushed:?}"
    );
    drop((dir, dir2));
}

/// A replay git refuses outright says why, in its words, and is not taken for a conflict; and
/// what git said is read from its end, where the verdict is.
#[tokio::test]
async fn a_failed_replay_is_told_from_a_conflict() {
    let (_dir, project, _origin) = stacked();
    let failed = replay_run(&project, &["replay", "--onto=nonexistent", "main..one"]).await;
    assert!(matches!(&failed, Replayed::Failed(said) if !said.is_empty()), "{failed:?}");
    assert_eq!(
        last_lines("runtime noise\n\nfatal: one\nhint: two\nhint: three\n"),
        "fatal: one hint: two hint: three"
    );
    assert_eq!(last_lines("warning\nfatal: it"), "warning fatal: it");
    assert_eq!(last_lines("  \n"), "no reason given");
    // A layer GitHub does not have: git's own words, not an empty sentence
    let missing = [Layer { number: 9, head_ref: "gone".into(), base_ref: "main".into() }];
    let fetched = plan(&project, "origin", &missing, None).await;
    assert!(
        matches!(&fetched, Err(RestackError::Git { operation: "fetch the stack", detail }) if detail.contains("gone")),
        "{fetched:?}"
    );
}

fn same(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
