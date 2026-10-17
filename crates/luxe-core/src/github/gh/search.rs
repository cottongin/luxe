//! Search — what `/github`'s sections list, each row with what its second line says.
//!
//! One GraphQL search per page, thirty rows, one point of the hourly budget: GitHub's own tally of
//! each pull request's checks rather than a list of them, and the first pull request that would
//! close each issue rather than every one that mentions it. The page says how many the search
//! matches in all and where the next page starts, so a section can say `30 of 214` and go on.

use serde::Deserialize;

use super::{count_checks, status_text, Nodes, RollupContexts};
use crate::github::overview::{Label, RowFacts};
use crate::github::{GitHubError, ListItem, PullRequestState, ReviewDecision, SearchPage};

/// The search, with what each row's second line needs.
pub(super) const SEARCH_QUERY: &str = "query($q:String!,$after:String){\
     search(query:$q,type:ISSUE,first:30,after:$after){issueCount pageInfo{endCursor hasNextPage} \
     nodes{__typename ...on PullRequest{number title url state isDraft updatedAt \
     repository{nameWithOwner} author{__typename login} additions deletions reviewDecision \
     comments{totalCount} commits(last:1){nodes{commit{statusCheckRollup{contexts(first:1){\
     checkRunCountsByState{state count} statusContextCountsByState{state count}}}}}}} \
     ...on Issue{number title url state updatedAt repository{nameWithOwner} \
     author{__typename login} labels(first:5){nodes{name color}} comments{totalCount} \
     closedByPullRequestsReferences(first:1,includeClosedPrs:false){nodes{number}}}}}}";

#[derive(Deserialize)]
struct Answer {
    data: Data,
}

#[derive(Deserialize)]
struct Data {
    search: Search,
}

