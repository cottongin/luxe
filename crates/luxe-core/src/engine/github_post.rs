//! Engine GitHub post — a reply or a comment you wrote, sent from `/github` or `/pr`.
//!
//! The reader read it in full and pressed Enter on it, so that keypress is the approval, the way
//! posting a review is; nothing the agent calls reaches this. A reply goes into its thread. Anything
//! else is a new comment on the pull request or issue, under a quote of the line it answers, since
//! a conversation comment has no thread to sit in.

use super::answering::quoted_reply;
use super::posting::sentence;
use super::*;

/// A reply or comment to post.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubPost {
    /// `owner/name` when it is not the checkout's repository
    pub repository: Option<String>,
    /// The pull request or issue it is on
    pub number: u64,
    /// The review thread it replies in, when it answers one
    pub thread: Option<String>,
    /// What it answers, outside a thread: quoted above it
    pub answering: Option<String>,
    /// The words, the reader's
    pub body: String,
}

impl Engine {
    /// Posts `post` and answers where it landed, or why it did not.
    pub(crate) fn handle_post_github_comment(
        &self,
        post: GithubPost,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let github = self.config.github.clone();
        let root = self.config.guided_review_project_root.clone();
        let event_tx = event_tx.clone();
        tokio::spawn(async move {
            let (url, error) = match send(github, root, &post).await {
                Ok(url) => (Some(url), None),
                Err(reason) => (None, Some(reason)),
            };
            let event = Event::GithubCommentPosted {
                repository: post.repository,
                number: post.number,
                thread: post.thread,
                url,
                error,
            };
            let _ = event_tx.send(event).await;
        });
    }
}

async fn send(
    github: Option<Arc<dyn crate::github::GitHub>>,
    root: Option<std::path::PathBuf>,
    post: &GithubPost,
) -> Result<String, String> {
    if post.body.trim().is_empty() {
        return Err("There is nothing to post.".into());
    }
    let (github, repository) =
        super::github_overview::reach(github, root, post.repository.clone()).await?;
    match &post.thread {
        Some(thread) => github.reply_to_thread(&repository, thread, &post.body).await,
        None => {
            let body = quoted_reply(post.answering.as_deref(), &post.body);
            github.comment_on_pull_request(&repository, post.number, &body).await
        }
    }
    .map_err(|error| sentence(&error))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing written is nothing posted, whatever else is missing.
    #[tokio::test]
    async fn an_empty_reply_is_refused_before_anything_is_asked() {
        let post = GithubPost {
            repository: None,
            number: 1,
            thread: None,
            answering: None,
            body: "  \n".into(),
        };
        assert_eq!(send(None, None, &post).await.unwrap_err(), "There is nothing to post.");
    }
}
