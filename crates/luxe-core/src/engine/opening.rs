//! Engine opening — a branch's pull request, drafted from its review and opened through GitHub.
//!
//! Two steps, like posting. Preparing drafts the pull request from the review
//! ([`crate::guided_review::pull_request_draft`]) and reads what the preview has to show: whether
//! the branch is on GitHub yet, and the commits the reviewed work would be written as when it is
//! not committed, since a pull request carries only commits. Opening writes those commits — the
//! same ones "Accept and commit" would — pushes the branch when it has to, then opens exactly the
//! pull request the reader confirmed.

use super::posting::sentence;
use super::*;
use crate::github::{
    BranchHead, GitHub, GitHubError, NewPullRequest, Publication, PullRequestState,
};
use crate::guided_review::{ChapterCommit, LocalGit, PullRequestOpenId, ReviewBase};
use crate::program::{captured_command, run_bounded};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a push may take before it is given up on.
pub(super) const PUSH_DEADLINE: Duration = Duration::from_secs(120);

/// How long a local git read may take.
pub(super) const GIT_DEADLINE: Duration = Duration::from_secs(10);

impl Engine {
    /// Drafts the pull request the open review would open, for the reader to edit, and keeps the
    /// draft for the confirm.
    pub(crate) async fn handle_prepare_pull_request(
        &self,
        request_id: PullRequestOpenId,
        review_id: ReviewId,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let review = match self.open_review(&review_id, None).await {
            Ok((review, _)) => review,
            Err(message) => {
                let _ = event_tx.send(Event::PullRequestOpenFailed { request_id, message }).await;
                return;
            }
        };
        let github = self.config.github.clone();
        let root = self.config.guided_review_project_root.clone();
        let git = root.clone().map(|root| self.review_git(root));
        let issue = self.session_issue().await;
        let previews = Arc::clone(&self.previews);
        super::previews::lock(&previews).opening.begin(request_id.clone());
        let event_tx = event_tx.clone();
        tokio::spawn(async move {
            let event = match prepare(github, root, git, &review, issue).await {
                Ok(Prepared { pull_request, publication, uncommitted, commits }) => {
                    super::previews::lock(&previews).opening.keep(
                        &request_id,
                        super::previews::OpeningPreview {
                            review_id: review.id.clone(),
                            revision_id: review.revision_id.clone(),
                            pull_request: pull_request.clone(),
                            commits: !commits.is_empty(),
                        },
                    );
                    Event::PullRequestPrepared {
                        request_id,
                        pull_request,
                        publication,
                        uncommitted,
                        commits,
                    }
                }
                Err(message) => Event::PullRequestOpenFailed { request_id, message },
            };
            let _ = event_tx.send(event).await;
        });
    }

