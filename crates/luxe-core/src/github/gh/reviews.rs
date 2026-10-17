//! Reviews — the lines a review may comment on, posting one, and reading those already there.

use super::*;

#[async_trait]
impl Reviews for GhCli {
    async fn pull_request_patches(
        &self,
        repository: &Repository,
        number: u64,
    ) -> Result<Vec<FilePatch>, GitHubError> {
        let endpoint = format!("{}/files?per_page=100", pulls_endpoint(repository, number));
        let mut args = vec!["api", "--paginate"];
        push_hostname(&mut args, repository);
        args.extend([
            endpoint.as_str(),
            "--jq",
            ".[] | {filename, previous_filename, status, patch}",
        ]);
        parse_patches(&self.gh(&std::env::temp_dir(), &args).await?)
    }

    async fn submit_review(
        &self,
        repository: &Repository,
        number: u64,
        review: &SubmitReview,
    ) -> Result<PostedReview, GitHubError> {
        let input = PrivateFile::write(&review_payload(review).to_string())
            .map_err(|source| GitHubError::Run { program: "gh", source })?;
        let endpoint = format!("{}/reviews", pulls_endpoint(repository, number));
        let path = input.path.to_string_lossy().into_owned();
        let mut args = vec!["api", "--method", "POST"];
        push_hostname(&mut args, repository);
        args.extend([endpoint.as_str(), "--input", path.as_str()]);
        let ran = self.run(&std::env::temp_dir(), &args).await?;
        if ran.success {
            parse_posted_review(&ran.stdout)
        } else {
            Err(rejection(&ran.stdout)
                .unwrap_or_else(|| classify_failure(&ran.stderr, &ran.stdout)))
        }
    }

    async fn reviews(
        &self,
        repository: &Repository,
        number: u64,
    ) -> Result<Vec<ExistingReview>, GitHubError> {
        let endpoint = format!("{}/reviews?per_page=100", pulls_endpoint(repository, number));
        let mut args = vec!["api", "--paginate"];
        push_hostname(&mut args, repository);
        args.extend([
            endpoint.as_str(),
            "--jq",
            ".[] | {id, user: .user.login, state, commit_id, body, submitted_at, html_url}",
        ]);
        parse_reviews(&self.gh(&std::env::temp_dir(), &args).await?)
    }
}

/// The JSON body GitHub's create-review endpoint takes.
pub(super) fn review_payload(review: &SubmitReview) -> serde_json::Value {
    let side = |side: ThreadSide| match side {
        ThreadSide::Left => "LEFT",
        ThreadSide::Right => "RIGHT",
    };
    let comments: Vec<serde_json::Value> = review
        .comments
        .iter()
        .map(|comment| {
            let mut value = serde_json::json!({
                "path": comment.path,
                "line": comment.line,
                "side": side(comment.side),
                "body": comment.body,
            });
            if let Some(start) = comment.start_line {
                value["start_line"] = start.into();
                value["start_side"] = side(comment.side).into();
            }
            value
        })
        .collect();
    serde_json::json!({
        "commit_id": review.commit_id,
        "event": match review.event {
            ReviewEvent::Comment => "COMMENT",
            ReviewEvent::Approve => "APPROVE",
            ReviewEvent::RequestChanges => "REQUEST_CHANGES",
        },
        "body": review.body,
        "comments": comments,
    })
}

/// One line of `gh api --paginate …/files --jq '.[] | {…}'`.
#[derive(Deserialize)]
pub(super) struct FileLine {
    filename: String,
    #[serde(default)]
    previous_filename: Option<String>,
    #[serde(default)]
    patch: Option<String>,
}

pub(super) fn parse_patches(body: &str) -> Result<Vec<FilePatch>, GitHubError> {
    Ok(parse_lines::<FileLine>(body)?
        .into_iter()
        .map(|file| FilePatch {
            commentable: file.patch.as_deref().map(commentable_lines).unwrap_or_default(),
            path: file.filename,
            previous_path: file.previous_filename,
        })
        .collect())
}

/// The lines of a unified patch a comment can sit on: every line inside a hunk, on its side.
pub(super) fn commentable_lines(patch: &str) -> CommentableLines {
    let mut lines = CommentableLines::default();
    let (mut old, mut new) = (0u32, 0u32);
    for line in patch.lines() {
        if let Some(header) = line.strip_prefix("@@ ") {
            let mut ranges = header.split_whitespace();
            let start = |range: Option<&str>, sign: char| {
                range
                    .and_then(|range| range.strip_prefix(sign))
                    .and_then(|range| range.split(',').next())
                    .and_then(|start| start.parse::<u32>().ok())
                    .unwrap_or(0)
            };
            old = start(ranges.next(), '-');
            new = start(ranges.next(), '+');
            continue;
        }
        match line.chars().next() {
            Some('+') => {
                lines.right.insert(new);
                new += 1;
            }
            Some('-') => {
                lines.left.insert(old);
                old += 1;
            }
            Some('\\') => {}
            _ => {
                lines.left.insert(old);
                lines.right.insert(new);
                old += 1;
                new += 1;
            }
        }
    }
    lines
}

#[derive(Deserialize)]
pub(super) struct PostedLine {
    id: u64,
    html_url: String,
}

pub(super) fn parse_posted_review(body: &str) -> Result<PostedReview, GitHubError> {
    let posted: PostedLine = serde_json::from_str(body).map_err(GitHubError::Unreadable)?;
    Ok(PostedReview { id: posted.id, url: posted.html_url })
}

#[derive(Deserialize)]
pub(super) struct ReviewLine {
    id: u64,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    commit_id: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    submitted_at: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
}

pub(super) fn parse_reviews(body: &str) -> Result<Vec<ExistingReview>, GitHubError> {
    Ok(parse_lines::<ReviewLine>(body)?
        .into_iter()
        .map(|review| ExistingReview {
            id: review.id,
            author: review.user.unwrap_or_default(),
            commit_id: review.commit_id.unwrap_or_default(),
            body: review.body.unwrap_or_default(),
            submitted_at: review.submitted_at.unwrap_or_default(),
            url: review.html_url.unwrap_or_default(),
        })
        .collect())
}
