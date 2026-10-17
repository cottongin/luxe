//! Engine GitHub list — what `/github` shows: each section's search, run through GitHub.
//!
//! A section is a GitHub search with `{repo}` standing for the checkout's repository. Opening the
//! panel runs every section at once and answers them together; one GitHub refuses leaves the others
//! listed, and says why in its own place. A tab's edited query and each next page run on their own,
//! answered under the id the front-end gave them.

use super::posting::sentence;
use super::*;
use crate::config::GithubSection;
use crate::github::{GitHub, ListedSection, Repository};

impl Engine {
    /// Runs every section's search and answers them together, each with its rows or its reason.
    pub(crate) fn handle_list_github(
        &self,
        sections: Vec<GithubSection>,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let github = self.config.github.clone();
        let root = self.config.guided_review_project_root.clone();
        let event_tx = event_tx.clone();
        tokio::spawn(async move {
            let (repository, sections) = list(github, root, sections).await;
            let _ = event_tx.send(Event::GithubListed { repository, sections }).await;
        });
    }

    /// Runs one section's search, a page of it when `after` names where the last one ended.
    pub(crate) fn handle_search_github(
        &self,
        request: u64,
        section: GithubSection,
        after: Option<String>,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let github = self.config.github.clone();
        let root = self.config.guided_review_project_root.clone();
        let event_tx = event_tx.clone();
        tokio::spawn(async move {
            let (repository, section) = match place(github, root).await {
                Ok((github, repository)) => {
                    let found = search(&*github, &repository, section, after.as_deref()).await;
                    (Some(repository.slug()), found)
                }
                Err(reason) => (None, failed(section, reason)),
            };
            let _ = event_tx.send(Event::GithubSearched { request, repository, section }).await;
        });
    }
}

async fn list(
    github: Option<Arc<dyn GitHub>>,
    root: Option<std::path::PathBuf>,
    sections: Vec<GithubSection>,
) -> (Option<String>, Vec<ListedSection>) {
    let (github, repository) = match place(github, root).await {
        Ok(place) => place,
        Err(reason) => {
            let sections = sections.into_iter().map(|s| failed(s, reason.clone())).collect();
            return (None, sections);
        }
    };
    // Each is its own `gh` and its own point, so five tabs open about as fast as one
    let searches = sections.into_iter().map(|section| search(&*github, &repository, section, None));
    let listed = futures::future::join_all(searches).await;
    (Some(repository.slug()), listed)
}

/// GitHub and the checkout's repository, or why a search cannot run here, as a sentence.
async fn place(
    github: Option<Arc<dyn GitHub>>,
    root: Option<std::path::PathBuf>,
) -> Result<(Arc<dyn GitHub>, Repository), String> {
    let Some(github) = github else {
        return Err("Listing GitHub needs the GitHub CLI, and this run cannot reach it.".into());
    };
    let Some(root) = root else {
        return Err("Listing GitHub needs a project, and this run has none.".into());
    };
    let repository = github.repository(&root).await.map_err(|error| sentence(&error))?;
    Ok((github, repository))
}

/// One page of `section`'s search, `{repo}` read as `repository`.
async fn search(
    github: &(impl crate::github::Discovery + ?Sized),
    repository: &Repository,
    section: GithubSection,
    after: Option<&str>,
) -> ListedSection {
    let query = section.query.replace("{repo}", &repository.slug());
    match github.search(&repository.host, &query, after).await {
        Ok(page) => ListedSection {
            name: section.name,
            items: page.items,
            error: None,
            total: Some(page.total),
            next: page.next,
        },
        Err(error) => failed(section, sentence(&error)),
    }
}

fn failed(section: GithubSection, reason: String) -> ListedSection {
    ListedSection {
        name: section.name,
        items: Vec::new(),
        error: Some(reason),
        total: None,
        next: None,
    }
}
