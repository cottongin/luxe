use super::*;

/// A worktree names its project, from its root or from inside it; a project's own
/// `.luxe/worktrees`, and any other path, name none.
#[test]
fn a_worktree_names_its_project() {
    let project = Path::new("/home/me/app");
    let worktree = project.join(".luxe/worktrees/topic");
    assert_eq!(project_of(&worktree).as_deref(), Some(project));
    assert_eq!(project_of(&worktree.join("src/lib.rs")).as_deref(), Some(project));
    // A `.luxe` of the worktree's own, below it, is not the one that names the project
    assert_eq!(project_of(&worktree.join(".luxe/cache")).as_deref(), Some(project));
    assert_eq!(project_of(&project.join(".luxe/worktrees")), None, "no worktree named");
    assert_eq!(project_of(project), None);
    assert_eq!(project_of(Path::new(".luxe/worktrees/topic")), None, "no project before it");
}

fn sh(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(output.status.success(), "{args:?}: {output:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A project with one commit, a remote whose default branch is `main`, and a local edit of
/// its own that must survive everything.
fn project() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    let project = dir.path().join("project");
    sh(dir.path(), &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
    sh(dir.path(), &["init", "-q", "-b", "main", project.to_str().unwrap()]);
    sh(&project, &["config", "user.name", "t"]);
    sh(&project, &["config", "user.email", "t@t"]);
    std::fs::write(project.join("lib.rs"), "fn lib() {}\n").unwrap();
    sh(&project, &["add", "lib.rs"]);
    sh(&project, &["commit", "-qm", "start"]);
    sh(&project, &["remote", "add", "origin", origin.to_str().unwrap()]);
    sh(&project, &["push", "-q", "-u", "origin", "main"]);
    sh(&project, &["remote", "set-head", "origin", "main"]);
    std::fs::create_dir_all(project.join(".luxe")).unwrap();
    std::fs::write(project.join(".luxe/.gitignore"), "*\n").unwrap();
    std::fs::write(project.join("lib.rs"), "fn lib() { mine(); }\n").unwrap();
    (dir, project)
}

#[tokio::test]
async fn a_new_branch_starts_from_the_remotes_default_and_leaves_the_checkout_alone() {
    let (_dir, project) = project();
    let made = prepare(&project, "luxe/fix-the-loop", None).await.unwrap();

    assert_eq!(made.path, project.join(".luxe/worktrees/luxe-fix-the-loop"));
    assert_eq!(made.branch, "luxe/fix-the-loop");
    assert_eq!(made.base.as_deref(), Some("origin/main"));
    assert!(made.created && !made.fetch_failed);
    assert_eq!(sh(&made.path, &["branch", "--show-current"]), "luxe/fix-the-loop");
    // Line endings as git on this platform checks them out, which on Windows is CRLF
    assert_eq!(
        std::fs::read_to_string(made.path.join("lib.rs")).unwrap().replace("\r\n", "\n"),
        "fn lib() {}\n",
        "the worktree has the committed file, not the checkout's edit"
    );
    assert_eq!(
        std::fs::read_to_string(project.join("lib.rs")).unwrap(),
        "fn lib() { mine(); }\n",
        "and the checkout keeps its own"
    );
    assert_eq!(sh(&project, &["status", "--porcelain"]), "M lib.rs", "nothing else appears");
    // A plain push publishes it under its own name, and then git knows where it went
    // with no global config of its own, which could set up the upstream itself
    let empty = project.join(".luxe/empty-gitconfig");
    std::fs::write(&empty, "").unwrap();
    let pushed = std::process::Command::new("git")
        .args(["push", "-q"])
        .current_dir(&made.path)
        .env("GIT_CONFIG_GLOBAL", &empty)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(pushed.status.success(), "{pushed:?}");
    assert_eq!(
        sh(&made.path, &["rev-parse", "--abbrev-ref", "@{push}"]),
        "origin/luxe/fix-the-loop"
    );
}

/// Asking again finds the same worktree, and a directory deleted by hand is made again from
/// the branch it was on, with the work committed there intact.
#[tokio::test]
async fn the_same_branch_is_found_again_and_rebuilt_after_a_manual_delete() {
    let (_dir, project) = project();
    let first = prepare(&project, "topic", None).await.unwrap();
    std::fs::write(first.path.join("new.rs"), "fn new() {}\n").unwrap();
    sh(&first.path, &["add", "new.rs"]);
    sh(&first.path, &["commit", "-qm", "work"]);

    let again = prepare(&project, "topic", None).await.unwrap();
    assert_eq!((again.path.clone(), again.created), (first.path.clone(), false));

    std::fs::remove_dir_all(&first.path).unwrap();
    let rebuilt = prepare(&project, "topic", None).await.unwrap();
    assert!(rebuilt.created && rebuilt.base.is_none(), "from the branch, not a base");
    assert!(rebuilt.path.join("new.rs").exists(), "the committed work is still there");
}

#[tokio::test]
async fn a_given_base_is_used_and_an_unreachable_remote_falls_back_to_what_is_here() {
    let (_dir, project) = project();
    let made = prepare(&project, "from-head", Some("HEAD")).await.unwrap();
    assert_eq!(made.base.as_deref(), Some("HEAD"));

    sh(&project, &["remote", "set-url", "origin", "/nonexistent/origin.git"]);
    let offline = prepare(&project, "offline", None).await.unwrap();
    assert!(offline.created && offline.fetch_failed);
    assert_eq!(offline.base.as_deref(), Some("origin/main"), "the last known default");
}

#[tokio::test]
async fn refusals_say_why() {
    let (_dir, project) = project();
    assert!(matches!(
        prepare(&project, "bad..name", None).await,
        Err(WorktreeError::InvalidBranch(name)) if name == "bad..name"
    ));
    assert!(matches!(
        prepare(&project, "main", None).await,
        Err(WorktreeError::CheckedOutElsewhere { branch, .. }) if branch == "main"
    ));
    let plain = tempfile::tempdir().unwrap();
    assert!(matches!(prepare(plain.path(), "x", None).await, Err(WorktreeError::NotARepository)));
}

#[tokio::test]
async fn without_a_remote_a_new_branch_starts_at_head() {
    let dir = tempfile::tempdir().unwrap();
    sh(dir.path(), &["init", "-q", "-b", "main"]);
    sh(
        dir.path(),
        &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "a"],
    );
    let made = prepare(dir.path(), "solo", None).await.unwrap();
    assert_eq!(made.base.as_deref(), Some("HEAD"));
    assert!(!made.fetch_failed);
    let upstream = std::process::Command::new("git")
        .args(["config", "branch.solo.remote"])
        .current_dir(&made.path)
        .output()
        .unwrap();
    assert!(!upstream.status.success(), "no remote, so no upstream is invented");
}

/// Removing refuses while anything is uncommitted, and once it goes the branch and its
/// commits stay.
#[tokio::test]
async fn removing_refuses_uncommitted_work_and_keeps_the_branch() {
    let (_dir, project) = project();
    let made = prepare(&project, "topic", None).await.unwrap();
    std::fs::write(made.path.join("wip.rs"), "fn wip() {}\n").unwrap();
    let Err(WorktreeError::Git { detail, .. }) = remove(&project, &made.path).await else {
        panic!("refused");
    };
    assert!(detail.contains("untracked") || detail.contains("modified"), "{detail}");
    assert!(made.path.exists());

    sh(&made.path, &["add", "wip.rs"]);
    sh(&made.path, &["commit", "-qm", "wip"]);
    remove(&project, &made.path).await.unwrap();
    assert!(!made.path.exists());
    assert_eq!(sh(&project, &["log", "-1", "--format=%s", "topic"]), "wip", "the branch stays");
}

#[test]
fn the_porcelain_listing_says_where_a_branch_is() {
    let listing = "worktree /work/app\nHEAD abc\nbranch refs/heads/main\n\n\
                       worktree /work/app/.luxe/worktrees/topic\nHEAD def\nbranch refs/heads/topic\n";
    assert_eq!(
        checked_out_at(listing, "topic"),
        Some(PathBuf::from("/work/app/.luxe/worktrees/topic"))
    );
    assert_eq!(checked_out_at(listing, "main"), Some(PathBuf::from("/work/app")));
    assert_eq!(checked_out_at(listing, "other"), None);
    assert_eq!(checked_out_at(listing, "mai"), None);
}

/// Where the directory is taken already, git's own reason comes back.
#[tokio::test]
async fn a_taken_directory_says_what_git_said() {
    let (_dir, project) = project();
    let taken = worktree_path(&project, "clash");
    std::fs::create_dir_all(&taken).unwrap();
    std::fs::write(taken.join("stray.txt"), "left behind\n").unwrap();
    let Err(WorktreeError::Git { detail, .. }) = prepare(&project, "clash", None).await else {
        panic!("refused");
    };
    assert!(detail.contains("already exists"), "{detail}");
}

#[test]
fn two_spellings_of_one_directory_are_the_same_path() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("b")).unwrap();
    std::fs::create_dir(dir.path().join("a")).unwrap();
    assert!(same_path(&dir.path().join("a/../b"), &dir.path().join("b")));
    assert!(!same_path(&dir.path().join("a"), &dir.path().join("b")));
    assert!(
        same_path(Path::new("/no/such/x"), Path::new("/no/such/x")),
        "unresolvable: as written"
    );
}

