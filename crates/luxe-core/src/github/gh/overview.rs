//! Overview query — one pull request as its detail shows it, in one GraphQL request.
//!
//! What GitHub knows about whether it can merge (`mergeStateStatus`), who was asked to review and
//! what each reviewer last said, which checks are failing and whether branch protection requires
//! them, and what the signed-in account may do with it. About one point of the hourly budget.

use serde::Deserialize;

use super::{count_checks, rfc3339_seconds, Connection, Login, Nodes, RollupContexts};
use crate::github::overview::{
    FailingCheckName, Label, MergeMethod, MergeSettings, MergeStatus, PullRequestOverview,
    Reviewer, ReviewerKind, Verdict, Viewer,
};
use crate::github::{Author, GitHubError, PullRequestState, ReviewDecision};

/// The query. `isRequired` takes the pull request's number because a check is required of a pull
/// request by the rules on its base, not of a commit.
pub(super) const PULL_REQUEST_OVERVIEW_QUERY: &str = "query($owner:String!,$name:String!,\
     $number:Int!){repository(owner:$owner,name:$name){nameWithOwner viewerPermission \
     mergeCommitAllowed squashMergeAllowed rebaseMergeAllowed viewerDefaultMergeMethod \
     deleteBranchOnMerge pullRequest(number:$number){id number viewerCanEnableAutoMerge \
     viewerCanDisableAutoMerge autoMergeRequest{mergeMethod} \
     title body url state isDraft baseRefName headRefName headRefOid additions deletions \
     changedFiles updatedAt mergeStateStatus mergeable reviewDecision viewerDidAuthor isCrossRepository \
     maintainerCanModify headRepository{viewerPermission} author{__typename login} \
     labels(first:10){nodes{name color}} commitCount:commits{totalCount} \
     head:commits(last:1){nodes{commit{statusCheckRollup{contexts(first:100){\
     checkRunCountsByState{state count} statusContextCountsByState{state count} \
     nodes{__typename ...on CheckRun{name conclusion status isRequired(pullRequestNumber:$number)} \
     ...on StatusContext{context state isRequired(pullRequestNumber:$number)}}}}}}} \
     reviewRequests(first:20){nodes{requestedReviewer{__typename ...on User{login} \
     ...on Bot{login} ...on Mannequin{login} ...on Team{slug organization{login}}}}} \
     latestOpinionatedReviews(first:20){nodes{author{__typename login} state submittedAt}} \
     latestReviews(first:20){nodes{author{__typename login} state submittedAt}} \
     reviewThreads(first:100){totalCount nodes{isResolved}} comments{totalCount} \
     reviews{totalCount}}} rateLimit{cost remaining resetAt}}";

#[derive(Deserialize)]
struct Answer {
    data: Data,
}

