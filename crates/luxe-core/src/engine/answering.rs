//! Engine answering — replying to and resolving your own pull request's review threads.
//!
//! Two steps, like posting a review. Preparing reads whether the fix is on GitHub yet — the
//! checkout's `HEAD` is the pull request's head, and nothing is left uncommitted — because "fixed"
//! and "fixed on my machine" are different answers to a reviewer. Answering then sends exactly
//! what the reader confirmed, one thread at a time, and reports each: a failure on one thread
//! leaves the others answered and says which.

use super::posting::sentence;
use super::*;
use crate::github::{GitHub, GitHubError, Repository, Threads};
use crate::guided_review::{
    ConversationEntry, ThreadAnswer, ThreadAnswerResult, ThreadAnswersId, ThreadReply,
};

/// How long a local git read may take.
const GIT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

impl Engine {
    /// Reads whether the fixes behind the answers are on GitHub yet.
    pub(crate) async fn handle_prepare_thread_answers(
        &self,
        request_id: ThreadAnswersId,
        review_id: crate::guided_review::ReviewId,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let review = match self.open_review(&review_id, None).await {
            Ok((review, _)) => review,
            Err(message) => {
                let _ = event_tx.send(Event::ThreadAnswersFailed { request_id, message }).await;
                return;
            }
        };
        let github = self.config.github.clone();
        let project_root = self.config.guided_review_project_root.clone();
        let event_tx = event_tx.clone();
        tokio::spawn(async move {
            let event = match prepare(github, project_root, &review).await {
                Ok((pushed, uncommitted)) => {
                    Event::ThreadAnswersPrepared { request_id, pushed, uncommitted }
                }
                Err(message) => Event::ThreadAnswersFailed { request_id, message },
            };
            let _ = event_tx.send(event).await;
        });
    }

    /// Sends the answers the reader confirmed.
    ///
    /// Which GitHub thread each comment is, and whether it was said in the conversation, is read
    /// from the session the comment lives in. A reply names a comment, not a thread, so a front end
    /// cannot point an answer at a thread the review never had.
    pub(crate) async fn handle_answer_threads(
        &self,
        request_id: ThreadAnswersId,
        review_id: crate::guided_review::ReviewId,
        replies: Vec<ThreadReply>,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let (review, session) = match self.open_review(&review_id, None).await {
            Ok(open) => open,
            Err(message) => {
                let _ = event_tx.send(Event::ThreadAnswersFailed { request_id, message }).await;
                return;
            }
        };
        let (answers, unanswerable) = threads_of(&session, replies);
        let github = self.config.github.clone();
        let project_root = self.config.guided_review_project_root.clone();
        let event_tx = event_tx.clone();
        let desk = self.review_desk();
        tokio::spawn(async move {
            let event = match target(github, project_root, &review).await {
                Ok((github, repository, number)) => {
                    let mut results =
                        answer(github.as_ref(), &repository, number, &review, answers).await;
                    results.extend(unanswerable);
                    // What landed is recorded on the session, so it is not offered again
                    desk.send(super::review_desk::DeskCommand::ThreadsAnswered(results.clone()));
                    Event::ThreadsAnswered { request_id, results }
                }
                Err(message) => Event::ThreadAnswersFailed { request_id, message },
            };
            let _ = event_tx.send(event).await;
        });
    }
}

/// Each reply as the thread it answers, and a result for each one that names no thread.
fn threads_of(
    session: &crate::guided_review::ReviewSession,
    replies: Vec<ThreadReply>,
) -> (Vec<ThreadAnswer>, Vec<ThreadAnswerResult>) {
    let mut answers = Vec::new();
    let mut unanswerable = Vec::new();
    for ThreadReply { finding_id, reply, resolve } in replies {
        let thread = session
            .findings
            .iter()
            .find(|finding| finding.id == finding_id)
            .and_then(|finding| finding.github_thread.as_ref());
        match thread {
            Some(thread) => answers.push(ThreadAnswer {
                finding_id,
                thread_id: thread.id.clone(),
                reply,
                resolve,
                conversation: thread.conversation,
            }),
            None => unanswerable.push(ThreadAnswerResult {
                finding_id,
                reply_url: None,
                resolved: false,
                error: Some("That comment did not come from a thread on GitHub.".into()),
            }),
        }
    }
    (answers, unanswerable)
}

