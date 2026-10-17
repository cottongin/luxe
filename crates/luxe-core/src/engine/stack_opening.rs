//! Engine stack opening — a branch's review published as a stack of pull requests.
//!
//! The review's chapters are the layers ([`crate::guided_review::stack_draft`]). Preparing drafts
//! them and checks they can be made: the reviewed work is uncommitted and starts at `HEAD`, so
//! each chapter can be its own commit, and no branch a layer needs is taken. Opening commits them,
//! gives each lower layer a branch at its last commit — the checked-out branch is the top — pushes
//! all of them at once, and opens the pull requests from the bottom, each on the one below.

use super::opening::{base_branch, git, head_ref, GIT_DEADLINE, PUSH_DEADLINE};
use super::posting::sentence;
use super::*;
use crate::github::{GitHub, NewPullRequest, PullRequestState};
use crate::guided_review::{LocalGit, PullRequestOpenId, ReviewBase};
use std::path::PathBuf;

impl Engine {
    /// Drafts the open review as a stack, for the reader to confirm, and keeps the layers.
    pub(crate) async fn handle_prepare_stack(
        &self,
        request_id: PullRequestOpenId,
        review_id: ReviewId,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let review = match self.open_review(&review_id, None).await {
            Ok((review, _)) => review,
            Err(refusal) => {
                let event =
                    Event::StackPrepared { request_id, layers: None, refusal: Some(refusal) };
                let _ = event_tx.send(event).await;
                return;
            }
        };
        let github = self.config.github.clone();
        let root = self.config.guided_review_project_root.clone();
        let review_git = root.clone().map(|root| self.review_git(root));
        let issue = self.session_issue().await;
        let previews = Arc::clone(&self.previews);
        super::previews::lock(&previews).stack.begin(request_id.clone());
        let event_tx = event_tx.clone();
        tokio::spawn(async move {
            let event = match prepare(github, root, review_git, &review, issue).await {
                Ok(layers) => {
                    super::previews::lock(&previews).stack.keep(
                        &request_id,
                        super::previews::StackPreview {
                            review_id: review.id.clone(),
                            revision_id: review.revision_id.clone(),
                            layers: layers.clone(),
                        },
                    );
                    Event::StackPrepared { request_id, layers: Some(layers), refusal: None }
                }
                Err(refusal) => {
                    Event::StackPrepared { request_id, layers: None, refusal: Some(refusal) }
                }
            };
            let _ = event_tx.send(event).await;
        });
    }

    /// Commits the reviewed chapters, branches and pushes the layers, and opens their pull
    /// requests from the bottom, exactly as `request_id` drafted them.
    ///
    /// Once only: what opened is on GitHub, and a second attempt would open it again.
    pub(crate) async fn handle_open_stack(
        &self,
        request_id: PullRequestOpenId,
        draft: bool,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let refused = |failed: String| Event::StackOpened {
            request_id: request_id.clone(),
            numbers: Vec::new(),
            urls: Vec::new(),
            failed: Some(failed),
        };
        let preview = super::previews::lock(&self.previews).stack.take(&request_id);
        let Some(preview) = preview else {
            let failed = "That stack is not the one drafted last, or it already opened, so \
                          nothing was opened. Prepare it again."
                .to_string();
            let _ = event_tx.send(refused(failed)).await;
            return;
        };
        let review = match self.open_review(&preview.review_id, Some(&preview.revision_id)).await {
            Ok((review, _)) => review,
            Err(failed) => {
                let _ = event_tx.send(refused(failed)).await;
                return;
            }
        };
        let layers: Vec<NewPullRequest> =
            preview.layers.into_iter().map(|layer| NewPullRequest { draft, ..layer }).collect();
        let github = self.config.github.clone();
        let root = self.config.guided_review_project_root.clone();
        let review_git = root.clone().map(|root| self.review_git(root));
        let issue = self.session_issue().await;
        let actions = self.config.action_tx.clone();
        let event_tx = event_tx.clone();
        tokio::spawn(async move {
            let (opened, failed) = open(github, root, review_git, &review, issue, &layers).await;
            if let Some(actions) = &actions {
                for (number, url) in &opened {
                    let link = Action::LinkPullRequest { number: *number, url: url.clone() };
                    let _ = actions.send(link).await;
                }
            }
            let (numbers, urls) = opened.into_iter().unzip();
            let _ = event_tx.send(Event::StackOpened { request_id, numbers, urls, failed }).await;
        });
    }
}