#[derive(Deserialize)]
struct Data {
    repository: Option<RepositoryNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryNode {
    name_with_owner: String,
    viewer_permission: Option<String>,
    merge_commit_allowed: bool,
    squash_merge_allowed: bool,
    rebase_merge_allowed: bool,
    viewer_default_merge_method: Option<String>,
    #[serde(default)]
    delete_branch_on_merge: bool,
    pull_request: Option<PullRequestNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutoMergeNode {
    merge_method: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestNode {
    number: u64,
    #[serde(default)]
    id: String,
    #[serde(default)]
    viewer_can_enable_auto_merge: bool,
    #[serde(default)]
    viewer_can_disable_auto_merge: bool,
    auto_merge_request: Option<AutoMergeNode>,
    title: String,
    #[serde(default)]
    body: String,
    url: String,
    state: String,
    is_draft: bool,
    base_ref_name: String,
    head_ref_name: String,
    head_ref_oid: String,
    additions: u64,
    deletions: u64,
    changed_files: u64,
    updated_at: String,
    merge_state_status: String,
    mergeable: String,
    review_decision: Option<String>,
    viewer_did_author: bool,
    is_cross_repository: bool,
    maintainer_can_modify: bool,
    head_repository: Option<PermissionNode>,
    author: Option<Actor>,
    labels: Nodes<LabelNode>,
    commit_count: Count,
    head: Nodes<HeadCommit>,
    review_requests: Nodes<RequestNode>,
    latest_opinionated_reviews: Nodes<ReviewNode>,
    latest_reviews: Nodes<ReviewNode>,
    review_threads: Connection<ResolvedThread>,
    comments: Count,
    reviews: Count,
}

/// A connection asked only how long it is.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Count {
    total_count: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PermissionNode {
    viewer_permission: Option<String>,
}

/// A user, bot or mannequin; the typename is what tells a bot from a person.
#[derive(Deserialize)]
struct Actor {
    #[serde(rename = "__typename")]
    typename: String,
    login: String,
}

#[derive(Deserialize)]
struct LabelNode {
    name: String,
    color: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeadCommit {
    commit: HeadRollup,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeadRollup {
    status_check_rollup: Option<Rollup>,
}

#[derive(Deserialize)]
struct Rollup {
    contexts: Contexts,
}

#[derive(Deserialize)]
struct Contexts {
    #[serde(flatten)]
    counts: RollupContexts,
    #[serde(default)]
    nodes: Vec<CheckNode>,
}

/// A check run or a commit status; each names itself and says how it ended its own way.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckNode {
    name: Option<String>,
    context: Option<String>,
    conclusion: Option<String>,
    state: Option<String>,
    #[serde(default)]
    is_required: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestNode {
    /// Absent for a team the signed-in account cannot see
    requested_reviewer: Option<Requested>,
}

#[derive(Deserialize)]
struct Requested {
    #[serde(rename = "__typename")]
    typename: String,
    login: Option<String>,
    slug: Option<String>,
    organization: Option<Login>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewNode {
    author: Option<Actor>,
    state: String,
    submitted_at: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolvedThread {
    is_resolved: bool,
}

fn pull_request_state(state: &str) -> PullRequestState {
    match state {
        "MERGED" => PullRequestState::Merged,
        "CLOSED" => PullRequestState::Closed,
        _ => PullRequestState::Open,
    }
}

fn review_decision(decision: Option<&str>) -> Option<ReviewDecision> {
    match decision {
        Some("APPROVED") => Some(ReviewDecision::Approved),
        Some("CHANGES_REQUESTED") => Some(ReviewDecision::ChangesRequested),
        Some("REVIEW_REQUIRED") => Some(ReviewDecision::ReviewRequired),
        _ => None,
    }
}

/// What the viewer may do to the pull request: push to its branch, and merge it.
///
/// Args: `(base permission, head repository permission, (cross repository, maintainers may
/// modify, viewer authored it))`.
fn viewer_rights(
    base: Option<&str>,
    head: Option<&str>,
    (cross_repository, maintainer_can_modify, authored): (bool, bool, bool),
) -> Viewer {
    let writes =
        |permission: Option<&str>| matches!(permission, Some("WRITE" | "MAINTAIN" | "ADMIN"));
    let base_writable = writes(base);
    let can_push = match cross_repository {
        false => base_writable,
        true => writes(head) || (maintainer_can_modify && base_writable),
    };
    Viewer { author: authored, can_push, can_merge: base_writable }
}

/// The merge methods the repository allows, in the order GitHub's own menu lists them.
///
/// Args: `[merge commit, squash, rebase]` allowed.
fn allowed_merge_methods([merge, squash, rebase]: [bool; 3]) -> Vec<MergeMethod> {
    [(merge, MergeMethod::Merge), (squash, MergeMethod::Squash), (rebase, MergeMethod::Rebase)]
        .into_iter()
        .filter_map(|(allowed, method)| allowed.then_some(method))
        .collect()
}

/// The overview in `body`, the answer to [`PULL_REQUEST_OVERVIEW_QUERY`].
pub(super) fn parse_pull_request_overview(body: &str) -> Result<PullRequestOverview, GitHubError> {
    let answer: Answer = serde_json::from_str(body).map_err(GitHubError::Unreadable)?;
    let repository = answer.data.repository.ok_or(GitHubError::NotFound)?;
    let node = repository.pull_request.ok_or(GitHubError::NotFound)?;

    let rollup =
        node.head.nodes.into_iter().next().and_then(|head| head.commit.status_check_rollup);
    let (checks, failing) = match rollup {
        Some(rollup) => (count_checks(&rollup.contexts.counts), failing(rollup.contexts.nodes)),
        None => Default::default(),
    };
    let viewer = viewer_rights(
        repository.viewer_permission.as_deref(),
        node.head_repository.as_ref().and_then(|head| head.viewer_permission.as_deref()),
        (node.is_cross_repository, node.maintainer_can_modify, node.viewer_did_author),
    );
    let author = node.author.map_or_else(
        || Author { login: String::new(), bot: false },
        |actor| Author { bot: actor.typename == "Bot", login: actor.login },
    );
    let merge_methods = allowed_merge_methods([
        repository.merge_commit_allowed,
        repository.squash_merge_allowed,
        repository.rebase_merge_allowed,
    ]);
    let threads = node.review_threads.total_count;
    Ok(PullRequestOverview {
        number: node.number,
        node_id: node.id,
        title: node.title,
        body: node.body,
        url: node.url,
        repository: repository.name_with_owner,
        author,
        state: pull_request_state(&node.state),
        draft: node.is_draft,
        base_ref: node.base_ref_name,
        head_ref: node.head_ref_name,
        head_oid: node.head_ref_oid,
        cross_repository: node.is_cross_repository,
        additions: node.additions,
        deletions: node.deletions,
        changed_files: node.changed_files,
        commits: node.commit_count.total_count as u64,
        updated_at: rfc3339_seconds(&node.updated_at).unwrap_or_default(),
        labels: node
            .labels
            .nodes
            .into_iter()
            .map(|label| Label { name: label.name, color: label.color })
            .collect(),
        merge_status: merge_status(&node.merge_state_status, &node.mergeable),
        review_decision: review_decision(node.review_decision.as_deref()),
        reviewers: reviewers(
            node.latest_opinionated_reviews.nodes,
            node.latest_reviews.nodes,
            node.review_requests.nodes,
        ),
        checks,
        failing,
        unresolved_threads: node
            .review_threads
            .nodes
            .iter()
            .filter(|thread| !thread.is_resolved)
            .count(),
        stack: None,
        viewer,
        merge_methods,
        merge_settings: MergeSettings {
            last_method: repository.viewer_default_merge_method.as_deref().and_then(merge_method),
            deletes_branch: repository.delete_branch_on_merge,
            can_auto_merge: node.viewer_can_enable_auto_merge,
            can_cancel_auto_merge: node.viewer_can_disable_auto_merge,
            auto_merge: node
                .auto_merge_request
                .and_then(|request| merge_method(&request.merge_method)),
        },
        // The description, then every comment, review and thread: what Conversation lists
        conversation_count: 1 + node.comments.total_count + node.reviews.total_count + threads,
    })
}

/// GitHub's `PullRequestMergeMethod`.
fn merge_method(word: &str) -> Option<MergeMethod> {
    match word {
        "MERGE" => Some(MergeMethod::Merge),
        "SQUASH" => Some(MergeMethod::Squash),
        "REBASE" => Some(MergeMethod::Rebase),
        _ => None,
    }
}

/// GitHub's word for whether it can merge. A conflict reported by `mergeable` is a conflict even
/// while `mergeStateStatus` is still catching up.
fn merge_status(state: &str, mergeable: &str) -> MergeStatus {
    if mergeable == "CONFLICTING" {
        return MergeStatus::Dirty;
    }
    match state {
        "CLEAN" => MergeStatus::Clean,
        "HAS_HOOKS" => MergeStatus::HasHooks,
        "UNSTABLE" => MergeStatus::Unstable,
        "BEHIND" => MergeStatus::Behind,
        "BLOCKED" => MergeStatus::Blocked,
        "DIRTY" => MergeStatus::Dirty,
        "DRAFT" => MergeStatus::Draft,
        _ => MergeStatus::Unknown,
    }
}

/// The checks that failed, required ones first, each by the name GitHub lists it under.
fn failing(nodes: Vec<CheckNode>) -> Vec<FailingCheckName> {
    let mut failed: Vec<FailingCheckName> = nodes
        .into_iter()
        .filter(|node| {
            let run = matches!(
                node.conclusion.as_deref(),
                Some(
                    "FAILURE"
                        | "TIMED_OUT"
                        | "CANCELLED"
                        | "ACTION_REQUIRED"
                        | "STARTUP_FAILURE"
                        | "STALE"
                )
            );
            let status = matches!(node.state.as_deref(), Some("FAILURE" | "ERROR"));
            run || status
        })
        .filter_map(|node| {
            let name = node.name.or(node.context)?;
            Some(FailingCheckName { name, required: node.is_required })
        })
        .collect();
    // Stable, so checks keep GitHub's order within each group
    failed.sort_by_key(|check| !check.required);
    failed
}

/// Everyone with a say: each reviewer's latest verdict, then whoever is asked and has not answered.
///
/// An opinionated review — approval or changes requested — is what counts toward merging, so it
/// wins over a later comment from the same person. A request to someone who already reviewed is a
/// re-request, which GitHub treats as pending again, so it wins over both.
fn reviewers(
    opinionated: Vec<ReviewNode>,
    latest: Vec<ReviewNode>,
    requested: Vec<RequestNode>,
) -> Vec<Reviewer> {
    let mut reviewers: Vec<Reviewer> = Vec::new();
    for review in opinionated.into_iter().chain(latest) {
        let Some(actor) = review.author else { continue };
        if reviewers.iter().any(|known| known.login == actor.login) {
            continue;
        }
        let verdict = match review.state.as_str() {
            "APPROVED" => Verdict::Approved,
            "CHANGES_REQUESTED" => Verdict::ChangesRequested,
            "DISMISSED" => Verdict::Dismissed,
            "COMMENTED" => Verdict::Commented,
            _ => continue,
        };
        reviewers.push(Reviewer {
            kind: if actor.typename == "Bot" { ReviewerKind::Bot } else { ReviewerKind::User },
            login: actor.login,
            verdict,
            at: review.submitted_at.as_deref().and_then(rfc3339_seconds),
        });
    }
    for request in requested {
        let Some(requested) = request.requested_reviewer else { continue };
        let (login, kind) = match requested.typename.as_str() {
            "Team" => {
                let organization = requested.organization.map(|org| org.login).unwrap_or_default();
                let slug = requested.slug.unwrap_or_default();
                (format!("{organization}/{slug}"), ReviewerKind::Team)
            }
            "Bot" => (requested.login.unwrap_or_default(), ReviewerKind::Bot),
            _ => (requested.login.unwrap_or_default(), ReviewerKind::User),
        };
        if login.is_empty() || login == "/" {
            continue;
        }
        match reviewers.iter_mut().find(|known| known.login == login) {
            Some(known) => known.verdict = Verdict::Requested,
            None => reviewers.push(Reviewer { login, kind, verdict: Verdict::Requested, at: None }),
        }
    }
    reviewers
}

#[cfg(test)]
mod tests;
