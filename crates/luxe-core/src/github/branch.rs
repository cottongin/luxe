//! Branch heads — where the checked-out branch is published, which decides its pull request.
//!
//! A pull request is found by its head branch's name, and on a popular repository many forks push
//! a branch called `fix-typo`. What makes one of them yours is the repository you push to, so the
//! head carries that owner, read from git's own idea of where `git push` goes (`@{push}`).

use super::gh::local_remotes;
use super::remote::parse_remote_url;
use super::BranchHead;
use crate::program::{captured_command, run_bounded};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a local git read may take. It reads config and refs; anything slower is stuck.
const GIT_DEADLINE: Duration = Duration::from_secs(10);

/// The branch checked out in `cwd` and where it is pushed, or `None` on a detached `HEAD` or
/// outside a repository.
pub async fn branch_head(cwd: &Path) -> Option<BranchHead> {
    let local = git(cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"]).await?;
    let push = git(cwd, &["rev-parse", "--symbolic-full-name", "@{push}"]).await;
    Some(head_from(&local, push.as_deref(), &local_remotes(cwd).await))
}

/// Where `local` is pushed, given git's `@{push}` ref and the checkout's remotes.
///
/// A branch with nowhere to push keeps its own name and no owner. So does one pushed to a remote
/// that is not a hosted repository, since no pull request can have it as its head.
fn head_from(local: &str, push_ref: Option<&str>, remotes: &[(String, String)]) -> BranchHead {
    let unpublished = || BranchHead { name: local.to_string(), owner: None };
    let Some(tracking) = push_ref.and_then(|push| push.strip_prefix("refs/remotes/")) else {
        return unpublished();
    };
    // The longest remote name that prefixes the ref, since a remote may be named `a` and `a/b`
    let found = remotes
        .iter()
        .filter_map(|(remote, url)| {
            let name = tracking.strip_prefix(remote.as_str())?.strip_prefix('/')?;
            (!name.is_empty()).then_some((remote.len(), name, url))
        })
        .max_by_key(|(length, ..)| *length);
    match found {
        Some((_, name, url)) => BranchHead {
            name: name.to_string(),
            owner: parse_remote_url(url).map(|parsed| parsed.owner),
        },
        None => unpublished(),
    }
}

/// One line of git's output in `cwd`, or `None` when it fails or prints nothing.
async fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let program = crate::program::resolve("git").unwrap_or_else(|| PathBuf::from("git"));
    let output = run_bounded(captured_command(program, args, cwd), GIT_DEADLINE).await.ok()?;
    let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !line.is_empty()).then_some(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remotes(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(name, url)| ((*name).to_string(), (*url).to_string())).collect()
    }

    #[test]
    fn the_push_target_names_the_owner_and_the_remote_branch() {
        let remotes = remotes(&[
            ("origin", "git@github.com:me/luxe.git"),
            ("upstream", "https://github.com/acme/luxe"),
        ]);
        let head = head_from("topic", Some("refs/remotes/origin/feat/topic"), &remotes);
        assert_eq!(head, BranchHead { name: "feat/topic".into(), owner: Some("me".into()) });
    }

    #[test]
    fn the_longest_remote_name_wins() {
        let remotes = remotes(&[
            ("fork", "https://github.com/short/luxe"),
            ("fork/team", "https://github.com/team/luxe"),
        ]);
        let head = head_from("x", Some("refs/remotes/fork/team/x"), &remotes);
        assert_eq!(head, BranchHead { name: "x".into(), owner: Some("team".into()) });
    }

    #[test]
    fn nowhere_to_push_keeps_the_local_name_and_no_owner() {
        let remotes = remotes(&[("origin", "git@github.com:me/luxe.git")]);
        let unpublished = BranchHead { name: "topic".into(), owner: None };
        assert_eq!(head_from("topic", None, &remotes), unpublished);
        assert_eq!(head_from("topic", Some("refs/heads/topic"), &remotes), unpublished);
        assert_eq!(head_from("topic", Some("refs/remotes/gone/topic"), &remotes), unpublished);
        assert_eq!(head_from("topic", Some("refs/remotes/origin/"), &remotes), unpublished);
    }

    #[test]
    fn a_remote_that_is_not_hosted_has_no_owner() {
        let remotes = remotes(&[("local", "/srv/git/luxe.git")]);
        let head = head_from("topic", Some("refs/remotes/local/topic"), &remotes);
        assert_eq!(head, BranchHead { name: "topic".into(), owner: None });
    }

    /// Against a real repository: a branch pushed to a remote reports that remote's owner and the
    /// branch's name there, and a detached `HEAD` has no head at all.
    #[tokio::test]
    async fn a_real_checkout_reports_where_it_pushes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let run = |args: &[&str]| {
            let output =
                std::process::Command::new("git").args(args).current_dir(root).output().unwrap();
            assert!(output.status.success(), "{args:?}: {output:?}");
        };
        run(&["init", "-q", "-b", "topic"]);
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "a",
        ]);
        run(&["remote", "add", "origin", "git@github.com:me/luxe.git"]);
        run(&["update-ref", "refs/remotes/origin/topic", "HEAD"]);
        run(&["branch", "-q", "--set-upstream-to=origin/topic"]);
        // Whatever the machine's own config says about where pushes go
        run(&["config", "push.default", "simple"]);
        run(&["config", "remote.pushDefault", "origin"]);

        let head = branch_head(root).await.expect("a branch is checked out");
        assert_eq!(head, BranchHead { name: "topic".into(), owner: Some("me".into()) });

        run(&["checkout", "-q", "--detach"]);
        assert_eq!(branch_head(root).await, None);

        // A read that succeeds with nothing to say, and one that fails after printing
        assert_eq!(git(root, &["rev-parse", "--show-prefix"]).await, None);
        assert_eq!(git(root, &["rev-parse", "no-such-ref"]).await, None);
    }
}
