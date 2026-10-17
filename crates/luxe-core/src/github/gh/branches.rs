//! Branches — a branch's pull request and stack, where its head lives, and what merged below it.

use super::*;

mod stack;

pub(super) use stack::*;

#[async_trait]
impl Branches for GhCli {
    async fn branch_status(
        &self,
        repository: &Repository,
        head: &BranchHead,
    ) -> Result<BranchStatus, GitHubError> {
        let query = format!("query={BRANCH_STATUS_QUERY}");
        let (owner, name, branch) = (
            format!("owner={}", repository.owner),
            format!("name={}", repository.name),
            format!("branch={}", head.name),
        );
        let mut args = vec!["api", "graphql"];
        push_hostname(&mut args, repository);
        // `-f`, not `-F`, for the branch: `-F` reads `@name` as a file and `123` as a number
        args.extend(["-f", &query, "-F", &owner, "-F", &name, "-f", &branch]);
        let body = self.gh(&std::env::temp_dir(), &args).await?;
        parse_branch_status(&body, &head.name, head.owner.as_deref().unwrap_or(&repository.owner))
    }

    async fn pull_request_head(
        &self,
        repository: &Repository,
        number: u64,
    ) -> Result<PullRequestHead, GitHubError> {
        let query = format!("query={PULL_REQUEST_HEAD_QUERY}");
        let (owner, name, number) = (
            format!("owner={}", repository.owner),
            format!("name={}", repository.name),
            format!("number={number}"),
        );
        let mut args = vec!["api", "graphql"];
        push_hostname(&mut args, repository);
        args.extend(["-f", &query, "-F", &owner, "-F", &name, "-F", &number]);
        let body = self.gh(&std::env::temp_dir(), &args).await?;
        parse_pull_request_head(&body)
    }

    async fn merged_below(
        &self,
        repository: &Repository,
        number: u64,
        base: &str,
    ) -> Result<Option<crate::restack::MergedBelow>, GitHubError> {
        let (owner, name) =
            (format!("owner={}", repository.owner), format!("name={}", repository.name));
        let query = format!("query={MERGED_BELOW_QUERY}");
        let (number, base_arg) = (format!("number={number}"), format!("base={base}"));
        let mut args = vec!["api", "graphql"];
        push_hostname(&mut args, repository);
        args.extend(["-f", &query, "-F", &owner, "-F", &name, "-F", &number, "-f", &base_arg]);
        let body = self.gh(&std::env::temp_dir(), &args).await?;
        let (merged, previous) = parse_merged_below(&body)?;
        if merged.is_some() {
            return Ok(merged);
        }
        let Some(previous) = previous.filter(|previous| previous != base) else {
            return Ok(None);
        };
        let query = format!("query={MERGED_BY_HEAD_QUERY}");
        let head = format!("head={previous}");
        let mut args = vec!["api", "graphql"];
        push_hostname(&mut args, repository);
        args.extend(["-f", &query, "-F", &owner, "-F", &name, "-f", &head]);
        let body = self.gh(&std::env::temp_dir(), &args).await?;
        Ok(parse_merged_below(&body)?.0)
    }
}

pub(super) const MERGED_BELOW_QUERY: &str =
    "query($owner:String!,$name:String!,$number:Int!,$base:String!){\
     repository(owner:$owner,name:$name){pullRequest(number:$number){\
     timelineItems(itemTypes:[BASE_REF_CHANGED_EVENT],last:1){nodes{...on BaseRefChangedEvent{\
     previousRefName}}}} merged:pullRequests(headRefName:$base,states:MERGED,first:1,\
     orderBy:{field:UPDATED_AT,direction:DESC}){nodes{number headRefOid baseRefName}}}}";

pub(super) const MERGED_BY_HEAD_QUERY: &str = "query($owner:String!,$name:String!,$head:String!){\
     repository(owner:$owner,name:$name){merged:pullRequests(headRefName:$head,states:MERGED,\
     first:1,orderBy:{field:UPDATED_AT,direction:DESC}){nodes{number headRefOid baseRefName}}}}";

pub(super) const PULL_REQUEST_HEAD_QUERY: &str =
    "query($owner:String!,$name:String!,$number:Int!){\
     repository(owner:$owner,name:$name){viewerPermission defaultBranchRef{name} \
     pullRequest(number:$number){headRefName isCrossRepository maintainerCanModify \
     headRepositoryOwner{login} headRepository{name viewerPermission}}}}";