impl Engine {
    /// Reports what in the working tree is not committed, asked fresh from git rather than read
    /// from the last snapshot, which may predate a file made outside Luxe.
    pub(crate) fn check_worktree_removal(&self, event_tx: &mpsc::Sender<Event>) {
        let root = self.config.guided_review_project_root.clone();
        let event_tx = event_tx.clone();
        tokio::spawn(async move {
            let uncommitted = match root {
                Some(root) => match git(&root, &["status", "--porcelain"]).await {
                    Some(status) => status
                        .lines()
                        .map(|line| line.get(3..).unwrap_or(line).to_string())
                        .collect(),
                    None => vec!["(git could not say what is uncommitted)".into()],
                },
                None => vec!["(this run has no project)".into()],
            };
            let _ = event_tx.send(Event::WorktreeRemovalChecked { uncommitted }).await;
        });
    }
}

/// GitHub, the pull request's repository, the checkout, and its number, or why the threads of
/// this review cannot be answered.
async fn target(
    github: Option<Arc<dyn GitHub>>,
    project_root: Option<std::path::PathBuf>,
    review: &GuidedReview,
) -> Result<(Arc<dyn GitHub>, Repository, u64), String> {
    let ReviewTarget::OwnPullRequest { number } = review.target else {
        return Err("Only your own pull request's threads are answered from here.".into());
    };
    let slug = review
        .context
        .pull_request
        .as_ref()
        .map(|pr| pr.repository.clone())
        .ok_or("The review does not say which repository its pull request is on.")?;
    let github =
        github.ok_or("Answering threads needs the GitHub CLI, and this run cannot reach it.")?;
    let root = project_root.ok_or("Answering threads needs a project, and this run has none.")?;
    let checkout = github.repository(&root).await.map_err(|error| sentence(&error))?;
    let (owner, name) = slug.split_once('/').ok_or("The review names no repository.")?;
    let repository = Repository {
        host: checkout.host,
        owner: owner.into(),
        name: name.into(),
        default_branch: String::new(),
        remote: None,
    };
    Ok((github, repository, number))
}

/// Whether the checkout's `HEAD` is the pull request's head on GitHub, and whether anything in the
/// working tree is left uncommitted.
async fn prepare(
    github: Option<Arc<dyn GitHub>>,
    project_root: Option<std::path::PathBuf>,
    review: &GuidedReview,
) -> Result<(bool, bool), String> {
    let root = project_root.clone();
    let (github, repository, number) = target(github, project_root, review).await?;
    let root = root.ok_or("Answering threads needs a project, and this run has none.")?;
    let pr = github.pull_request(&repository, number).await.map_err(|error| sentence(&error))?;
    let head = git(&root, &["rev-parse", "HEAD"]).await.unwrap_or_default();
    let status = git(&root, &["status", "--porcelain"]).await.unwrap_or_default();
    Ok((!head.is_empty() && head == pr.head_oid, !status.is_empty()))
}