// === Checking out a pull request ===

/// A hub of bare repositories reached as `https://github.com/<owner>/<name>.git` through
/// `insteadOf`, so the remotes look hosted: `acme/app` is the project's own, `mara/app-fork`
/// a fork of it. The fork's `main` has a commit the project has not seen, and the project's
/// `origin` has a pull request branch, `fix-parser`, of its own.
fn hub() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let hub = dir.path().join("hub");
    let own = hub.join("acme/app.git");
    let fork = hub.join("mara/app-fork.git");
    let project = dir.path().join("project");
    std::fs::create_dir_all(&hub).unwrap();
    sh(dir.path(), &["init", "-q", "--bare", "-b", "main", own.to_str().unwrap()]);
    sh(dir.path(), &["init", "-q", "-b", "main", project.to_str().unwrap()]);
    let rewrite = format!("url.{}/.insteadOf", hub.display());
    sh(&project, &["config", &rewrite, "https://github.com/"]);
    sh(&project, &["config", "user.name", "t"]);
    sh(&project, &["config", "user.email", "t@t"]);
    std::fs::write(project.join("lib.rs"), "fn lib() {}\n").unwrap();
    sh(&project, &["add", "lib.rs"]);
    sh(&project, &["commit", "-qm", "start"]);
    sh(&project, &["remote", "add", "origin", "https://github.com/acme/app.git"]);
    sh(&project, &["push", "-q", "origin", "main"]);
    // The same-repository pull request's branch, pushed from elsewhere
    sh(&project, &["switch", "-qc", "fix-parser"]);
    std::fs::write(project.join("lib.rs"), "fn lib() { parse(); }\n").unwrap();
    sh(&project, &["commit", "-qam", "Parse"]);
    sh(&project, &["push", "-q", "origin", "fix-parser"]);
    sh(&project, &["switch", "-q", "main"]);
    sh(&project, &["branch", "-qD", "fix-parser"]);
    // The fork, with a commit on its own main
    sh(dir.path(), &["clone", "-q", "--bare", own.to_str().unwrap(), fork.to_str().unwrap()]);
    let contributor = dir.path().join("contributor");
    sh(dir.path(), &["clone", "-q", fork.to_str().unwrap(), contributor.to_str().unwrap()]);
    sh(&contributor, &["config", "user.name", "m"]);
    sh(&contributor, &["config", "user.email", "m@m"]);
    std::fs::write(contributor.join("lib.rs"), "fn lib() { forked(); }\n").unwrap();
    sh(&contributor, &["commit", "-qam", "Fork"]);
    sh(&contributor, &["push", "-q", "origin", "main"]);
    sh(&contributor, &["push", "-q", "origin", "main:patch-1"]);
    std::fs::create_dir_all(project.join(".luxe")).unwrap();
    std::fs::write(project.join(".luxe/.gitignore"), "*\n").unwrap();
    (dir, project, hub)
}