    /// Opens the pull request `request_id` drafted, with the reader's title, description and draft
    /// choice: commits the reviewed work when the preview said it would, and pushes the branch if
    /// it has to.
    ///
    /// In a task of its own, since a push can take a while, and the session is linked to what
    /// opened through the engine's own queue. The draft stays until it opens, so a failed attempt
    /// can be confirmed again.
    pub(crate) async fn handle_open_pull_request(
        &self,
        request_id: PullRequestOpenId,
        title: String,
        body: String,
        draft: bool,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let preview = super::previews::lock(&self.previews).opening.get(&request_id).cloned();
        let Some(preview) = preview else {
            let message = "That draft is not the current one, so nothing was opened. Prepare the \
                           pull request again."
                .to_string();
            let _ = event_tx.send(Event::PullRequestOpenFailed { request_id, message }).await;
            return;
        };
        if title.trim().is_empty() {
            let message = "A pull request needs a title.".to_string();
            let _ = event_tx.send(Event::PullRequestOpenFailed { request_id, message }).await;
            return;
        }
        // The work is committed from the review as the preview showed it, or not at all
        let commit = if preview.commits {
            match self.open_review(&preview.review_id, Some(&preview.revision_id)).await {
                Ok((review, _)) => Some(review),
                Err(message) => {
                    let _ =
                        event_tx.send(Event::PullRequestOpenFailed { request_id, message }).await;
                    return;
                }
            }
        } else {
            None
        };
        let pull_request = NewPullRequest { title, body, draft, ..preview.pull_request };
        let github = self.config.github.clone();
        let root = self.config.guided_review_project_root.clone();
        let git = root.clone().map(|root| self.review_git(root));
        let actions = self.config.action_tx.clone();
        let previews = Arc::clone(&self.previews);
        let event_tx = event_tx.clone();
        tokio::spawn(async move {
            let event = match open(github, root, git, commit.as_ref(), &pull_request).await {
                Ok((number, url, committed)) => {
                    super::previews::lock(&previews).opening.take(&request_id);
                    if let Some(actions) = actions {
                        let link = Action::LinkPullRequest { number, url: url.clone() };
                        let _ = actions.send(link).await;
                    }
                    Event::PullRequestOpened { request_id, number, url, committed }
                }
                Err(message) => Event::PullRequestOpenFailed { request_id, message },
            };
            let _ = event_tx.send(event).await;
        });
    }

    /// Answers whether pull request `number` can be checked out here to fix, before the front-end
    /// leaves the session it is in to do it.
    pub(crate) fn handle_check_pull_request_fix(
        &self,
        number: u64,
        repository: Option<String>,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let github = self.config.github.clone();
        // The worktrees live under the project, which is the main checkout's even from a worktree
        let root = self.config.guided_review_project_root.clone();
        let event_tx = event_tx.clone();
        tokio::spawn(async move {
            let answer = fix_check(github, root, number, repository.as_deref()).await;
            let (branch, refusal) = match answer {
                Ok(branch) => (Some(branch), None),
                Err(refusal) => (None, Some(refusal)),
            };
            let _ = event_tx.send(Event::PullRequestFixChecked { number, branch, refusal }).await;
        });
    }

    pub(super) async fn session_issue(&self) -> Option<u64> {
        let session = self.current_session_id().await;
        let store = self.config.session_store.read().ok()?;
        crate::session::session_issue(&store.load(&session).ok()?)
    }
}

/// What the preview shows before anything is written.
struct Prepared {
    pull_request: NewPullRequest,
    publication: Publication,
    uncommitted: usize,
    commits: Vec<String>,
}

async fn prepare(
    github: Option<Arc<dyn GitHub>>,
    root: Option<PathBuf>,
    review_git: Option<LocalGit>,
    review: &GuidedReview,
    issue: Option<u64>,
) -> Result<Prepared, String> {
    let github = github
        .ok_or("Opening a pull request needs the GitHub CLI, and this run cannot reach it.")?;
    let root = root.ok_or("Opening a pull request needs a project, and this run has none.")?;
    let repository = github.repository(&root).await.map_err(|error| sentence(&error))?;
    let head = crate::github::branch_head(&root)
        .await
        .ok_or("Opening a pull request needs a branch checked out, and HEAD is detached.")?;
    let reviewed_against = match &review.source.base {
        ReviewBase::MergeBase { reference, .. } => reference.as_str(),
        _ => "",
    };
    let base =
        base_branch(reviewed_against, &repository.default_branch, repository.remote.as_deref());
    if head.name == base {
        return Err(format!(
            "You're on `{base}`, the branch it would merge into. Start a branch for the work first."
        ));
    }
    // Unpublished, it is looked for under the repository's owner: a pull request there by that
    // name would be the one a push lands in
    let status =
        github.branch_status(&repository, &head).await.map_err(|error| sentence(&error))?;
    if let Some(open) = status.pull_request.filter(|found| found.state == PullRequestState::Open) {
        return Err(format!("#{} is already open for this branch: {}", open.number, open.url));
    }
    let publication = publication(&root).await;
    let uncommitted = git(&root, &["status", "--porcelain"], GIT_DEADLINE)
        .await
        .map(|out| out.lines().count())
        .unwrap_or(0);
    let commits = match (uncommitted, review_git) {
        (0, _) | (_, None) => Vec::new(),
        (_, Some(git)) => {
            if let Some(refusal) = staged_refusal(&root).await {
                return Err(refusal);
            }
            let (commits, _) = commit_plan(&git, review).await?;
            commits.into_iter().map(|commit| commit.subject).collect()
        }
    };
    // The issue's current title, for the draft's; its number alone still closes it
    let closed =
        match issue {
            Some(number) => Some(crate::guided_review::ClosedIssue {
                number,
                title: github.issue(&repository, number).await.ok().map(|read| {
                    crate::github::untrusted(read.title.trim(), 200).replace('\n', " ")
                }),
            }),
            None => None,
        };
    let draft = crate::guided_review::pull_request_draft(review, closed.as_ref());
    Ok(Prepared {
        pull_request: NewPullRequest {
            head: head_ref(&head, &repository.owner),
            base,
            title: draft.title,
            body: draft.body,
            draft: false,
        },
        publication,
        uncommitted,
        commits,
    })
}

