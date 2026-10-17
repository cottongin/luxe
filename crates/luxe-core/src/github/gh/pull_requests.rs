//! Pull requests — reading one pull request or issue, and its detail and conversation.

use super::*;

#[async_trait]
impl PullRequests for GhCli {
    async fn pull_request(
        &self,
        repository: &Repository,
        number: u64,
    ) -> Result<PullRequest, GitHubError> {
        let number = number.to_string();
        let target = repo_arg(repository);
        let args = ["pr", "view", &number, "--repo", &target, "--json", PULL_REQUEST_FIELDS];
        parse_pull_request(&self.gh(&std::env::temp_dir(), &args).await?)
    }

    async fn issue(&self, repository: &Repository, number: u64) -> Result<Issue, GitHubError> {
        let number = number.to_string();
        let target = repo_arg(repository);
        let args = [
            "issue",
            "view",
            &number,
            "--repo",
            &target,
            "--json",
            "number,title,body,url,author,comments",
        ];
        parse_issue(&self.gh(&std::env::temp_dir(), &args).await?)
    }

    async fn conversation(
        &self,
        repository: &Repository,
        number: u64,
        pull_request: bool,
    ) -> Result<Vec<crate::github::overview::ConversationEntry>, GitHubError> {
        let query = match pull_request {
            true => conversation::PULL_REQUEST_CONVERSATION_QUERY,
            false => conversation::ISSUE_CONVERSATION_QUERY,
        };
        let query = format!("query={query}");
        let (owner, name, number) = (
            format!("owner={}", repository.owner),
            format!("name={}", repository.name),
            format!("number={number}"),
        );
        let mut args = vec!["api", "graphql"];
        push_hostname(&mut args, repository);
        args.extend(["-f", &query, "-F", &owner, "-F", &name, "-F", &number]);
        let body = self.gh(&std::env::temp_dir(), &args).await?;
        conversation::parse_conversation(&body)
    }

    async fn issue_overview(
        &self,
        repository: &Repository,
        number: u64,
    ) -> Result<crate::github::overview::IssueOverview, GitHubError> {
        let query = format!("query={}", issue_overview::ISSUE_OVERVIEW_QUERY);
        let (owner, name, number) = (
            format!("owner={}", repository.owner),
            format!("name={}", repository.name),
            format!("number={number}"),
        );
        let mut args = vec!["api", "graphql"];
        push_hostname(&mut args, repository);
        args.extend(["-f", &query, "-F", &owner, "-F", &name, "-F", &number]);
        let body = self.gh(&std::env::temp_dir(), &args).await?;
        issue_overview::parse_issue_overview(&body)
    }

    async fn pull_request_overview(
        &self,
        repository: &Repository,
        number: u64,
    ) -> Result<crate::github::overview::PullRequestOverview, GitHubError> {
        let query = format!("query={}", overview::PULL_REQUEST_OVERVIEW_QUERY);
        let (owner, name, number) = (
            format!("owner={}", repository.owner),
            format!("name={}", repository.name),
            format!("number={number}"),
        );
        let mut args = vec!["api", "graphql"];
        push_hostname(&mut args, repository);
        args.extend(["-f", &query, "-F", &owner, "-F", &name, "-F", &number]);
        let body = self.gh(&std::env::temp_dir(), &args).await?;
        overview::parse_pull_request_overview(&body)
    }
}

/// The `gh pr view` fields a [`PullRequest`] is read from.
///
/// Not `commits`: it carries every commit's full message and author, which on a large pull request
/// is most of the payload, and the commit messages in range are read from git instead.
pub(super) const PULL_REQUEST_FIELDS: &str = "number,title,body,url,author,isDraft,state,baseRefName,baseRefOid,\
     headRefName,headRefOid,isCrossRepository,reviewDecision,statusCheckRollup,closingIssuesReferences";

/// The body of `gh pr view --json` with [`PULL_REQUEST_FIELDS`].
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PrView {
    number: u64,
    title: String,
    #[serde(default)]
    body: String,
    url: String,
    author: PrAuthor,
    is_draft: bool,
    state: String,
    base_ref_name: String,
    base_ref_oid: String,
    head_ref_name: String,
    head_ref_oid: String,
    #[serde(default)]
    is_cross_repository: bool,
    #[serde(default)]
    review_decision: Option<String>,
    #[serde(default)]
    status_check_rollup: Vec<CheckEntry>,
    #[serde(default)]
    closing_issues_references: Vec<ClosingIssue>,
}

#[derive(Deserialize)]
pub(super) struct PrAuthor {
    login: String,
    #[serde(default)]
    is_bot: bool,
}