fn head(head_ref: &str, owner: &str, name: &str, local: &str) -> crate::github::PullRequestHead {
    crate::github::PullRequestHead {
        head_ref: head_ref.into(),
        owner: owner.into(),
        name: name.into(),
        cross_repository: owner != "acme",
        local_branch: local.into(),
        push: crate::github::PushAccess::Allowed,
    }
}

async fn check_out(
    project: &Path,
    head: &crate::github::PullRequestHead,
) -> Result<PullRequestWorktree, WorktreeError> {
    let checkout = PullRequestCheckout { head, host: "github.com", base_remote: "origin" };
    check_out_pull_request(project, &checkout).await
}

/// Pushes from `dir` with no global config, which could set up where a push goes by itself.
fn push(dir: &Path) -> std::process::Output {
    let empty = dir.join("../empty-gitconfig");
    std::fs::write(&empty, "").unwrap();
    std::process::Command::new("git")
        .args(["push", "-q"])
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", &empty)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
}

fn commit_in(dir: &Path, contents: &str) -> String {
    std::fs::write(dir.join("lib.rs"), contents).unwrap();
    sh(dir, &["commit", "-qam", "Fix it"]);
    sh(dir, &["rev-parse", "HEAD"])
}

/// A branch in this repository is checked out on its own name, tracking it, and a plain push
/// from the worktree lands on it. The checkout stays on `main`, untouched.
#[tokio::test]
async fn a_same_repository_pull_request_is_pushed_to_where_it_came_from() {
    let (_dir, project, hub) = hub();
    let made = check_out(&project, &head("fix-parser", "acme", "app", "fix-parser")).await.unwrap();
    assert_eq!((made.remote.as_str(), made.remote_added), ("origin", false));
    assert_eq!(made.worktree.path, project.join(".luxe/worktrees/fix-parser"));
    assert!(made.worktree.created);
    assert_eq!(sh(&made.worktree.path, &["branch", "--show-current"]), "fix-parser");
    assert_eq!(sh(&project, &["branch", "--show-current"]), "main", "your checkout stays put");

    let fixed = commit_in(&made.worktree.path, "fn lib() { parse(); fixed(); }\n");
    let pushed = push(&made.worktree.path);
    assert!(pushed.status.success(), "{pushed:?}");
    let remote = hub.join("acme/app.git");
    assert_eq!(sh(&remote, &["rev-parse", "fix-parser"]), fixed, "the pull request has it");
    assert_eq!(
        sh(&made.worktree.path, &["rev-parse", "--symbolic-full-name", "@{push}"]),
        "refs/remotes/origin/fix-parser"
    );

    // Asked again, as a resumed session would, it is found rather than made
    let again =
        check_out(&project, &head("fix-parser", "acme", "app", "fix-parser")).await.unwrap();
    assert!(!again.worktree.created);
}