/// The commits the reviewed work is written as: one per chapter, or the whole review as one when
/// it starts before `HEAD` and a chapter's patch cannot be staged on its own. Answers whether it
/// is the whole review.
async fn commit_plan(
    git: &LocalGit,
    review: &GuidedReview,
) -> Result<(Vec<ChapterCommit>, bool), String> {
    let refused = |error: String| format!("The reviewed work cannot be committed: {error}.");
    let from_head = git.starts_at_head(review).await.map_err(|error| refused(error.to_string()))?;
    if from_head {
        let commits = crate::guided_review::chapter_commits(review, &[])
            .map_err(|error| refused(error.to_string()))?;
        Ok((commits, false))
    } else {
        let commit = crate::guided_review::squashed_commit(review, &[])
            .map_err(|error| refused(error.to_string()))?;
        Ok((vec![commit], true))
    }
}

/// The branch pull request `number` would be checked out on, or why it cannot be.
async fn fix_check(
    github: Option<Arc<dyn GitHub>>,
    root: Option<PathBuf>,
    number: u64,
    reviewed_on: Option<&str>,
) -> Result<String, String> {
    let github = github
        .ok_or("Fixing a pull request needs the GitHub CLI, and this run cannot reach it.")?;
    let root = root.ok_or("Fixing a pull request needs a project, and this run has none.")?;
    let repository = github.repository(&root).await.map_err(|error| sentence(&error))?;
    if let Some(other) = reviewed_on.filter(|other| !repository.is(other)) {
        return Err(format!(
            "Pull request #{number} is on {other}, and this checkout is {}: it can only be fixed \
             from a checkout of {other}.",
            repository.slug()
        ));
    }
    let pull_request =
        github.pull_request(&repository, number).await.map_err(|error| sentence(&error))?;
    if pull_request.state != PullRequestState::Open {
        return Err(format!(
            "Pull request #{number} is not open, so there is nothing to push a fix to."
        ));
    }
    let head =
        github.pull_request_head(&repository, number).await.map_err(|error| sentence(&error))?;
    if let Some(refusal) = head.refusal(number) {
        return Err(refusal);
    }
    let state_root = project_of(&root).await;
    let existing = crate::worktree::worktree_path(&state_root, &head.local_branch);
    if existing.exists() {
        return Err(format!(
            "Pull request #{number} is already checked out, in `{}`: `luxe --from-pr {number}` \
             resumes the session working on it.",
            crate::paths::display(existing.strip_prefix(&state_root).unwrap_or(&existing))
        ));
    }
    Ok(head.local_branch)
}

