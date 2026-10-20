//! Push routes — where a plain `git push` from a pull request's worktree goes.

use std::path::Path;

use super::{git, WorktreeError};

/// Where a plain `git push` from `branch` goes: to `head_ref` on `remote`.
///
/// A branch on the head's own name needs nothing more: it tracks the head, and git pushes it
/// there. A renamed one (a fork's `main` checked out as `mara/main`, since `main` is yours) gets a
/// remote of its own with exactly one mapping, named by its `pushRemote`. A mapping on the shared
/// remote would reroute every other branch pushed to it, so one an earlier luxe wrote there is
/// moved onto its branch's own route whichever branch is being opened; a rule of the reader's own
/// is left alone.
pub(super) async fn route_push(
    project: &Path,
    remote: &str,
    branch: &str,
    head_ref: &str,
) -> Result<(), WorktreeError> {
    set_config(project, &format!("branch.{branch}.remote"), remote).await?;
    set_config(project, &format!("branch.{branch}.merge"), &format!("refs/heads/{head_ref}"))
        .await?;
    migrate_shared_rules(project, remote).await?;
    if branch != head_ref {
        dedicated_route(project, remote, branch, head_ref).await?;
    }
    Ok(())
}

/// Moves each `refs/heads/B:refs/heads/H` rule on `remote` that an earlier luxe wrote (B tracks H
/// on that remote) onto B's own route.
async fn migrate_shared_rules(project: &Path, remote: &str) -> Result<(), WorktreeError> {
    let shared = format!("remote.{remote}.push");
    let rules = git(project, &["config", "--get-all", &shared]).await.unwrap_or_default();
    for rule in rules.lines() {
        let Some((branch, head_ref)) = rule.split_once(':').and_then(|(from, to)| {
            Some((from.strip_prefix("refs/heads/")?, to.strip_prefix("refs/heads/")?))
        }) else {
            continue;
        };
        let get = |key: String| async move { git(project, &["config", "--get", &key]).await.ok() };
        let tracks = get(format!("branch.{branch}.remote")).await.as_deref() == Some(remote)
            && get(format!("branch.{branch}.merge")).await
                == Some(format!("refs/heads/{head_ref}"));
        if branch == head_ref || !tracks {
            continue;
        }
        let exactly = format!("^{}$", regex_escape(rule));
        git(project, &["config", "--unset", &shared, &exactly]).await.map_err(route_failed)?;
        dedicated_route(project, remote, branch, head_ref).await?;
    }
    Ok(())
}

/// `branch`'s own push remote: the shared remote's URL, one mapping, and the shared remote's
/// tracking ref, so `@{push}` names the head it pushes to. Marked, so it is never taken for the
/// fork's remote itself.
async fn dedicated_route(
    project: &Path,
    remote: &str,
    branch: &str,
    head_ref: &str,
) -> Result<(), WorktreeError> {
    let mapping = format!("refs/heads/{branch}:refs/heads/{head_ref}");
    let url = git(project, &["config", "--get", &format!("remote.{remote}.url")])
        .await
        .map_err(route_failed)?;
    let route = push_route_name(project, remote, head_ref, &url, &mapping).await;
    let tracking = format!("+refs/heads/{head_ref}:refs/remotes/{remote}/{head_ref}");
    for (key, value) in
        [("url", url.as_str()), ("push", &mapping), ("fetch", &tracking), (PUSH_ROUTE_MARK, "true")]
    {
        set_config(project, &format!("remote.{route}.{key}"), value).await?;
    }
    set_config(project, &format!("branch.{branch}.pushRemote"), &route).await
}

/// The config key, under `remote.<name>`, that marks a remote as a renamed branch's push route.
pub(crate) const PUSH_ROUTE_MARK: &str = "luxePushRoute";

async fn set_config(project: &Path, key: &str, value: &str) -> Result<(), WorktreeError> {
    git(project, &["config", "--replace-all", key, value]).await.map(drop).map_err(route_failed)
}

fn route_failed(detail: String) -> WorktreeError {
    WorktreeError::Git { operation: "set where a push goes", detail }
}

/// `text` as a regular expression matching only itself, for `git config`'s value patterns.
fn regex_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if r"\.^$|?*+()[]{}".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// A free remote name for a renamed branch's push route, or one already routing the same URL to
/// the same place, which is reused.
async fn push_route_name(
    project: &Path,
    remote: &str,
    head_ref: &str,
    url: &str,
    mapping: &str,
) -> String {
    let base = format!("{remote}-push-{}", head_ref.replace('/', "-"));
    let mut candidates = std::iter::once(base.clone()).chain((2..).map(|n| format!("{base}-{n}")));
    loop {
        let name = candidates.next().expect("an unbounded sequence has a free name");
        let (url_key, push_key) = (format!("remote.{name}.url"), format!("remote.{name}.push"));
        match git(project, &["config", "--get-all", &url_key]).await {
            Err(_) => return name,
            Ok(found) if found == url => {
                if git(project, &["config", "--get-all", &push_key]).await.as_deref() == Ok(mapping)
                {
                    return name;
                }
            }
            Ok(_) => {}
        }
    }
}