/// A fork's `main` cannot be checked out as `main`, which is yours. It gets `mara/main`, a
/// remote added for the fork in the style `origin` is written in, and a push refspec, so a
/// plain push still lands on the fork's `main` and `@{push}` names it.
#[tokio::test]
async fn a_forks_default_branch_is_checked_out_beside_yours_and_pushed_back_to_it() {
    let (_dir, project, hub) = hub();
    let made = check_out(&project, &head("main", "mara", "app-fork", "mara/main")).await.unwrap();
    assert_eq!((made.remote.as_str(), made.remote_added), ("mara", true));
    assert_eq!(
        sh(&project, &["config", "--get", "remote.mara.url"]),
        "https://github.com/mara/app-fork.git"
    );
    assert_eq!(made.worktree.path, project.join(".luxe/worktrees/mara-main"));
    assert_eq!(
        std::fs::read_to_string(made.worktree.path.join("lib.rs")).unwrap().replace("\r\n", "\n"),
        "fn lib() { forked(); }\n",
        "the fork's commit, not this repository's main"
    );

    let fixed = commit_in(&made.worktree.path, "fn lib() { forked(); fixed(); }\n");
    let pushed = push(&made.worktree.path);
    assert!(pushed.status.success(), "{pushed:?}");
    assert_eq!(sh(&hub.join("mara/app-fork.git"), &["rev-parse", "main"]), fixed);
    assert_ne!(sh(&hub.join("acme/app.git"), &["rev-parse", "main"]), fixed, "not yours");

    // A fork's branch that keeps its own name needs no mapping: a plain push already lands
    let same = check_out(&project, &head("patch-1", "mara", "app-fork", "patch-1")).await.unwrap();
    assert_eq!(same.remote, "mara", "the remote added the first time, found");
    assert!(remote_push_rules(&project, "mara").is_empty(), "the shared remote maps nothing");
    // and checking the renamed one out again, as a resumed session does, reuses its push route
    let routes = sh(&project, &["remote"]);
    check_out(&project, &head("main", "mara", "app-fork", "mara/main")).await.unwrap();
    assert_eq!(sh(&project, &["remote"]), routes, "no second route");
    assert_eq!(
        sh(&made.worktree.path, &["rev-parse", "--symbolic-full-name", "@{push}"]),
        "refs/remotes/mara/main"
    );
}