/// Answers each thread in turn: the reply first, then the resolve.
///
/// A reply that fails leaves its thread unresolved, since resolving a thread whose answer did not
/// arrive would close it with no word said. A reply whose answer never came back may still have
/// landed, so it says to look before sending again, instead of inviting a second copy.
///
/// Something said in the conversation has no thread to reply in, so its reply is a new comment
/// that quotes what it answers, the way GitHub's own quote reply does, and there is nothing to
/// resolve.
async fn answer(
    github: &(impl Threads + ?Sized),
    repository: &Repository,
    number: u64,
    review: &GuidedReview,
    answers: Vec<ThreadAnswer>,
) -> Vec<ThreadAnswerResult> {
    let mut results = Vec::with_capacity(answers.len());
    for ThreadAnswer { finding_id, thread_id, reply, resolve, conversation } in answers {
        let mut result =
            ThreadAnswerResult { finding_id, reply_url: None, resolved: false, error: None };
        if let Some(reply) = reply.filter(|reply| !reply.trim().is_empty()) {
            let replied = if conversation {
                let said = review.conversation.as_ref().map_or(&[][..], |c| c.said());
                let body = quote_reply(said, &thread_id, &reply);
                github.comment_on_pull_request(repository, number, &body).await
            } else {
                github.reply_to_thread(repository, &thread_id, &reply).await
            };
            match replied {
                Ok(url) => result.reply_url = Some(url),
                Err(error) => {
                    result.error = Some(match error {
                        GitHubError::Timeout { .. } | GitHubError::Run { .. } => format!(
                            "{} The reply may have landed anyway; look at the thread before \
                             sending it again.",
                            sentence(&error)
                        ),
                        error => sentence(&error),
                    });
                    results.push(result);
                    continue;
                }
            }
        }
        if resolve && !conversation {
            match github.resolve_thread(repository, &thread_id).await {
                Ok(()) => result.resolved = true,
                Err(error) => result.error = Some(sentence(&error)),
            }
        }
        results.push(result);
    }
    results
}

/// Longest a quoted line runs, in characters, before it is cut.
const MAX_QUOTE_CHARS: usize = 120;

/// `reply` under a quote of the first line of the conversation comment it answers, or alone when
/// that comment is no longer in the review.
fn quote_reply(said: &[ConversationEntry], id: &str, reply: &str) -> String {
    let answered = said.iter().find(|entry| entry.id == id).map(|entry| entry.body.as_str());
    quoted_reply(answered, reply)
}

/// `reply` under a quote of the first line of `answered`, or alone when there is nothing to quote.
///
/// A conversation comment has no thread to sit in, so the quote is what says which comment it
/// answers — to a reader, and to the next review, which reads it back as the comment answered.
pub(crate) fn quoted_reply(answered: Option<&str>, reply: &str) -> String {
    let Some(first) =
        answered.and_then(|body| body.lines().map(str::trim).find(|line| !line.is_empty()))
    else {
        return reply.to_owned();
    };
    let quoted = match first.char_indices().nth(MAX_QUOTE_CHARS) {
        Some((cut, _)) => format!("{}…", first[..cut].trim_end()),
        None => first.to_owned(),
    };
    format!("> {quoted}\n\n{reply}")
}

/// One line of git's output in `root`, or `None` when it fails.
async fn git(root: &std::path::Path, args: &[&str]) -> Option<String> {
    let program = crate::program::resolve("git").unwrap_or_else(|| std::path::PathBuf::from("git"));
    let command = crate::program::captured_command(program, args, root);
    let output = crate::program::run_bounded(command, GIT_DEADLINE).await.ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reply quotes the first line it answers, cut when long, and stands alone when what it
    /// answers is no longer in the review.
    #[test]
    fn a_conversation_reply_quotes_the_first_line_it_answers() {
        let entry = |id: &str, body: &str| ConversationEntry {
            id: id.into(),
            author: "mara".into(),
            body: body.into(),
            url: String::new(),
        };
        let long = format!("{} tail", "word ".repeat(30));
        let said = [entry("IC_1", "\n  Add a test.  \nFor the 422."), entry("IC_2", &long)];
        assert_eq!(quote_reply(&said, "IC_1", "Added."), "> Add a test.\n\nAdded.");
        let cut = quote_reply(&said, "IC_2", "Done.");
        let quoted = cut.lines().next().unwrap();
        assert!(quoted.ends_with("word…") && !quoted.contains("tail"), "{cut}");
        assert_eq!(quoted.chars().count(), "> ".len() + 119 + 1, "{cut}");
        assert_eq!(quote_reply(&said, "IC_gone", "Sure."), "Sure.");
        assert_eq!(quote_reply(&[entry("IC_3", "  \n ")], "IC_3", "Sure."), "Sure.");
    }
}
