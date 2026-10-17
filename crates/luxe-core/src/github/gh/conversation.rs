//! Conversation query — everything said about a pull request or an issue, in one request.
//!
//! The description, the comments, each review that said something and its verdict, and every
//! review thread, oldest first. About one point. A minimized comment was hidden by someone with
//! the standing to, and a review submitted with nothing but replies in threads has nothing to say
//! of its own; both are left out here, and the replies are in their threads.

use serde::Deserialize;

use super::threads::{review_thread, ThreadNode};
use super::{rfc3339_seconds, Connection};
use crate::github::overview::{ConversationEntry, Verdict};
use crate::github::{Author, ConversationComment, GitHubError};

pub(super) const PULL_REQUEST_CONVERSATION_QUERY: &str = "query($owner:String!,$name:String!,\
     $number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){\
     author{__typename login} body createdAt viewerDidAuthor \
     comments(last:50){nodes{id author{__typename login} body createdAt url isMinimized \
     viewerDidAuthor}} reviews(last:50){nodes{id author{__typename login} body state submittedAt \
     url viewerDidAuthor comments{totalCount}}} reviewThreads(first:100){totalCount nodes{id \
     isResolved isOutdated path line startLine diffSide subjectType viewerCanReply \
     viewerCanResolve comments(first:20){totalCount nodes{author{login} body createdAt url \
     diffHunk}}}}}} rateLimit{cost remaining}}";

pub(super) const ISSUE_CONVERSATION_QUERY: &str = "query($owner:String!,$name:String!,\
     $number:Int!){repository(owner:$owner,name:$name){issue(number:$number){\
     author{__typename login} body createdAt viewerDidAuthor \
     comments(last:50){nodes{id author{__typename login} body createdAt url isMinimized \
     viewerDidAuthor}}}} rateLimit{cost remaining}}";

