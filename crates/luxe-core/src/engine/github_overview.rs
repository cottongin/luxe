//! Engine GitHub overview — one pull request's or issue's detail, read for `/pr` and `/github`.
//!
//! A read and nothing else: it touches no checkout, so a pull request on another repository needs
//! no remote pointing at it, only an account that can see it. The answer goes back keyed by the
//! repository and number asked about, so a reply that lands after the reader moved on is matched
//! to nothing rather than drawn over the wrong row.
//!
//! An issue's answer also says whether one of your sessions took it up — Luxe's own knowledge, read
//! from the session store, which GitHub has no way to know.

use super::posting::sentence;
use super::*;
use crate::github::overview::{LocalSession, Overview};
use crate::github::Repository;

impl Engine {
    /// Reads pull request or issue `number` on the checkout's repository, or on `repository`.
    pub(crate) async fn handle_read_github_overview(
        &self,
        repository: Option<String>,
        number: u64,
        pull_request: bool,
        event_tx: &mpsc::Sender<Event>,
    ) {
        // Only an issue here can be taken up from here, so only one here has a session to resume
        let local = match (pull_request, &repository) {
            (false, None) => self.local_session_on_issue(number).await,
            _ => None,
        };
        let github = self.config.github.clone();
        let root = self.config.guided_review_project_root.clone();
        let event_tx = event_tx.clone();
        tokio::spawn(async move {
            let asked = repository.clone();
            let (overview, error) = match read(github, root, repository, number, pull_request).await
            {
                Ok(overview) => (Some(overview), None),
                Err(reason) => (None, Some(reason)),
            };
            let event =
                Event::GithubOverviewRead { repository: asked, number, overview, error, local };
            let _ = event_tx.send(event).await;
        });
    }

    /// Reads everything said about pull request or issue `number`, for its Conversation tab.
    pub(crate) fn handle_read_github_conversation(
        &self,
        repository: Option<String>,
        number: u64,
        pull_request: bool,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let github = self.config.github.clone();
        let root = self.config.guided_review_project_root.clone();
        let event_tx = event_tx.clone();
        tokio::spawn(async move {
            let asked = repository.clone();
            let read = async {
                let (github, repository) = reach(github, root, repository).await?;
                github.conversation(&repository, number, pull_request).await.map_err(|error| {
                    if pull_request {
                        sentence(&error)
                    } else {
                        issue_sentence(&error)
                    }
                })
            };
            let (entries, error) = match read.await {
                Ok(entries) => (Some(entries), None),
                Err(reason) => (None, Some(reason)),
            };
            let event = Event::GithubConversationRead { repository: asked, number, entries, error };
            let _ = event_tx.send(event).await;
        });
    }

    /// Your latest session, other than this one, that took issue `number` up.
    ///
    /// Every session is read to find it, which is cheap at the size sessions are — this project's
    /// 171 come to under two megabytes — and only an issue's detail asks.
    async fn local_session_on_issue(&self, number: u64) -> Option<LocalSession> {
        let current = self.current_session_id().await;
        let store = self.config.session_store.read().ok()?;
        store
            .list_sessions()
            .into_iter()
            .filter(|session| *session != current)
            .filter_map(|session| {
                let entries = store.load(&session).ok()?;
                (crate::session::session_issue(&entries) == Some(number)).then(|| LocalSession {
                    branch: crate::session::session_worktree(&entries),
                    last_used: entries
                        .last()
                        .map_or(0, |entry| u64::try_from(entry.ts).unwrap_or(0)),
                    session: session.0,
                })
            })
            .max_by_key(|local| local.last_used)
    }
}

/// GitHub and the repository a read is about: the checkout's, or the one `requested` names.
pub(super) async fn reach(
    github: Option<Arc<dyn crate::github::GitHub>>,
    root: Option<std::path::PathBuf>,
    requested: Option<String>,
) -> Result<(Arc<dyn crate::github::GitHub>, Repository), String> {
    let github =
        github.ok_or("Reading GitHub needs the GitHub CLI, and this run cannot reach it.")?;
    let root = root.ok_or("Reading GitHub needs a project, and this run has none.")?;
    let checkout = github.repository(&root).await.map_err(|error| sentence(&error))?;
    let repository = match requested.as_deref() {
        None => checkout,
        Some(slug) if checkout.is(slug) => checkout,
        Some(slug) => {
            let (owner, name) =
                slug.split_once('/').ok_or_else(|| format!("`{slug}` is not owner/name."))?;
            Repository {
                host: checkout.host,
                owner: owner.into(),
                name: name.into(),
                default_branch: String::new(),
                remote: None,
            }
        }
    };
    Ok((github, repository))
}

async fn read(
    github: Option<Arc<dyn crate::github::GitHub>>,
    root: Option<std::path::PathBuf>,
    requested: Option<String>,
    number: u64,
    pull_request: bool,
) -> Result<Overview, String> {
    let (github, repository) = reach(github, root, requested).await?;
    if pull_request {
        let overview = github
            .pull_request_overview(&repository, number)
            .await
            .map_err(|error| sentence(&error))?;
        return Ok(Overview::PullRequest(Box::new(overview)));
    }
    let overview =
        github.issue_overview(&repository, number).await.map_err(|error| issue_sentence(&error))?;
    Ok(Overview::Issue(Box::new(overview)))
}

/// What went wrong reading an issue: the same sentences, but naming an issue where they would say
/// a pull request.
fn issue_sentence(error: &crate::github::GitHubError) -> String {
    match error {
        crate::github::GitHubError::NotFound => {
            "GitHub has no such issue that this account can see.".into()
        }
        other => sentence(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With nothing to ask, the reason is a sentence the reader can act on, never a panic.
    #[tokio::test]
    async fn a_run_without_gh_or_a_project_says_why() {
        let said = read(None, None, None, 1, true).await.unwrap_err();
        assert!(said.contains("GitHub CLI"), "{said}");
    }
}
