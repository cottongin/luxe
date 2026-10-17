//! Writes — the changes a status row makes: update the branch, mark it ready, merge it.
//!
//! Each is one REST call, or `gh pr ready`, run so that a refusal is read from GitHub's own answer:
//! `gh api` prints the JSON body even when the call fails, and its `message` is what GitHub said,
//! where `gh`'s own wording on stderr is not ours to parse.

use super::*;
use crate::github::overview::Merged;

#[async_trait]
impl Writes for GhCli {
    async fn update_branch(
        &self,
        repository: &Repository,
        number: u64,
        expected_head: &str,
    ) -> Result<(), GitHubError> {
        let endpoint =
            format!("repos/{}/{}/pulls/{number}/update-branch", repository.owner, repository.name);
        let sha = format!("expected_head_sha={expected_head}");
        let mut args = vec!["api", "--method", "PUT"];
        push_hostname(&mut args, repository);
        args.extend([endpoint.as_str(), "-f", sha.as_str()]);
        let run = self.run(&std::env::temp_dir(), &args).await?;
        if run.success {
            Ok(())
        } else {
            Err(refusal(&run))
        }
    }

    async fn set_auto_merge(
        &self,
        repository: &Repository,
        node_id: &str,
        enable: Option<(crate::github::overview::MergeMethod, &str)>,
    ) -> Result<(), GitHubError> {
        use crate::github::overview::MergeMethod;
        let id = format!("id={node_id}");
        let mut args = vec!["api", "graphql"];
        push_hostname(&mut args, repository);
        let (query, field, extra) = match enable {
            Some((method, head)) => {
                let method = match method {
                    MergeMethod::Merge => "method=MERGE",
                    MergeMethod::Squash => "method=SQUASH",
                    MergeMethod::Rebase => "method=REBASE",
                };
                (
                    ENABLE_AUTO_MERGE,
                    "enablePullRequestAutoMerge",
                    vec![
                        "-F".to_string(),
                        method.to_string(),
                        "-F".to_string(),
                        format!("head={head}"),
                    ],
                )
            }
            None => (DISABLE_AUTO_MERGE, "disablePullRequestAutoMerge", Vec::new()),
        };
        let query = format!("query={query}");
        args.extend(["-f", query.as_str(), "-F", id.as_str()]);
        args.extend(extra.iter().map(String::as_str));
        let run = self.run(&std::env::temp_dir(), &args).await?;
        parse_auto_merge(&run, field)
    }

    async fn mark_ready(&self, repository: &Repository, number: u64) -> Result<(), GitHubError> {
        let number = number.to_string();
        let target = repo_arg(repository);
        // Already ready answers success too, which is what the reader wanted
        self.gh(&std::env::temp_dir(), &["pr", "ready", &number, "--repo", &target]).await?;
        Ok(())
    }

    async fn merge_pull_request(
        &self,
        repository: &Repository,
        number: u64,
        request: &crate::github::overview::MergeRequest,
    ) -> Result<crate::github::overview::Merged, GitHubError> {
        use crate::github::overview::MergeMethod;
        let endpoint =
            format!("repos/{}/{}/pulls/{number}/merge", repository.owner, repository.name);
        let method = format!(
            "merge_method={}",
            match request.method {
                MergeMethod::Merge => "merge",
                MergeMethod::Squash => "squash",
                MergeMethod::Rebase => "rebase",
            }
        );
        let sha = format!("sha={}", request.head_oid);
        let mut args = vec!["api", "--method", "PUT"];
        push_hostname(&mut args, repository);
        args.extend([endpoint.as_str(), "-f", method.as_str(), "-f", sha.as_str()]);
        let run = self.run(&std::env::temp_dir(), &args).await?;
        if !run.success {
            return Err(refusal(&run));
        }
        let commit = parse_merged(&run.stdout)?;
        let Some(branch) = &request.delete_branch else {
            return Ok(merged(commit, false));
        };
        // A repository that deletes branches on merge may have done it already: ask before deleting
        // rather than reading the refusal a second delete would draw
        let reference =
            format!("repos/{}/{}/git/ref/heads/{branch}", repository.owner, repository.name);
        let mut look = vec!["api"];
        push_hostname(&mut look, repository);
        look.push(reference.as_str());
        match self.gh(&std::env::temp_dir(), &look).await {
            Err(GitHubError::NotFound) => return Ok(merged(commit, true)),
            Err(error) => {
                tracing::warn!(%error, branch, "could not tell whether the merged branch remains");
                return Ok(merged(commit, false));
            }
            Ok(_) => {}
        }
        let reference =
            format!("repos/{}/{}/git/refs/heads/{branch}", repository.owner, repository.name);
        let mut delete = vec!["api", "--method", "DELETE"];
        push_hostname(&mut delete, repository);
        delete.push(reference.as_str());
        let deleted = self.gh(&std::env::temp_dir(), &delete).await;
        if let Err(error) = &deleted {
            tracing::warn!(%error, branch, "the merged branch could not be deleted");
        }
        Ok(merged(commit, deleted.is_ok()))
    }