/// The project a checkout belongs to: itself, or for a session worktree the main checkout it was
/// made from, where every worktree lives.
async fn project_of(root: &Path) -> PathBuf {
    match git(root, &["rev-parse", "--path-format=absolute", "--git-common-dir"], GIT_DEADLINE)
        .await
    {
        Ok(common) => {
            let common = PathBuf::from(common);
            common.parent().map_or_else(|| root.to_path_buf(), Path::to_path_buf)
        }
        Err(_) => root.to_path_buf(),
    }
}

/// Opens once, and never twice: an answer that never arrived is looked for before it is retried.
///
/// Answers the pull request and the commits written first, when `commit` asked for them.
async fn open(
    github: Option<Arc<dyn GitHub>>,
    root: Option<PathBuf>,
    review_git: Option<LocalGit>,
    commit: Option<&GuidedReview>,
    pull_request: &NewPullRequest,
) -> Result<(u64, String, Vec<String>), String> {
    let github = github
        .ok_or("Opening a pull request needs the GitHub CLI, and this run cannot reach it.")?;
    let root = root.ok_or("Opening a pull request needs a project, and this run has none.")?;
    let repository = github.repository(&root).await.map_err(|error| sentence(&error))?;
    let local = git(&root, &["symbolic-ref", "--quiet", "--short", "HEAD"], GIT_DEADLINE)
        .await
        .map_err(|_| "Opening a pull request needs a branch checked out, and HEAD is detached.")?;
    let committed = match (commit, review_git) {
        (Some(review), Some(git)) => {
            let (commits, whole) = commit_plan(&git, review).await?;
            git.commit_review_chapters(review, &commits, whole).await.map_err(|error| {
                format!("Committing the reviewed work failed: {error}. Nothing was pushed.")
            })?
        }
        _ => Vec::new(),
    };
    if publication(&root).await != Publication::Pushed {
        push(&root, &local, repository.remote.as_deref()).await?;
    }
    match github.open_pull_request(&repository, pull_request).await {
        Ok(opened) => Ok((opened.number, opened.url, committed)),
        Err(error @ (GitHubError::Timeout { .. } | GitHubError::Run { .. })) => {
            let head = crate::github::branch_head(&root).await;
            let landed = match head {
                Some(head) => github.branch_status(&repository, &head).await.ok(),
                None => None,
            }
            .and_then(|status| status.pull_request)
            .filter(|found| found.state == PullRequestState::Open);
            match landed {
                Some(found) => Ok((found.number, found.url, committed)),
                None => Err(format!(
                    "{} It may still open: look on GitHub before trying again.",
                    sentence(&error)
                )),
            }
        }
        Err(error) => Err(sentence(&error)),
    }
}

/// Whether the branch checked out in `root` is on its remote, and with every commit.
async fn publication(root: &Path) -> Publication {
    if git(root, &["rev-parse", "--verify", "--quiet", "@{push}"], GIT_DEADLINE).await.is_err() {
        return Publication::Unpublished;
    }
    match git(root, &["rev-list", "--count", "@{push}..HEAD"], GIT_DEADLINE).await {
        Ok(count) => match count.trim().parse::<usize>() {
            Ok(0) => Publication::Pushed,
            Ok(commits) => Publication::Ahead { commits },
            Err(_) => Publication::Unpublished,
        },
        Err(_) => Publication::Unpublished,
    }
}

/// Pushes `local` where it goes, setting it up to track there the first time.
async fn push(root: &Path, local: &str, default_remote: Option<&str>) -> Result<(), String> {
    let configured =
        git(root, &["config", &format!("branch.{local}.remote")], GIT_DEADLINE).await.ok();
    let remote = configured.as_deref().or(default_remote).unwrap_or("origin").to_string();
    git(root, &["push", "--set-upstream", &remote, local], PUSH_DEADLINE)
        .await
        .map(drop)
        .map_err(|said| format!("Pushing `{local}` to {remote} failed: {said}"))
}