/// The branch each layer is on: `<branch>-part-<n>` below, the checked-out branch on top.
fn layer_branches(top: &str, count: usize) -> Vec<String> {
    (1..count)
        .map(|part| format!("{top}-part-{part}"))
        .chain(std::iter::once(top.to_string()))
        .collect()
}

async fn prepare(
    github: Option<Arc<dyn GitHub>>,
    root: Option<PathBuf>,
    review_git: Option<LocalGit>,
    review: &GuidedReview,
    issue: Option<u64>,
) -> Result<Vec<NewPullRequest>, String> {
    let github =
        github.ok_or("Opening a stack needs the GitHub CLI, and this run cannot reach it.")?;
    let root = root.ok_or("Opening a stack needs a project, and this run has none.")?;
    let review_git = review_git.ok_or("Opening a stack needs a project, and this run has none.")?;
    let repository = github.repository(&root).await.map_err(|error| sentence(&error))?;
    let head = crate::github::branch_head(&root)
        .await
        .ok_or("Opening a stack needs a branch checked out, and HEAD is detached.")?;
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
    let status =
        github.branch_status(&repository, &head).await.map_err(|error| sentence(&error))?;
    if let Some(open) = status.pull_request.filter(|found| found.state == PullRequestState::Open) {
        return Err(format!("#{} is already open for this branch: {}", open.number, open.url));
    }
    // Each chapter its own commit: only work not committed yet, from `HEAD`, can be split that way
    let uncommitted =
        git(&root, &["status", "--porcelain"], GIT_DEADLINE).await.map(|out| !out.is_empty());
    if !uncommitted.unwrap_or(false) {
        return Err(
            "A stack is made from reviewed work not committed yet, one commit per chapter, \
                    and nothing here is uncommitted: Open pull request carries what is committed."
                .into(),
        );
    }
    if let Some(refusal) = super::opening::staged_refusal(&root).await {
        return Err(refusal);
    }
    let from_head = review_git.starts_at_head(review).await.map_err(|error| error.to_string())?;
    if !from_head {
        return Err("This review starts before HEAD, so its chapters are already mixed into \
                    commits and cannot be split: Open pull request opens it as one."
            .into());
    }
    let closed = issue.map(|number| crate::guided_review::ClosedIssue { number, title: None });
    let drafts = crate::guided_review::stack_draft(review, closed.as_ref()).ok_or(
        "This review has one chapter worth reading, which is one pull request, not a stack.",
    )?;
    let branches = layer_branches(&head.name, drafts.len());
    for branch in &branches[..branches.len() - 1] {
        let local = format!("refs/heads/{branch}");
        if git(&root, &["rev-parse", "--verify", "--quiet", &local], GIT_DEADLINE).await.is_ok() {
            return Err(format!("A branch named `{branch}` already exists, which a layer needs."));
        }
    }
    let heads: Vec<String> = branches
        .iter()
        .map(|name| {
            head_ref(
                &crate::github::BranchHead { name: name.clone(), owner: head.owner.clone() },
                &repository.owner,
            )
        })
        .collect();
    let mut below = base;
    let mut layers = Vec::new();
    for ((draft, branch), head) in drafts.into_iter().zip(&branches).zip(heads) {
        layers.push(NewPullRequest {
            head,
            base: std::mem::replace(&mut below, branch.clone()),
            title: draft.title,
            body: draft.body,
            draft: false,
        });
    }
    Ok(layers)
}