    async fn retarget_pull_request(
        &self,
        repository: &Repository,
        number: u64,
        base: &str,
    ) -> Result<(), GitHubError> {
        let endpoint = format!("repos/{}/{}/pulls/{number}", repository.owner, repository.name);
        let field = format!("base={base}");
        let mut args = vec!["api", "--method", "PATCH"];
        push_hostname(&mut args, repository);
        args.extend([endpoint.as_str(), "-f", field.as_str(), "--silent"]);
        let ran = self.run(&std::env::temp_dir(), &args).await?;
        if ran.success {
            Ok(())
        } else {
            Err(rejection(&ran.stdout)
                .unwrap_or_else(|| classify_failure(&ran.stderr, &ran.stdout)))
        }
    }

    async fn open_pull_request(
        &self,
        repository: &Repository,
        pull_request: &NewPullRequest,
    ) -> Result<OpenedPullRequest, GitHubError> {
        let payload = serde_json::json!({
            "title": pull_request.title,
            "head": pull_request.head,
            "base": pull_request.base,
            "body": pull_request.body,
            "draft": pull_request.draft,
        });
        let input = PrivateFile::write(&payload.to_string())
            .map_err(|source| GitHubError::Run { program: "gh", source })?;
        let endpoint = format!("repos/{}/{}/pulls", repository.owner, repository.name);
        let path = input.path.to_string_lossy().into_owned();
        let mut args = vec!["api", "--method", "POST"];
        push_hostname(&mut args, repository);
        args.extend([endpoint.as_str(), "--input", path.as_str(), "--jq", "{number, html_url}"]);
        let ran = self.run(&std::env::temp_dir(), &args).await?;
        if ran.success {
            parse_opened(&ran.stdout)
        } else {
            Err(rejection(&ran.stdout)
                .unwrap_or_else(|| classify_failure(&ran.stderr, &ran.stdout)))
        }
    }
}

/// Turns on auto-merge, naming the head the reader saw so a push in between is refused.
pub(super) const ENABLE_AUTO_MERGE: &str = "mutation($id:ID!,$method:PullRequestMergeMethod!,\
     $head:GitObjectID!){enablePullRequestAutoMerge(input:{pullRequestId:$id,mergeMethod:$method,\
     expectedHeadOid:$head}){pullRequest{autoMergeRequest{mergeMethod}}}}";

/// Turns auto-merge off.
pub(super) const DISABLE_AUTO_MERGE: &str = "mutation($id:ID!){disablePullRequestAutoMerge(\
     input:{pullRequestId:$id}){pullRequest{autoMergeRequest{mergeMethod}}}}";

/// Why a write did not land: GitHub's own message when its answer carries one.
pub(super) fn refusal(run: &GhRun) -> GitHubError {
    #[derive(Deserialize)]
    struct Refused {
        message: String,
    }
    match serde_json::from_str::<Refused>(run.stdout.trim()) {
        Ok(refused) if !refused.message.trim().is_empty() => {
            GitHubError::Rejected { reasons: vec![crate::github::untrusted(&refused.message, 300)] }
        }
        _ => classify_failure(&run.stderr, &run.stdout),
    }
}

/// Whether an auto-merge mutation landed. GitHub answers a refusal with its own sentence in
/// `errors`, and a mutation that did nothing with a null in place of what it changed.
pub(super) fn parse_auto_merge(run: &GhRun, field: &str) -> Result<(), GitHubError> {
    #[derive(Deserialize)]
    struct Said {
        message: String,
    }
    #[derive(Deserialize)]
    struct Answer {
        data: Option<serde_json::Map<String, serde_json::Value>>,
        #[serde(default)]
        errors: Vec<Said>,
    }
    let answer: Answer = match serde_json::from_str(run.stdout.trim()) {
        Ok(answer) => answer,
        Err(_) if !run.success => return Err(classify_failure(&run.stderr, &run.stdout)),
        Err(error) => return Err(GitHubError::Unreadable(error)),
    };
    if let Some(said) = answer.errors.first().filter(|said| !said.message.trim().is_empty()) {
        return Err(GitHubError::Rejected {
            reasons: vec![crate::github::untrusted(&said.message, 300)],
        });
    }
    match answer.data.as_ref().and_then(|data| data.get(field)) {
        Some(changed) if !changed.is_null() => Ok(()),
        _ => {
            Err(GitHubError::Rejected { reasons: vec!["GitHub did not change auto-merge.".into()] })
        }
    }
}