/// One entry of `statusCheckRollup`: a GitHub Actions or app check run, or a commit status.
#[derive(Deserialize)]
#[serde(tag = "__typename")]
pub(super) enum CheckEntry {
    CheckRun {
        #[serde(default)]
        status: String,
        #[serde(default)]
        conclusion: String,
    },
    StatusContext {
        #[serde(default)]
        state: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
pub(super) struct ClosingIssue {
    number: u64,
    repository: ClosingRepository,
}

#[derive(Deserialize)]
pub(super) struct ClosingRepository {
    name: String,
    owner: ClosingOwner,
}

#[derive(Deserialize)]
pub(super) struct ClosingOwner {
    login: String,
}

pub(super) fn parse_pull_request(body: &str) -> Result<PullRequest, GitHubError> {
    let view: PrView = serde_json::from_str(body).map_err(GitHubError::Unreadable)?;
    let state = match view.state.as_str() {
        "MERGED" => PullRequestState::Merged,
        "CLOSED" => PullRequestState::Closed,
        _ => PullRequestState::Open,
    };
    // An empty string, not null, is how GitHub says the repository asks for no review
    let review_decision = match view.review_decision.as_deref() {
        Some("APPROVED") => Some(ReviewDecision::Approved),
        Some("CHANGES_REQUESTED") => Some(ReviewDecision::ChangesRequested),
        Some("REVIEW_REQUIRED") => Some(ReviewDecision::ReviewRequired),
        _ => None,
    };
    let bot = view.author.is_bot || view.author.login.ends_with("[bot]");
    Ok(PullRequest {
        number: view.number,
        title: view.title,
        body: view.body,
        url: view.url,
        author: Author { login: view.author.login, bot },
        draft: view.is_draft,
        state,
        base_ref: view.base_ref_name,
        base_oid: view.base_ref_oid,
        head_ref: view.head_ref_name,
        head_oid: view.head_ref_oid,
        cross_repository: view.is_cross_repository,
        review_decision,
        checks: roll_up(&view.status_check_rollup),
        closing_issues: view
            .closing_issues_references
            .into_iter()
            .map(|issue| IssueRef {
                repository: format!("{}/{}", issue.repository.owner.login, issue.repository.name),
                number: issue.number,
            })
            .collect(),
    })
}

/// Counts checks by outcome across both kinds GitHub reports.
///
/// A check run that has not completed is pending whatever its conclusion field says; a commit
/// status has only a state. Anything unrecognized is left out of the count rather than guessed at.
pub(super) fn roll_up(entries: &[CheckEntry]) -> CheckRollup {
    let mut rollup = CheckRollup::default();
    for entry in entries {
        let bucket = match entry {
            CheckEntry::CheckRun { status, .. } if status != "COMPLETED" => &mut rollup.pending,
            CheckEntry::CheckRun { conclusion, .. } => match conclusion.as_str() {
                "SUCCESS" => &mut rollup.passed,
                "NEUTRAL" | "SKIPPED" => &mut rollup.skipped,
                "FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE"
                | "STALE" => &mut rollup.failed,
                _ => continue,
            },
            CheckEntry::StatusContext { state } => match state.as_str() {
                "SUCCESS" => &mut rollup.passed,
                "PENDING" | "EXPECTED" => &mut rollup.pending,
                "FAILURE" | "ERROR" => &mut rollup.failed,
                _ => continue,
            },
            CheckEntry::Other => continue,
        };
        *bucket += 1;
    }
    rollup
}

#[derive(Deserialize)]
pub(super) struct IssueView {
    number: u64,
    title: String,
    #[serde(default)]
    body: String,
    url: String,
    #[serde(default)]
    author: Option<IssueAuthor>,
    #[serde(default)]
    comments: Vec<IssueCommentView>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct IssueCommentView {
    #[serde(default)]
    author: Option<IssueAuthor>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    is_minimized: bool,
}

/// How many of an issue's comments are kept: the latest, since that is where a discussion lands.
pub(super) const ISSUE_COMMENTS_KEPT: usize = 5;

pub(super) fn parse_issue(body: &str) -> Result<Issue, GitHubError> {
    let view: IssueView = serde_json::from_str(body).map_err(GitHubError::Unreadable)?;
    let total_comments = view.comments.len();
    // Hidden as spam or abuse by the repository's maintainers: not part of the discussion
    let visible: Vec<IssueComment> = view
        .comments
        .into_iter()
        .filter(|comment| !comment.is_minimized)
        .map(|comment| IssueComment {
            author: comment.author.map(|author| author.login).unwrap_or_default(),
            body: comment.body,
        })
        .collect();
    let skip = visible.len().saturating_sub(ISSUE_COMMENTS_KEPT);
    Ok(Issue {
        number: view.number,
        title: view.title,
        body: view.body,
        url: view.url,
        author: view.author.map(|author| author.login).unwrap_or_default(),
        comments: visible.into_iter().skip(skip).collect(),
        total_comments,
    })
}