/// The branch a pull request from this review merges into: the branch it was reviewed against,
/// else the repository's default.
pub(super) fn base_branch(reference: &str, default: &str, remote: Option<&str>) -> String {
    let short = reference
        .strip_prefix("refs/heads/")
        .or_else(|| {
            reference
                .strip_prefix("refs/remotes/")
                .and_then(|rest| rest.split_once('/').map(|(_, b)| b))
        })
        .or_else(|| remote.and_then(|remote| reference.strip_prefix(remote)?.strip_prefix('/')))
        .or_else(|| reference.strip_prefix("origin/"))
        .unwrap_or(reference);
    let is_oid = short.len() >= 7 && short.chars().all(|c| c.is_ascii_hexdigit());
    if short.is_empty() || short == "HEAD" || is_oid {
        default.to_string()
    } else {
        short.to_string()
    }
}

/// Why the reviewed work cannot be committed yet, when something is staged already.
///
/// A chapter commit is staged hunk by hunk from the reviewed revision, and anything already in the
/// index would ride along into it unreviewed, so committing refuses. Said here, in the preview,
/// rather than after Enter, where it read as opening having failed.
pub(super) async fn staged_refusal(root: &Path) -> Option<String> {
    let staged = git(root, &["diff", "--cached", "--name-only"], GIT_DEADLINE).await.ok()?;
    (!staged.trim().is_empty()).then(|| {
        "Some changes are staged, and committing the review would carry them along unreviewed: \
         commit them or unstage them (`git restore --staged .`) first."
            .to_string()
    })
}

/// How GitHub names the head branch: bare in its own repository, `owner:branch` from a fork.
pub(super) fn head_ref(head: &BranchHead, owner: &str) -> String {
    match &head.owner {
        Some(fork) if !fork.eq_ignore_ascii_case(owner) => format!("{fork}:{}", head.name),
        _ => head.name.clone(),
    }
}

pub(super) async fn git(dir: &Path, args: &[&str], deadline: Duration) -> Result<String, String> {
    let program = crate::program::resolve("git").unwrap_or_else(|| PathBuf::from("git"));
    let mut command = captured_command(program, args, dir);
    command.env("GIT_TERMINAL_PROMPT", "0").env("GCM_INTERACTIVE", "never");
    let output = run_bounded(command, deadline).await.map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let said = String::from_utf8_lossy(&output.stderr);
        Err(said
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(3)
            .collect::<Vec<_>>()
            .join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(name: &str, owner: Option<&str>) -> BranchHead {
        BranchHead { name: name.into(), owner: owner.map(str::to_string) }
    }

    #[test]
    fn the_base_is_the_branch_reviewed_against_else_the_default() {
        for (reference, expected) in [
            ("origin/release", "release"),
            ("upstream/release", "release"),
            ("refs/remotes/origin/stack-1", "stack-1"),
            ("refs/heads/stack-1", "stack-1"),
            ("stack-1", "stack-1"),
            ("HEAD", "main"),
            ("", "main"),
            ("0123abcd", "main"),
        ] {
            assert_eq!(base_branch(reference, "main", Some("upstream")), expected, "{reference}");
        }
        assert_eq!(base_branch("origin/dev", "main", None), "dev");
    }

    /// A failure says what git said, not an empty sentence.
    #[tokio::test]
    async fn a_failed_git_says_what_git_said() {
        let dir = tempfile::tempdir().unwrap();
        let said = git(dir.path(), &["rev-parse", "--verify", "HEAD"], GIT_DEADLINE).await;
        assert!(said.unwrap_err().starts_with("fatal:"));
    }

    #[test]
    fn a_fork_names_its_owner_in_the_head() {
        assert_eq!(head_ref(&head("fix", Some("me")), "acme"), "me:fix");
        assert_eq!(head_ref(&head("fix", Some("Acme")), "acme"), "fix");
        assert_eq!(head_ref(&head("fix", None), "acme"), "fix");
    }
}