#[derive(Deserialize)]
struct Answer {
    data: Data,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Data {
    repository: Option<RepositoryNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryNode {
    pull_request: Option<Subject>,
    issue: Option<Subject>,
}

/// A pull request or an issue: the same fields, threads and reviews only on a pull request.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Subject {
    author: Option<Actor>,
    #[serde(default)]
    body: String,
    created_at: String,
    comments: Nodes<CommentNode>,
    #[serde(default)]
    reviews: Option<Nodes<ReviewNode>>,
    #[serde(default)]
    review_threads: Option<Connection<ThreadNode>>,
}

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Deserialize)]
struct Actor {
    #[serde(rename = "__typename", default)]
    typename: String,
    login: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommentNode {
    id: String,
    author: Option<Actor>,
    #[serde(default)]
    body: String,
    created_at: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    is_minimized: bool,
    #[serde(default)]
    viewer_did_author: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewNode {
    id: String,
    author: Option<Actor>,
    #[serde(default)]
    body: String,
    state: String,
    submitted_at: Option<String>,
    #[serde(default)]
    url: String,
    #[serde(default)]
    viewer_did_author: bool,
    comments: Count,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Count {
    total_count: usize,
}

fn comment(
    id: String,
    author: Option<Actor>,
    body: String,
    created_at: String,
    url: String,
    mine: bool,
) -> ConversationComment {
    let (author, bot) =
        author.map_or((String::new(), false), |actor| (actor.login, actor.typename == "Bot"));
    ConversationComment { id, author, bot, mine, body, created_at, url }
}

/// Everything said, oldest first, from the answer to either conversation query.
pub(super) fn parse_conversation(body: &str) -> Result<Vec<ConversationEntry>, GitHubError> {
    let answer: Answer = serde_json::from_str(body).map_err(GitHubError::Unreadable)?;
    let repository = answer.data.repository.ok_or(GitHubError::NotFound)?;
    let subject = repository.pull_request.or(repository.issue).ok_or(GitHubError::NotFound)?;

    // Each entry with when it was said, to sort on: RFC 3339 in one zone sorts as text
    let mut said: Vec<(String, ConversationEntry)> = Vec::new();
    let author = subject.author.map_or_else(
        || Author { login: String::new(), bot: false },
        |actor| Author { bot: actor.typename == "Bot", login: actor.login },
    );
    said.push((
        subject.created_at.clone(),
        ConversationEntry::Opening {
            author,
            body: subject.body,
            at: rfc3339_seconds(&subject.created_at).unwrap_or_default(),
        },
    ));
    for node in subject.comments.nodes.into_iter().filter(|node| !node.is_minimized) {
        let at = node.created_at.clone();
        let entry = comment(
            node.id,
            node.author,
            node.body,
            node.created_at,
            node.url,
            node.viewer_did_author,
        );
        said.push((at, ConversationEntry::Comment(entry)));
    }
    for node in subject.reviews.map(|reviews| reviews.nodes).unwrap_or_default() {
        let verdict = match node.state.as_str() {
            "APPROVED" => Verdict::Approved,
            "CHANGES_REQUESTED" => Verdict::ChangesRequested,
            "DISMISSED" => Verdict::Dismissed,
            "COMMENTED" => Verdict::Commented,
            // A pending review is someone's unsent draft
            _ => continue,
        };
        // A verdict is news even without words; a bare comment-review's words are in its threads
        if node.body.trim().is_empty() && verdict == Verdict::Commented {
            continue;
        }
        let at = node.submitted_at.unwrap_or_default();
        let entry =
            comment(node.id, node.author, node.body, at.clone(), node.url, node.viewer_did_author);
        said.push((
            at,
            ConversationEntry::Review {
                comment: entry,
                verdict,
                line_comments: node.comments.total_count,
            },
        ));
    }
    let threads = subject.review_threads.map(|threads| threads.nodes).unwrap_or_default();
    for node in threads {
        let thread = review_thread(node);
        let at = thread.comments.first().map(|first| first.created_at.clone()).unwrap_or_default();
        said.push((at, ConversationEntry::Thread(thread)));
    }
    // Stable, so the description stays first when something shares its second
    said.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(said.into_iter().map(|(_, entry)| entry).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cli/cli#13982`: the description first, then everything in the order it was said — a bot's
    /// long review, a reviewer asking for changes, fourteen threads — and the bare comment-reviews
    /// that only carried thread replies left out.
    #[test]
    fn a_pull_request_conversation_reads_in_order() {
        let entries =
            parse_conversation(include_str!("../fixtures/conversation_pull_request.json"))
                .expect("the fixture parses");
        assert!(
            matches!(&entries[0], ConversationEntry::Opening { author, .. } if author.login == "offbyone")
        );
        let reviews: Vec<(&str, Verdict)> = entries
            .iter()
            .filter_map(|entry| match entry {
                ConversationEntry::Review { comment, verdict, .. } => {
                    Some((comment.author.as_str(), *verdict))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            reviews,
            [
                ("copilot-pull-request-reviewer", Verdict::Commented),
                ("BagToad", Verdict::ChangesRequested)
            ]
        );
        let bot = entries.iter().find_map(|entry| match entry {
            ConversationEntry::Review { comment, .. } if comment.bot => Some(comment),
            _ => None,
        });
        assert!(bot.is_some(), "the copilot reviewer is a bot");
        let threads = entries.iter().filter(|e| matches!(e, ConversationEntry::Thread(_))).count();
        assert_eq!(threads, 14);
        let stamps: Vec<u64> = entries
            .iter()
            .filter_map(|entry| match entry {
                ConversationEntry::Comment(c) | ConversationEntry::Review { comment: c, .. } => {
                    rfc3339_seconds(&c.created_at)
                }
                _ => None,
            })
            .collect();
        assert!(stamps.windows(2).all(|pair| pair[0] <= pair[1]), "oldest first");
    }

    /// An issue's conversation: its description, then its comments, a bot's among them.
    #[test]
    fn an_issue_conversation_is_its_comments() {
        let entries = parse_conversation(include_str!("../fixtures/conversation_issue.json"))
            .expect("parses");
        assert!(matches!(&entries[0], ConversationEntry::Opening { .. }));
        let comments: Vec<(&str, bool)> = entries
            .iter()
            .filter_map(|entry| match entry {
                ConversationEntry::Comment(comment) => Some((comment.author.as_str(), comment.bot)),
                _ => None,
            })
            .collect();
        assert_eq!(comments.len(), 5);
        assert!(comments.contains(&("github-actions", true)), "{comments:?}");
        assert!(!entries.iter().any(|entry| matches!(entry, ConversationEntry::Thread(_))));
    }

    #[test]
    fn a_missing_subject_is_not_found() {
        let body = r#"{"data":{"repository":{"pullRequest":null}}}"#;
        assert!(matches!(parse_conversation(body), Err(GitHubError::NotFound)));
    }
}