/// `remote.<name>.push`, every value, empty when there is none.
fn remote_push_rules(project: &Path, remote: &str) -> String {
    let output = std::process::Command::new("git")
        .args(["config", "--get-all", &format!("remote.{remote}.push")])
        .current_dir(project)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A plain push from a fork's same-name branch goes to that branch, even after a renamed one from
/// the same fork was opened: the rename's mapping belongs to the renamed branch alone.
#[tokio::test]
async fn a_push_from_patch_1_after_opening_mara_main_moves_only_patch_1() {
    let (_dir, project, hub) = hub();
    check_out(&project, &head("main", "mara", "app-fork", "mara/main")).await.unwrap();
    let patch = check_out(&project, &head("patch-1", "mara", "app-fork", "patch-1")).await.unwrap();
    let fork = hub.join("mara/app-fork.git");
    let fork_main = sh(&fork, &["rev-parse", "main"]);
    let fixed = commit_in(&patch.worktree.path, "fn lib() { forked(); patched(); }\n");
    let pushed = push(&patch.worktree.path);
    assert!(pushed.status.success(), "{pushed:?}");
    assert_eq!(sh(&fork, &["rev-parse", "patch-1"]), fixed, "patch-1 moved");
    assert_eq!(sh(&fork, &["rev-parse", "main"]), fork_main, "and the fork's main did not");
}

/// A fork remote that was already here, which Luxe did not add, still gets the renamed branch's
/// route, so a plain push from it lands on the fork's `main`.
#[tokio::test]
async fn a_pre_existing_fork_remote_still_routes_a_renamed_branch() {
    let (_dir, project, hub) = hub();
    sh(&project, &["remote", "add", "mara", "https://github.com/mara/app-fork.git"]);
    let made = check_out(&project, &head("main", "mara", "app-fork", "mara/main")).await.unwrap();
    assert_eq!((made.remote.as_str(), made.remote_added), ("mara", false));
    let fixed = commit_in(&made.worktree.path, "fn lib() { forked(); fixed(); }\n");
    let pushed = push(&made.worktree.path);
    assert!(pushed.status.success(), "{pushed:?}");
    assert_eq!(sh(&hub.join("mara/app-fork.git"), &["rev-parse", "main"]), fixed);
}

/// The remote-wide mapping an earlier Luxe wrote is moved off the shared remote; a rule of the
/// reader's own on it is left as it is.
#[tokio::test]
async fn an_earlier_mapping_is_migrated_and_a_readers_rule_kept() {
    let (_dir, project, _hub) = hub();
    sh(&project, &["remote", "add", "mara", "https://github.com/mara/app-fork.git"]);
    sh(&project, &["config", "--add", "remote.mara.push", "refs/heads/mara/main:refs/heads/main"]);
    sh(&project, &["config", "--add", "remote.mara.push", "refs/heads/mine:refs/heads/theirs"]);
    check_out(&project, &head("main", "mara", "app-fork", "mara/main")).await.unwrap();
    assert_eq!(remote_push_rules(&project, "mara"), "refs/heads/mine:refs/heads/theirs");
}

/// A branch by the pull request's name that holds other work is refused, not moved; one that
/// is behind the head catches up; and a fork that cannot be fetched leaves no remote behind.
#[tokio::test]
async fn a_branch_in_the_way_is_refused_and_a_failed_fetch_leaves_nothing() {
    let (_dir, project, _hub) = hub();
    sh(&project, &["switch", "-qc", "fix-parser"]);
    commit_in(&project, "fn lib() { mine(); }\n");
    sh(&project, &["switch", "-q", "main"]);
    let taken = check_out(&project, &head("fix-parser", "acme", "app", "fix-parser")).await;
    assert!(
        matches!(taken, Err(WorktreeError::BranchTaken(ref branch)) if branch == "fix-parser"),
        "{taken:?}"
    );
    assert_eq!(sh(&project, &["log", "-1", "--format=%s", "fix-parser"]), "Fix it", "untouched");

    // Behind the head: it catches up
    sh(&project, &["branch", "-qf", "fix-parser", "main"]);
    let made = check_out(&project, &head("fix-parser", "acme", "app", "fix-parser")).await.unwrap();
    assert_eq!(sh(&made.worktree.path, &["log", "-1", "--format=%s"]), "Parse");

    let gone = check_out(&project, &head("patch", "nobody", "missing", "patch")).await;
    assert!(
        matches!(gone, Err(WorktreeError::Git { operation: "fetch the pull request", .. })),
        "{gone:?}"
    );
    assert!(!sh(&project, &["remote"]).contains("nobody"), "the remote added for it is gone");
    assert!(!project.join(".luxe/worktrees/patch").exists());
}

#[test]
fn a_branch_with_slashes_is_one_directory() {
    let path = worktree_path(Path::new("/work/app"), "luxe/2026/fix");
    assert_eq!(path, Path::new("/work/app/.luxe/worktrees/luxe-2026-fix"));
}

/// Someone upgrading has the old remote-wide mapping already; opening only the same-name branch
/// still moves it off the shared remote, so a push from `patch-1` lands on `patch-1`.
#[tokio::test]
async fn an_earlier_mapping_is_migrated_even_when_only_patch_1_is_opened() {
    let (_dir, project, hub) = hub();
    sh(&project, &["remote", "add", "mara", "https://github.com/mara/app-fork.git"]);
    sh(&project, &["fetch", "-q", "mara", "main"]);
    sh(&project, &["branch", "-q", "--no-track", "mara/main", "FETCH_HEAD"]);
    sh(&project, &["config", "branch.mara/main.remote", "mara"]);
    sh(&project, &["config", "branch.mara/main.merge", "refs/heads/main"]);
    sh(&project, &["config", "--add", "remote.mara.push", "refs/heads/mara/main:refs/heads/main"]);
    let patch = check_out(&project, &head("patch-1", "mara", "app-fork", "patch-1")).await.unwrap();
    assert!(remote_push_rules(&project, "mara").is_empty(), "moved off the shared remote");
    let fork = hub.join("mara/app-fork.git");
    let fork_main = sh(&fork, &["rev-parse", "main"]);
    let fixed = commit_in(&patch.worktree.path, "fn lib() { forked(); patched(); }\n");
    assert!(push(&patch.worktree.path).status.success());
    assert_eq!(sh(&fork, &["rev-parse", "patch-1"]), fixed);
    assert_eq!(sh(&fork, &["rev-parse", "main"]), fork_main);
    let route = sh(&project, &["config", "--get", "branch.mara/main.pushRemote"]);
    assert_eq!(remote_push_rules(&project, &route), "refs/heads/mara/main:refs/heads/main");
}

/// A remote of the reader's own that happens to have the route's name is left alone; the route
/// takes the next free name.
#[tokio::test]
async fn a_routes_name_taken_by_another_remote_is_skipped() {
    let (_dir, project, _hub) = hub();
    sh(&project, &["remote", "add", "mara-push-main", "https://example.com/elsewhere.git"]);
    check_out(&project, &head("main", "mara", "app-fork", "mara/main")).await.unwrap();
    assert_eq!(
        sh(&project, &["config", "--get", "remote.mara-push-main.url"]),
        "https://example.com/elsewhere.git"
    );
    assert_eq!(
        sh(&project, &["config", "--get", "branch.mara/main.pushRemote"]),
        "mara-push-main-2"
    );
}
