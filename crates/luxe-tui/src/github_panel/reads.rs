//! Reads — when `/github` asks for a row's detail, and where the answer lands.
//!
//! The cursor has to rest on a row for a moment before it is read, so holding Down through a list
//! asks for the row you stop on rather than every row you passed; each row is read once, until a
//! refresh forgets them all. An answer is matched to its row by repository and number, so one that
//! arrives after the reader moved on still lands where it belongs.

use luxe_core::github::ListItem;

use super::GithubPanel;
use crate::github_detail::GithubDetail;

/// How long the cursor rests on a row before its overview is read: holding Down through a list
/// asks for the row you stop on, not every row you passed.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(250);

/// An overview to ask the engine for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewRead {
    /// `owner/name` when it is not the checkout's repository
    pub repository: Option<String>,
    /// Its number
    pub number: u64,
    /// Whether it is a pull request, rather than an issue
    pub pull_request: bool,
}

impl GithubPanel {
    /// Watches the cursor, and answers the read to send once it has rested on a row that has not
    /// been read: one read per row, however often the cursor passes through it.
    pub fn due_read(&mut self, now: std::time::Instant) -> Option<OverviewRead> {
        let item = self.rows().get(self.selected).copied().cloned()?;
        let known = self.details.contains_key(&item.url)
            || self.failures.contains_key(&item.url)
            || self.asked.contains(&item.url);
        match &self.settling {
            Some((url, _)) if *url != item.url => {
                self.settling = Some((item.url, now));
                None
            }
            None => {
                self.settling = Some((item.url, now));
                None
            }
            Some((_, since)) if known || now.duration_since(*since) < SETTLE => None,
            Some(_) => {
                self.asked.insert(item.url.clone());
                let repository = self.elsewhere(&item).then(|| item.repository.clone());
                Some(OverviewRead {
                    repository,
                    number: item.number,
                    pull_request: item.pull_request,
                })
            }
        }
    }

    /// Whether the row under the cursor is still waiting on its overview.
    pub fn reading(&self) -> bool {
        self.rows().get(self.selected).is_some_and(|item| {
            !self.details.contains_key(&item.url) && !self.failures.contains_key(&item.url)
        })
    }

    /// Lands an overview, or why there is none, in the detail of the row it answers.
    ///
    /// Args: `(repository, number, overview, error, local, branch)` — `repository` as asked, `None`
    /// for the checkout's; `branch` the number of the checked-out branch's pull request, the only
    /// one whose threads can be worked through here.
    #[allow(clippy::too_many_arguments)]
    pub fn land(
        &mut self,
        repository: Option<&str>,
        number: u64,
        overview: Option<luxe_core::github::overview::Overview>,
        error: Option<String>,
        local: Option<luxe_core::github::overview::LocalSession>,
        branch: Option<u64>,
    ) {
        let here = self.repository.clone();
        let matches = |item: &ListItem| {
            item.number == number
                && match repository {
                    Some(slug) => item.repository.eq_ignore_ascii_case(slug),
                    None => here
                        .as_deref()
                        .is_none_or(|here| here.eq_ignore_ascii_case(&item.repository)),
                }
        };
        let Some(url) = self
            .sections
            .iter()
            .flatten()
            .flat_map(|section| &section.items)
            .find(|item| matches(item))
            .map(|item| item.url.clone())
        else {
            return;
        };
        self.asked.remove(&url);
        match overview {
            Some(overview) => {
                let on_branch = repository.is_none()
                    && matches!(overview, luxe_core::github::overview::Overview::PullRequest(_))
                    && branch == Some(number);
                let mut detail = GithubDetail::new(overview);
                detail.on_branch = on_branch;
                detail.local = local;
                self.failures.remove(&url);
                self.details.insert(url, detail);
            }
            None => {
                let reason = error.unwrap_or_else(|| "No answer from GitHub".into());
                self.failures.insert(url, reason);
            }
        }
    }

    /// Forgets every overview read, so the next look at each row reads it again.
    pub fn forget_details(&mut self) {
        self.details.clear();
        self.failures.clear();
        self.asked.clear();
        self.settling = None;
    }
}