/// The merge commit in a merge's answer.
pub(super) fn parse_merged(body: &str) -> Result<String, GitHubError> {
    #[derive(Deserialize)]
    struct Answer {
        sha: String,
        merged: bool,
    }
    let answer: Answer = serde_json::from_str(body).map_err(GitHubError::Unreadable)?;
    match answer.merged {
        true => Ok(answer.sha),
        false => Err(GitHubError::Rejected { reasons: vec!["GitHub did not merge it.".into()] }),
    }
}

/// A merge that landed, and whether its branch is gone.
pub(super) fn merged(sha: String, branch_deleted: bool) -> Merged {
    Merged { sha, branch_deleted }
}

#[derive(Deserialize)]
struct OpenedView {
    number: u64,
    html_url: String,
}

pub(super) fn parse_opened(body: &str) -> Result<OpenedPullRequest, GitHubError> {
    let view: OpenedView = serde_json::from_str(body).map_err(GitHubError::Unreadable)?;
    Ok(OpenedPullRequest { number: view.number, url: view.html_url })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(stdout: &str, stderr: &str) -> GhRun {
        GhRun { success: false, stdout: stdout.into(), stderr: stderr.into() }
    }

    fn ran(stdout: &str, success: bool) -> GhRun {
        GhRun { success, stdout: stdout.into(), stderr: String::new() }
    }

    /// Auto-merge on and off land; a moved head and a missing permission read as GitHub said
    /// them; a mutation answered with a bare null is a refusal, not a success.
    #[test]
    fn auto_merge_answers_are_read_as_github_gave_them() {
        let (enable, disable) = ("enablePullRequestAutoMerge", "disablePullRequestAutoMerge");
        let enabled = include_str!("../fixtures/pr_auto_merge_enabled.json");
        assert!(parse_auto_merge(&ran(enabled, true), enable).is_ok());
        let disabled = include_str!("../fixtures/pr_auto_merge_disabled.json");
        assert!(parse_auto_merge(&ran(disabled, true), disable).is_ok());

        let reason = |stdout: &str, field: &str| match parse_auto_merge(&ran(stdout, false), field)
        {
            Err(GitHubError::Rejected { reasons }) => reasons.join(" "),
            other => panic!("refused, got {other:?}"),
        };
        let moved = include_str!("../fixtures/pr_auto_merge_moved.json");
        assert!(reason(moved, enable).contains("expected head oid does not match"));
        let forbidden = include_str!("../fixtures/pr_auto_merge_forbidden.json");
        assert!(reason(forbidden, disable).contains("does not have the correct permissions"));
        assert!(parse_auto_merge(&ran(r#"{"data":null}"#, true), enable).is_err());
    }

    /// Each refusal reads as what GitHub said: a head that moved under a merge or an update.
    #[test]
    fn a_refusal_is_what_github_said() {
        let moved = failed(
            include_str!("../fixtures/pr_merge_409_moved.json"),
            "gh: Head branch was modified. Review and try the merge again. (HTTP 409)",
        );
        assert!(matches!(
            refusal(&moved),
            GitHubError::Rejected { reasons } if reasons == ["Head branch was modified. Review and try the merge again."]
        ));
        let stale = failed(
            include_str!("../fixtures/pr_update_branch_422_stale.json"),
            "gh: expected head sha didn’t match current head ref. (HTTP 422)",
        );
        assert!(matches!(
            refusal(&stale),
            GitHubError::Rejected { reasons } if reasons[0].starts_with("expected head sha")
        ));
        // Without a body to read, `gh`'s failure is classified as any other
        let signed_out = failed("", "To get started with GitHub CLI, please run:  gh auth login");
        assert!(matches!(refusal(&signed_out), GitHubError::NotAuthenticated { .. }));
    }

    /// A merge answers its commit — the same one when asked twice, so a retry after a lost answer
    /// does no harm.
    #[test]
    fn a_merge_answers_its_commit_and_a_second_ask_the_same() {
        let first = parse_merged(include_str!("../fixtures/pr_merge.json")).expect("merged");
        let again = parse_merged(include_str!("../fixtures/pr_merge_again.json")).expect("merged");
        assert_eq!(first, "abce6b1396d97556c3a8f0beafbb681b5963d714");
        assert_eq!(first, again);
        assert!(include_str!("../fixtures/pr_update_branch.json")
            .contains("Updating pull request branch."));
    }
}