/// A branch's pull requests, newest first, with what a status row needs and the budget left. One
/// point: the check counts come from GitHub's own tally rather than a list of every check.
pub(super) const BRANCH_STATUS_QUERY: &str = "query($owner:String!,$name:String!,$branch:String!){\
     repository(owner:$owner,name:$name){defaultBranchRef{name} pullRequests(headRefName:$branch,\
     first:10,orderBy:{field:UPDATED_AT,direction:DESC},states:[OPEN,MERGED,CLOSED]){nodes{number \
     title url state isDraft headRefOid baseRefName headRepositoryOwner{login} reviewDecision \
     commits(last:1){nodes{commit{statusCheckRollup{contexts(first:1){checkRunCountsByState{state \
     count} statusContextCountsByState{state count}}}}}} reviewThreads(first:100){totalCount \
     nodes{isResolved}}}} stack:pullRequests(states:OPEN,first:100,orderBy:{field:UPDATED_AT,\
     direction:DESC}){nodes{number title isDraft headRefName baseRefName headRepositoryOwner{login} \
     commits(last:1){nodes{commit{statusCheckRollup{state}}}}}}} rateLimit{remaining resetAt}}";

// === Branch status ===

#[derive(Deserialize)]
pub(super) struct StatusAnswer {
    data: StatusData,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StatusData {
    repository: Option<StatusRepository>,
    rate_limit: Option<RateLimitNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StatusRepository {
    #[serde(default)]
    default_branch_ref: Option<BranchName>,
    pull_requests: Nodes<StatusNode>,
    #[serde(default)]
    stack: Option<Nodes<StackNode>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StatusNode {
    number: u64,
    title: String,
    url: String,
    state: String,
    is_draft: bool,
    head_ref_oid: String,
    #[serde(default)]
    base_ref_name: String,
    head_repository_owner: Option<Login>,
    review_decision: Option<String>,
    commits: Nodes<CommitNode>,
    review_threads: Nodes<ResolvedNode>,
}

#[derive(Deserialize)]
pub(super) struct CommitNode {
    commit: StatusCommit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StatusCommit {
    status_check_rollup: Option<StatusRollup>,
}

#[derive(Deserialize)]
pub(super) struct StatusRollup {
    contexts: RollupContexts,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ResolvedNode {
    is_resolved: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RateLimitNode {
    remaining: u32,
    reset_at: String,
}

/// The merged pull request an answer names, and the base the stack's bottom was moved from.
pub(super) fn parse_merged_below(
    body: &str,
) -> Result<(Option<crate::restack::MergedBelow>, Option<String>), GitHubError> {
    #[derive(Deserialize)]
    struct Answer {
        data: Data,
    }
    #[derive(Deserialize)]
    struct Data {
        repository: Option<Repo>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Repo {
        #[serde(default)]
        pull_request: Option<Pr>,
        merged: Nodes<Merged>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Pr {
        timeline_items: Nodes<Changed>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Changed {
        #[serde(default)]
        previous_ref_name: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Merged {
        number: u64,
        head_ref_oid: String,
        base_ref_name: String,
    }
    let answer: Answer = serde_json::from_str(body).map_err(GitHubError::Unreadable)?;
    let repository = answer.data.repository.ok_or(GitHubError::NotFound)?;
    let previous = repository
        .pull_request
        .and_then(|pr| pr.timeline_items.nodes.into_iter().last())
        .and_then(|changed| changed.previous_ref_name);
    let merged =
        repository.merged.nodes.into_iter().next().map(|merged| crate::restack::MergedBelow {
            number: merged.number,
            head_oid: merged.head_ref_oid,
            base_ref: merged.base_ref_name,
        });
    Ok((merged, previous))
}

#[derive(Deserialize)]
pub(super) struct HeadAnswer {
    data: HeadData,
}

#[derive(Deserialize)]
pub(super) struct HeadData {
    repository: Option<HeadRepositoryView>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HeadRepositoryView {
    viewer_permission: Option<String>,
    default_branch_ref: Option<BranchName>,
    pull_request: Option<HeadPullRequest>,
}

#[derive(Deserialize)]
pub(super) struct BranchName {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HeadPullRequest {
    head_ref_name: String,
    is_cross_repository: bool,
    maintainer_can_modify: bool,
    head_repository_owner: Option<IssueAuthor>,
    head_repository: Option<HeadRepository>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HeadRepository {
    name: String,
    viewer_permission: Option<String>,
}

/// Whether GitHub's permission name lets its holder push.
pub(super) fn can_write(permission: Option<&str>) -> bool {
    matches!(permission, Some("ADMIN" | "MAINTAIN" | "WRITE"))
}

pub(super) fn parse_pull_request_head(body: &str) -> Result<PullRequestHead, GitHubError> {
    let answer: HeadAnswer = serde_json::from_str(body).map_err(GitHubError::Unreadable)?;
    let repository = answer.data.repository.ok_or(GitHubError::NotFound)?;
    let pull_request = repository.pull_request.ok_or(GitHubError::NotFound)?;
    let owner = pull_request.head_repository_owner.map(|owner| owner.login).unwrap_or_default();
    let maintainer = can_write(repository.viewer_permission.as_deref());
    let (name, push) = match &pull_request.head_repository {
        None => (String::new(), PushAccess::HeadRepositoryGone),
        Some(head) if can_write(head.viewer_permission.as_deref()) => {
            (head.name.clone(), PushAccess::Allowed)
        }
        // A maintainer may push to a fork's branch only when its author allowed it
        Some(head) if pull_request.is_cross_repository && maintainer => (
            head.name.clone(),
            if pull_request.maintainer_can_modify {
                PushAccess::Allowed
            } else {
                PushAccess::MaintainerEditsOff
            },
        ),
        Some(head) => (head.name.clone(), PushAccess::NoWriteAccess),
    };
    let default_branch = repository.default_branch_ref.map(|branch| branch.name);
    let head_ref = pull_request.head_ref_name;
    let local_branch = if pull_request.is_cross_repository
        && default_branch.as_deref() == Some(head_ref.as_str())
    {
        format!("{owner}/{head_ref}")
    } else {
        head_ref.clone()
    };
    Ok(PullRequestHead {
        head_ref,
        owner,
        name,
        cross_repository: pull_request.is_cross_repository,
        local_branch,
        push,
    })
}

/// The branch's pull request among those whose head has its name, and the budget left.
///
/// Only a pull request from `head_owner`'s repository is the branch's: a fork's branch with the
/// same name is someone else's. An open one wins; otherwise the newest, so a branch whose pull
/// request merged says so rather than looking like it never had one.
pub(super) fn parse_branch_status(
    body: &str,
    head_ref: &str,
    head_owner: &str,
) -> Result<BranchStatus, GitHubError> {
    let answer: StatusAnswer = serde_json::from_str(body).map_err(GitHubError::Unreadable)?;
    let rate_limit = answer.data.rate_limit.and_then(|limit| {
        Some(RateLimit { remaining: limit.remaining, reset_at: rfc3339_seconds(&limit.reset_at)? })
    });
    let repository = answer.data.repository.ok_or(GitHubError::NotFound)?;
    let default_branch = repository.default_branch_ref.map(|branch| branch.name);
    let open = repository.stack.map(|stack| stack.nodes).unwrap_or_default();
    let candidates: Vec<StatusNode> = repository
        .pull_requests
        .nodes
        .into_iter()
        .filter(|node| {
            node.head_repository_owner
                .as_ref()
                .is_some_and(|owner| owner.login.eq_ignore_ascii_case(head_owner))
        })
        .collect();
    let chosen = match candidates.iter().position(|node| node.state == "OPEN") {
        Some(open) => candidates.into_iter().nth(open),
        None => candidates.into_iter().next(),
    };
    let pull_request = chosen.map(|node| {
        let checks = node
            .commits
            .nodes
            .into_iter()
            .next()
            .and_then(|commit| commit.commit.status_check_rollup)
            .map(|rollup| count_checks(&rollup.contexts))
            .unwrap_or_default();
        PullRequestStatus {
            number: node.number,
            title: status_text(&node.title),
            url: node.url,
            state: match node.state.as_str() {
                "MERGED" => PullRequestState::Merged,
                "CLOSED" => PullRequestState::Closed,
                _ => PullRequestState::Open,
            },
            draft: node.is_draft,
            review_decision: match node.review_decision.as_deref() {
                Some("APPROVED") => Some(ReviewDecision::Approved),
                Some("CHANGES_REQUESTED") => Some(ReviewDecision::ChangesRequested),
                Some("REVIEW_REQUIRED") => Some(ReviewDecision::ReviewRequired),
                _ => None,
            },
            checks,
            unresolved_threads: node
                .review_threads
                .nodes
                .iter()
                .filter(|thread| !thread.is_resolved)
                .count(),
            stack: stack_of(
                node.number,
                head_ref,
                &node.base_ref_name,
                &open,
                head_owner,
                default_branch.as_deref(),
            ),
            head_oid: node.head_ref_oid,
        }
    });
    Ok(BranchStatus { pull_request, rate_limit })
}