/// Answers the pull requests that opened, bottom first, and what stopped the rest.
async fn open(
    github: Option<Arc<dyn GitHub>>,
    root: Option<PathBuf>,
    review_git: Option<LocalGit>,
    review: &GuidedReview,
    issue: Option<u64>,
    layers: &[NewPullRequest],
) -> (Vec<(u64, String)>, Option<String>) {
    let mut opened = Vec::new();
    let made =
        make_branches(github.as_ref(), root.as_ref(), review_git, review, issue, layers).await;
    let (github, root) = match made {
        Ok(ready) => ready,
        Err(message) => return (opened, Some(message)),
    };
    let repository = match github.repository(&root).await {
        Ok(repository) => repository,
        Err(error) => return (opened, Some(sentence(&error))),
    };
    for layer in layers {
        match github.open_pull_request(&repository, layer).await {
            Ok(pull_request) => opened.push((pull_request.number, pull_request.url)),
            Err(error) => {
                let said = format!(
                    "`{}` was pushed, but its pull request did not open: {}",
                    layer.head,
                    sentence(&error)
                );
                return (opened, Some(said));
            }
        }
    }
    (opened, None)
}

/// Commits the chapters, branches the lower layers at their last commit, and pushes every layer.
async fn make_branches(
    github: Option<&Arc<dyn GitHub>>,
    root: Option<&PathBuf>,
    review_git: Option<LocalGit>,
    review: &GuidedReview,
    issue: Option<u64>,
    layers: &[NewPullRequest],
) -> Result<(Arc<dyn GitHub>, PathBuf), String> {
    let github = github
        .cloned()
        .ok_or("Opening a stack needs the GitHub CLI, and this run cannot reach it.")?;
    let root = root.cloned().ok_or("Opening a stack needs a project, and this run has none.")?;
    let review_git = review_git.ok_or("Opening a stack needs a project, and this run has none.")?;
    let closed = issue.map(|number| crate::guided_review::ClosedIssue { number, title: None });
    let drafts = crate::guided_review::stack_draft(review, closed.as_ref())
        .filter(|drafts| drafts.len() == layers.len())
        .ok_or("The review changed since the stack was drafted: open it again.")?;
    let commits = crate::guided_review::chapter_commits(review, &[])
        .map_err(|error| format!("The reviewed work cannot be committed: {error}."))?;
    let created =
        review_git.commit_review_chapters(review, &commits, false).await.map_err(|error| {
            format!("Committing the reviewed work failed: {error}. Nothing was pushed.")
        })?;
    let committed: std::collections::HashMap<_, _> =
        commits.iter().map(|commit| commit.chapter_id.clone()).zip(created).collect();
    let top = crate::github::branch_head(&root).await.map(|head| head.name).unwrap_or_default();
    let branches = layer_branches(&top, layers.len());
    for (draft, branch) in drafts.iter().zip(&branches).take(layers.len() - 1) {
        let tip =
            draft.chapters.iter().rev().find_map(|chapter| committed.get(chapter)).ok_or_else(
                || format!("No commit carries `{branch}`'s chapters, so it was not made."),
            )?;
        git(&root, &["branch", branch, tip], GIT_DEADLINE)
            .await
            .map_err(|said| format!("The branch `{branch}` could not be made: {said}"))?;
    }
    let configured =
        git(&root, &["config", &format!("branch.{top}.remote")], GIT_DEADLINE).await.ok();
    let remote = configured.unwrap_or_else(|| "origin".to_string());
    let mut push = vec!["push", "--quiet", "--atomic", "--set-upstream", remote.as_str()];
    push.extend(branches.iter().map(String::as_str));
    git(&root, &push, PUSH_DEADLINE)
        .await
        .map_err(|said| format!("The stack's branches were made here but not pushed: {said}"))?;
    Ok((github, root))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layers_branch_off_the_checked_out_one_which_is_the_top() {
        assert_eq!(layer_branches("fix", 3), ["fix-part-1", "fix-part-2", "fix"]);
        assert_eq!(layer_branches("fix", 1), ["fix"]);
    }
}
