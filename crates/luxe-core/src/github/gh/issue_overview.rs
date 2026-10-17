//! Issue overview query — one issue as its detail shows it, in one GraphQL request.
//!
//! Who it is assigned to, the milestone it is in, and the pull requests that would close it —
//! GitHub's Development list, closed attempts included, since a closed one is history worth seeing
//! before starting another. About one point of the hourly budget.

use serde::Deserialize;

use super::{rfc3339_seconds, Login, Nodes};
use crate::github::overview::{IssueOverview, Label, LinkedPullRequest};
use crate::github::{Author, GitHubError, PullRequestState};

pub(super) const ISSUE_OVERVIEW_QUERY: &str = "query($owner:String!,$name:String!,$number:Int!){\
     repository(owner:$owner,name:$name){nameWithOwner issue(number:$number){number title body \
     url state updatedAt author{__typename login} labels(first:10){nodes{name color}} \
     assignees(first:10){nodes{login}} milestone{title} \
     closedByPullRequestsReferences(first:10,includeClosedPrs:true){nodes{number title state \
     isDraft author{login}}} comments{totalCount}}} rateLimit{cost remaining}}";

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
    issue: Option<IssueNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssueNode {
    number: u64,
    title: String,
    #[serde(default)]
    body: String,
    url: String,
    state: String,
    updated_at: String,
    author: Option<Actor>,
    labels: Nodes<LabelNode>,
    assignees: Nodes<Login>,
    milestone: Option<Milestone>,
    closed_by_pull_requests_references: Nodes<Linked>,
    comments: Count,
}

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
struct Milestone {
    title: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Linked {
    number: u64,
    title: String,
    state: String,
    is_draft: bool,
    author: Option<Login>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Count {
    total_count: usize,
}

fn state(state: &str) -> PullRequestState {
    match state {
        "MERGED" => PullRequestState::Merged,
        "CLOSED" => PullRequestState::Closed,
        _ => PullRequestState::Open,
    }
}

/// The overview in `body`, the answer to [`ISSUE_OVERVIEW_QUERY`].
pub(super) fn parse_issue_overview(body: &str) -> Result<IssueOverview, GitHubError> {
    let answer: Answer = serde_json::from_str(body).map_err(GitHubError::Unreadable)?;
    let repository = answer.data.repository.ok_or(GitHubError::NotFound)?;
    let node = repository.issue.ok_or(GitHubError::NotFound)?;
    Ok(IssueOverview {
        number: node.number,
        title: node.title,
        body: node.body,
        url: node.url,
        repository: repository.name_with_owner,
        author: node.author.map_or_else(
            || Author { login: String::new(), bot: false },
            |actor| Author { bot: actor.typename == "Bot", login: actor.login },
        ),
        state: state(&node.state),
        updated_at: rfc3339_seconds(&node.updated_at).unwrap_or_default(),
        labels: node
            .labels
            .nodes
            .into_iter()
            .map(|label| Label { name: label.name, color: label.color })
            .collect(),
        assignees: node.assignees.nodes.into_iter().map(|login| login.login).collect(),
        milestone: node.milestone.map(|milestone| milestone.title),
        linked: node
            .closed_by_pull_requests_references
            .nodes
            .into_iter()
            .map(|linked| LinkedPullRequest {
                number: linked.number,
                title: linked.title,
                state: state(&linked.state),
                draft: linked.is_draft,
                author: linked.author.map(|author| author.login).unwrap_or_default(),
                closes: true,
            })
            .collect(),
        // The description, then every comment: what Conversation lists
        conversation_count: 1 + node.comments.total_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> IssueOverview {
        let body = match name {
            "linked" => include_str!("../fixtures/issue_overview_linked.json"),
            "closed" => include_str!("../fixtures/issue_overview_closed_pull_request.json"),
            "assigned" => include_str!("../fixtures/issue_overview_assigned.json"),
            "milestone" => include_str!("../fixtures/issue_overview_milestone.json"),
            other => panic!("no fixture {other}"),
        };
        parse_issue_overview(body).expect("the fixture parses")
    }

    /// An open pull request that would close it is named with who opened it: somebody is on it.
    #[test]
    fn an_issue_names_the_pull_request_that_would_close_it() {
        let issue = fixture("linked");
        assert_eq!((issue.number, issue.repository.as_str()), (14_015, "cli/cli"));
        assert_eq!(issue.linked.len(), 1);
        let linked = &issue.linked[0];
        assert_eq!((linked.number, linked.state), (14_017, PullRequestState::Open));
        assert!(linked.closes && !linked.author.is_empty());
        assert!(issue.assignees.is_empty() && issue.milestone.is_none());
        assert_eq!(issue.labels.len(), 4);
    }

    /// A closed attempt is history, kept: the next one should know it was tried.
    #[test]
    fn a_closed_attempt_is_kept() {
        let issue = fixture("closed");
        assert!(issue
            .linked
            .iter()
            .any(|linked| linked.number == 13_480 && linked.state == PullRequestState::Closed));
    }

    #[test]
    fn assignees_and_the_milestone_are_read() {
        assert_eq!(fixture("assigned").assignees, ["babakks"]);
        let issue = fixture("milestone");
        assert_eq!(issue.milestone.as_deref(), Some("1.101.0"));
        assert_eq!(issue.assignees, ["clarfonthey"]);
        assert!(issue.conversation_count >= 1);
    }

    #[test]
    fn a_missing_issue_is_not_found() {
        let body = r#"{"data":{"repository":{"nameWithOwner":"a/b","issue":null}}}"#;
        assert!(matches!(parse_issue_overview(body), Err(GitHubError::NotFound)));
    }
}
