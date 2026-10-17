//! Threads — a pull request's review threads: reading, replying, commenting and resolving.

use super::*;

#[async_trait]
impl Threads for GhCli {
    async fn review_threads(
        &self,
        repository: &Repository,
        number: u64,
    ) -> Result<ReviewThreads, GitHubError> {
        let query = format!("query={REVIEW_THREADS_QUERY}");
        let (owner, name, number) = (
            format!("owner={}", repository.owner),
            format!("name={}", repository.name),
            format!("number={number}"),
        );
        let mut args = vec!["api", "graphql"];
        push_hostname(&mut args, repository);
        args.extend(["-f", &query, "-F", &owner, "-F", &name, "-F", &number]);
        parse_review_threads(&self.gh(&std::env::temp_dir(), &args).await?)
    }

    async fn reply_to_thread(
        &self,
        repository: &Repository,
        thread_id: &str,
        body: &str,
    ) -> Result<String, GitHubError> {
        let input = PrivateFile::write(body)
            .map_err(|source| GitHubError::Run { program: "gh", source })?;
        let query = format!("query={REPLY_MUTATION}");
        let thread = format!("thread={thread_id}");
        let body = format!("body=@{}", input.path.to_string_lossy());
        let mut args = vec!["api", "graphql"];
        push_hostname(&mut args, repository);
        // `-F body=@file` reads the value from the file, which is the point of writing one
        args.extend(["-f", &query, "-f", &thread, "-F", &body]);
        parse_reply(&self.gh(&std::env::temp_dir(), &args).await?)
    }

    async fn comment_on_pull_request(
        &self,
        repository: &Repository,
        number: u64,
        body: &str,
    ) -> Result<String, GitHubError> {
        let payload = serde_json::json!({ "body": body });
        let input = PrivateFile::write(&payload.to_string())
            .map_err(|source| GitHubError::Run { program: "gh", source })?;
        let endpoint =
            format!("repos/{}/{}/issues/{number}/comments", repository.owner, repository.name);
        let path = input.path.to_string_lossy().into_owned();
        let mut args = vec!["api", "--method", "POST"];
        push_hostname(&mut args, repository);
        args.extend([endpoint.as_str(), "--input", path.as_str(), "--jq", "{html_url}"]);
        parse_commented(&self.gh(&std::env::temp_dir(), &args).await?)
    }

    async fn resolve_thread(
        &self,
        repository: &Repository,
        thread_id: &str,
    ) -> Result<(), GitHubError> {
        let query = format!("query={RESOLVE_MUTATION}");
        let thread = format!("thread={thread_id}");
        let mut args = vec!["api", "graphql"];
        push_hostname(&mut args, repository);
        args.extend(["-f", &query, "-f", &thread]);
        parse_resolved(&self.gh(&std::env::temp_dir(), &args).await?)
    }
}

/// The review threads on a pull request, oldest comment first, and the latest of what was said
/// about it as a whole. One point of the GraphQL budget.
pub(super) const REVIEW_THREADS_QUERY: &str = "query($owner:String!,$name:String!,$number:Int!){\
     repository(owner:$owner,name:$name){pullRequest(number:$number){\
     reviewThreads(first:100){totalCount nodes{id isResolved isOutdated path line originalLine \
     startLine diffSide startDiffSide subjectType viewerCanReply viewerCanResolve \
     comments(first:20){totalCount nodes{author{login} body createdAt url diffHunk}}}} \
     comments(last:20){nodes{id author{__typename login} body createdAt url isMinimized \
     viewerDidAuthor}} \
     reviews(last:20){nodes{id author{__typename login} body createdAt url viewerDidAuthor}}}} \
     rateLimit{cost remaining resetAt}}";

/// Replies to a thread; the body is read from a private file, never the command line.
pub(super) const REPLY_MUTATION: &str = "mutation($thread:ID!,$body:String!){\
     addPullRequestReviewThreadReply(input:{pullRequestReviewThreadId:$thread,body:$body}){\
     comment{url}}}";