/// The search's answer. A fixture from before the count and cursor were asked for has neither,
/// and reads as one page holding everything.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Search {
    nodes: Vec<Option<Node>>,
    issue_count: Option<u64>,
    page_info: Option<PageInfo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    end_cursor: Option<String>,
    has_next_page: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Node {
    #[serde(rename = "__typename")]
    typename: String,
    number: Option<u64>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    state: String,
    repository: Option<NameWithOwner>,
    author: Option<Actor>,
    additions: Option<u64>,
    deletions: Option<u64>,
    review_decision: Option<String>,
    comments: Option<Count>,
    commits: Option<Nodes<CommitNode>>,
    labels: Option<Nodes<LabelNode>>,
    closed_by_pull_requests_references: Option<Nodes<Numbered>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NameWithOwner {
    name_with_owner: String,
}

/// An author; an answer from before the typename was asked for has none, and reads as a person.
#[derive(Deserialize)]
struct Actor {
    #[serde(rename = "__typename", default)]
    typename: String,
    login: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Count {
    total_count: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitNode {
    commit: CommitRollup,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitRollup {
    status_check_rollup: Option<Rollup>,
}

#[derive(Deserialize)]
struct Rollup {
    contexts: RollupContexts,
}

#[derive(Deserialize)]
struct LabelNode {
    name: String,
    color: String,
}

#[derive(Deserialize)]
struct Numbered {
    number: u64,
}

/// The page a search found: its rows in the search's order, the count, and the next page's cursor.
pub(super) fn parse_search(body: &str) -> Result<SearchPage, GitHubError> {
    let answer: Answer = serde_json::from_str(body).map_err(GitHubError::Unreadable)?;
    let search = answer.data.search;
    let items: Vec<ListItem> = search
        .nodes
        .into_iter()
        // GitHub types each node nullable; anything but these two has no number to act on
        .flatten()
        .filter_map(|node| {
            let pull_request = match node.typename.as_str() {
                "PullRequest" => true,
                "Issue" => false,
                _ => return None,
            };
            let facts = facts(&node, pull_request);
            Some(ListItem {
                pull_request,
                number: node.number?,
                title: status_text(&node.title),
                url: node.url,
                repository: node.repository.map(|repo| repo.name_with_owner).unwrap_or_default(),
                author: node.author.map(|author| author.login).unwrap_or_default(),
                draft: node.is_draft,
                updated_at: node.updated_at,
                state: match node.state.as_str() {
                    "MERGED" => PullRequestState::Merged,
                    "CLOSED" => PullRequestState::Closed,
                    _ => PullRequestState::Open,
                },
                facts,
            })
        })
        .collect();
    let next = search
        .page_info
        .filter(|page| page.has_next_page)
        .and_then(|page| page.end_cursor)
        .filter(|cursor| !cursor.is_empty());
    // Never fewer than it brought, whatever the count says
    let total = search.issue_count.unwrap_or(0).max(items.len() as u64);
    Ok(SearchPage { items, total, next })
}

/// What a row's second line says, or nothing for an answer that carries none of it.
fn facts(node: &Node, pull_request: bool) -> Option<RowFacts> {
    let asked = node.comments.is_some();
    if !asked {
        return None;
    }
    let checks = node
        .commits
        .as_ref()
        .and_then(|commits| commits.nodes.first())
        .and_then(|commit| commit.commit.status_check_rollup.as_ref())
        .map(|rollup| count_checks(&rollup.contexts))
        .unwrap_or_default();
    Some(RowFacts {
        author_bot: node.author.as_ref().is_some_and(|author| author.typename == "Bot"),
        additions: node.additions.unwrap_or_default(),
        deletions: node.deletions.unwrap_or_default(),
        checks,
        review_decision: match node.review_decision.as_deref() {
            Some("APPROVED") => Some(ReviewDecision::Approved),
            Some("CHANGES_REQUESTED") => Some(ReviewDecision::ChangesRequested),
            Some("REVIEW_REQUIRED") => Some(ReviewDecision::ReviewRequired),
            _ => None,
        },
        comments: node.comments.as_ref().map_or(0, |count| count.total_count),
        labels: node
            .labels
            .as_ref()
            .map(|labels| {
                labels
                    .nodes
                    .iter()
                    .map(|label| Label {
                        name: status_text(&label.name),
                        color: label.color.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        linked_pull_request: match pull_request {
            true => None,
            false => node
                .closed_by_pull_requests_references
                .as_ref()
                .and_then(|linked| linked.nodes.first())
                .map(|linked| linked.number),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Thirty open pull requests on `cli/cli`: each row carries its size, its checks and its
    /// review verdict, and a bot is named as one.
    #[test]
    fn a_pull_request_row_says_how_big_and_how_it_stands() {
        let rows = parse_search(include_str!("../fixtures/search_pull_requests_facts.json"))
            .expect("the fixture parses")
            .items;
        assert_eq!(rows.len(), 30);
        let first = &rows[0];
        assert_eq!((first.number, first.draft), (14_578, true));
        let facts = first.facts.as_ref().expect("facts");
        assert_eq!((facts.additions, facts.deletions), (1_332, 867));
        assert_eq!(facts.review_decision, Some(ReviewDecision::ReviewRequired));
        assert!(!facts.author_bot);
        assert!(rows.iter().all(|row| row.pull_request && row.facts.is_some()));
        assert!(rows.iter().any(|row| row.facts.as_ref().unwrap().checks.total() > 0));
    }

    /// The first of three pages of `repo:cli/cli is:pr is:open`: GitHub's count of all 76, and
    /// the cursor the second page starts after.
    #[test]
    fn a_page_with_more_says_how_many_and_where_the_next_starts() {
        let page = parse_search(include_str!("../fixtures/search_page_first.json"))
            .expect("the fixture parses");
        assert_eq!(page.items.len(), 30);
        assert_eq!(page.total, 76);
        assert_eq!(page.next.as_deref(), Some("Y3Vyc29yOjMw"));
        assert_eq!(page.items[0].number, 14_620);
        // A real page's verdicts, every kind GitHub gives, each read as itself
        let verdicts = |wanted| {
            page.items
                .iter()
                .filter(|item| item.facts.as_ref().unwrap().review_decision == Some(wanted))
                .count()
        };
        assert_eq!(verdicts(ReviewDecision::Approved), 3);
        assert_eq!(verdicts(ReviewDecision::ChangesRequested), 2);
        assert_eq!(verdicts(ReviewDecision::ReviewRequired), 25);
    }

    /// The last of those pages carries a cursor too, and `hasNextPage: false` is what says it is
    /// the end: a section that followed the cursor would ask for an empty fourth page forever.
    #[test]
    fn the_last_page_has_no_next() {
        let page = parse_search(include_str!("../fixtures/search_page_last.json"))
            .expect("the fixture parses");
        assert_eq!((page.items.len(), page.total), (16, 76));
        assert_eq!(page.next, None);
    }

    /// An answer from before the count and cursor were asked for is one page holding everything,
    /// and a count below what arrived never says fewer than are on screen.
    #[test]
    fn an_answer_without_a_count_is_everything_it_brought() {
        let page = parse_search(include_str!("../fixtures/search_issues_facts.json"))
            .expect("the fixture parses");
        assert_eq!(page.total, page.items.len() as u64);
        assert_eq!(page.next, None);
        let low = r#"{"data":{"search":{"issueCount":0,"pageInfo":{"endCursor":"","hasNextPage":true},"nodes":[null,{"__typename":"Issue","number":7,"url":"u"}]}}}"#;
        let page = parse_search(low).expect("parses");
        assert_eq!((page.items.len(), page.total), (1, 1));
        assert_eq!(page.next, None, "an empty cursor leads nowhere");
    }

    /// Issues carry their labels and comment count, and the open pull request that closes one.
    #[test]
    fn an_issue_row_says_its_labels_and_what_closes_it() {
        let rows = parse_search(include_str!("../fixtures/search_issues_facts.json"))
            .expect("the fixture parses")
            .items;
        let first = rows.first().expect("rows");
        assert_eq!(first.number, 13_434);
        let facts = first.facts.as_ref().expect("facts");
        let labels: Vec<&str> = facts.labels.iter().map(|label| label.name.as_str()).collect();
        assert_eq!(labels, ["bug", "priority-3", "gh-run", "pitch"]);
        assert_eq!(facts.comments, 6);
        assert!(rows.iter().all(|row| !row.pull_request));
        assert!(
            rows.iter().any(|row| row.facts.as_ref().unwrap().linked_pull_request.is_some()),
            "an issue with an open pull request that closes it says which"
        );
    }
}