/// Resolves a thread.
pub(super) const RESOLVE_MUTATION: &str =
    "mutation($thread:ID!){resolveReviewThread(input:{threadId:$thread}){\
     thread{isResolved}}}";

/// The body of the [`REVIEW_THREADS_QUERY`] answer, down to the threads.
#[derive(Deserialize)]
pub(super) struct ThreadsAnswer {
    data: ThreadsData,
}

#[derive(Deserialize)]
pub(super) struct ThreadsData {
    repository: Option<ThreadsRepository>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ThreadsRepository {
    pull_request: Option<ThreadsPullRequest>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ThreadsPullRequest {
    review_threads: Connection<ThreadNode>,
    // Absent from an answer to the query before it asked for them
    #[serde(default)]
    comments: Option<Nodes<ConversationNode>>,
    #[serde(default)]
    reviews: Option<Nodes<ConversationNode>>,
}

/// A conversation comment or a review, which carry what is said about the pull request as a whole
/// in the same fields.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConversationNode {
    pub(super) id: String,
    pub(super) author: Option<ConversationAuthor>,
    #[serde(default)]
    pub(super) body: String,
    #[serde(default)]
    pub(super) created_at: String,
    #[serde(default)]
    pub(super) url: String,
    // Reviews are not minimizable, so theirs never comes
    #[serde(default)]
    pub(super) is_minimized: bool,
    #[serde(default)]
    pub(super) viewer_did_author: bool,
}

#[derive(Deserialize)]
pub(super) struct ConversationAuthor {
    #[serde(rename = "__typename", default)]
    typename: String,
    login: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ThreadNode {
    #[serde(default)]
    id: String,
    #[serde(default)]
    viewer_can_reply: bool,
    #[serde(default)]
    viewer_can_resolve: bool,
    is_resolved: bool,
    is_outdated: bool,
    path: String,
    line: Option<u32>,
    start_line: Option<u32>,
    diff_side: Option<String>,
    subject_type: Option<String>,
    comments: Connection<CommentNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CommentNode {
    author: Option<CommentAuthor>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    diff_hunk: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct CommentAuthor {
    login: String,
}

pub(super) fn parse_review_threads(body: &str) -> Result<ReviewThreads, GitHubError> {
    let answer: ThreadsAnswer = serde_json::from_str(body).map_err(GitHubError::Unreadable)?;
    let pull_request = answer
        .data
        .repository
        .and_then(|repository| repository.pull_request)
        .ok_or(GitHubError::NotFound)?;
    let conversation = conversation(
        pull_request.comments.map(|nodes| nodes.nodes).unwrap_or_default(),
        pull_request.reviews.map(|nodes| nodes.nodes).unwrap_or_default(),
    );
    let connection = pull_request.review_threads;
    let total = connection.total_count.max(connection.nodes.len());
    let threads = connection.nodes.into_iter().map(review_thread).collect();
    Ok(ReviewThreads { threads, total, conversation })
}

/// A review thread as GitHub's answer describes it, its first comment's patch read for the line it
/// was written on.
pub(super) fn review_thread(node: ThreadNode) -> ReviewThread {
    ReviewThread {
        id: node.id,
        can_reply: node.viewer_can_reply,
        can_resolve: node.viewer_can_resolve,
        anchor_text: node
            .comments
            .nodes
            .first()
            .and_then(|comment| comment.diff_hunk.as_deref())
            .and_then(anchor_text),
        path: node.path,
        line: node.line,
        start_line: node.start_line.filter(|start| Some(*start) != node.line),
        side: match node.diff_side.as_deref() {
            Some("LEFT") => ThreadSide::Left,
            _ => ThreadSide::Right,
        },
        outdated: node.is_outdated,
        resolved: node.is_resolved,
        file_level: node.subject_type.as_deref() == Some("FILE"),
        total_comments: node.comments.total_count.max(node.comments.nodes.len()),
        comments: node
            .comments
            .nodes
            .into_iter()
            .map(|comment| ThreadComment {
                // A deleted account comes back as a null author
                author: comment.author.map(|author| author.login).unwrap_or_default(),
                body: comment.body,
                created_at: comment.created_at,
                url: comment.url,
            })
            .collect(),
    }
}

/// The conversation comments and review summaries as one list, oldest first. A minimized comment
/// was hidden by someone with the standing to, and a review submitted with nothing but line
/// comments has no summary: neither says anything here.
pub(super) fn conversation(
    comments: Vec<ConversationNode>,
    reviews: Vec<ConversationNode>,
) -> Vec<ConversationComment> {
    let mut said: Vec<ConversationComment> = comments
        .into_iter()
        .chain(reviews)
        .filter(|node| !node.is_minimized && !node.body.trim().is_empty())
        .map(|node| {
            let (author, bot) = node
                .author
                .map_or((String::new(), false), |author| (author.login, author.typename == "Bot"));
            ConversationComment {
                id: node.id,
                author,
                bot,
                mine: node.viewer_did_author,
                body: node.body,
                created_at: node.created_at,
                url: node.url,
            }
        })
        .collect();
    // RFC 3339 in one zone sorts as text
    said.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    said
}

/// The line a thread was written on: the last line of its first comment's patch, without the
/// patch's marker. `None` for a hunk header alone, which is what a file-level thread carries.
pub(super) fn anchor_text(diff_hunk: &str) -> Option<String> {
    let last = diff_hunk.lines().rev().find(|line| !line.is_empty())?;
    if last.starts_with("@@") {
        return None;
    }
    let text = last.strip_prefix(['+', '-', ' ']).unwrap_or(last);
    Some(text.to_string())
}

#[derive(Deserialize)]
pub(super) struct ReplyAnswer {
    data: ReplyData,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReplyData {
    add_pull_request_review_thread_reply: Option<ReplyPayload>,
}

#[derive(Deserialize)]
pub(super) struct ReplyPayload {
    comment: ReplyComment,
}

#[derive(Deserialize)]
pub(super) struct ReplyComment {
    url: String,
}

pub(super) fn parse_reply(body: &str) -> Result<String, GitHubError> {
    let answer: ReplyAnswer = serde_json::from_str(body).map_err(GitHubError::Unreadable)?;
    answer
        .data
        .add_pull_request_review_thread_reply
        .map(|payload| payload.comment.url)
        .ok_or(GitHubError::NotFound)
}

/// Where a new conversation comment can be read.
pub(super) fn parse_commented(body: &str) -> Result<String, GitHubError> {
    #[derive(Deserialize)]
    struct Commented {
        html_url: String,
    }
    let commented: Commented = serde_json::from_str(body).map_err(GitHubError::Unreadable)?;
    Ok(commented.html_url)
}

#[derive(Deserialize)]
pub(super) struct ResolveAnswer {
    data: ResolveData,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ResolveData {
    resolve_review_thread: Option<ResolvePayload>,
}

#[derive(Deserialize)]
pub(super) struct ResolvePayload {
    thread: ResolvedThread,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ResolvedThread {
    is_resolved: bool,
}

/// A resolve that came back unresolved is refused, not quietly counted as done.
pub(super) fn parse_resolved(body: &str) -> Result<(), GitHubError> {
    let answer: ResolveAnswer = serde_json::from_str(body).map_err(GitHubError::Unreadable)?;
    match answer.data.resolve_review_thread {
        Some(payload) if payload.thread.is_resolved => Ok(()),
        Some(_) => Err(GitHubError::Rejected { reasons: vec!["the thread is still open".into()] }),
        None => Err(GitHubError::NotFound),
    }
}
