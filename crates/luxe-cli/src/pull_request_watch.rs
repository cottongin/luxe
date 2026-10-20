//! Pull request watch — keeps the branch's pull request in the workspace snapshot.
//!
//! Lives in the workspace bridge, which is the only place that sees both the engine's events and
//! the snapshot, and which headless runs do not start: nothing polls GitHub where nobody is
//! looking. It decides *that* a refresh is wanted — a push the agent made, `/pr`, focus coming
//! back — and asks [`luxe_core::github::refresh::Schedule`] *when* the next background one is due;
//! at most one runs at a time, and whatever arrives while it does is folded into the next.

use luxe_core::github::refresh::{touch, Schedule, Standing, Touch, IDLE};
use luxe_core::github::{
    BranchStatus, Branches, Discovery, GitHub, GitHubError, PullRequestState, Repository,
};
use luxe_core::message::ContentBlock;
use luxe_core::workspace::PullRequestPresence;
use luxe_core::Event;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant};

/// How long after an event its refresh waits, so a burst of them is one question.
const SETTLE: Duration = Duration::from_millis(250);

/// The least time between two refreshes that events asked for. Switching windows back and forth
/// should not spend a point each time.
const EVENT_GAP: Duration = Duration::from_secs(5);

/// What one refresh found.
#[derive(Debug)]
pub(crate) struct Looked {
    /// The repository it asked about, kept so the next refresh does not ask `gh` again
    repository: Option<Repository>,
    outcome: Result<Option<BranchStatus>, GitHubError>,
}

/// The branch's pull request, watched on the bridge's behalf.
///
/// Generic over what it asks GitHub, which is only where the checkout's repository is and its
/// branch's status, so a test double answers those two roles and nothing else.
pub(crate) struct PullRequestWatch<G: ?Sized = dyn GitHub> {
    github: Arc<G>,
    root: PathBuf,
    schedule: Schedule,
    repository: Option<Repository>,
    branch: Option<String>,
    in_flight: bool,
    /// When an event asked for a refresh, if one did since the last started
    wanted: Option<Instant>,
    last_started: Option<Instant>,
    /// Tool calls committed but not yet finished that will touch the pull request
    touching: HashMap<String, Touch>,
    /// The last budget GitHub reported: points left and when it refills
    budget: Option<(u32, u64)>,
}

impl<G: Discovery + Branches + ?Sized + 'static> PullRequestWatch<G> {
    /// A watch over the checkout at `root`, settling at `refresh_secs` (`0`: events only).
    pub(crate) fn new(github: Arc<G>, root: PathBuf, refresh_secs: u64) -> Self {
        Self {
            github,
            root,
            schedule: Schedule::new(refresh_secs),
            repository: None,
            branch: None,
            in_flight: false,
            wanted: None,
            last_started: None,
            touching: HashMap::new(),
            budget: None,
        }
    }

    /// Notes what an engine event means for the pull request.
    pub(crate) fn observe(&mut self, event: &Event, now: Instant) {
        match event {
            Event::MessageCommitted { message, .. } => {
                for block in &message.content {
                    if let ContentBlock::ToolCall { id, name, arguments } = block {
                        let touched = touch(name, arguments);
                        if touched != Touch::None {
                            self.touching.insert(id.clone(), touched);
                        }
                    }
                }
            }
            // After the call, not when it was written: a push is only on GitHub once it finishes
            Event::ToolEnded { call_id, .. } => {
                if let Some(touched) = self.touching.remove(call_id) {
                    if touched == Touch::Started {
                        self.schedule.activity(now.into_std());
                    }
                    self.request(now);
                }
            }
            Event::WorkspaceRefreshRequested
            | Event::SessionLoaded { .. }
            | Event::PullRequestOpened { .. } => self.request(now),
            Event::TerminalFocusChanged { focused } => {
                self.schedule.focus(now.into_std(), *focused);
                if *focused {
                    self.request(now);
                }
            }
            _ => {}
        }
    }

    /// Notes the branch a fresh snapshot found, answering whether it changed. A new branch has
    /// a different pull request, or none, so what the snapshot carried is no longer its own.
    pub(crate) fn saw_branch(&mut self, branch: Option<&str>, now: Instant) -> bool {
        if self.branch.as_deref() == branch {
            return false;
        }
        let first = self.branch.is_none() && self.last_started.is_none();
        self.branch = branch.map(str::to_string);
        if !first {
            self.request(now);
        }
        !first
    }

    /// When the next refresh should start, or `None` while one runs or none is due.
    pub(crate) fn due(&self, now: Instant) -> Option<Instant> {
        if self.in_flight {
            return None;
        }
        let background = self.schedule.next(now.into_std()).map(Instant::from_std);
        if !self.schedule.may_ask(now.into_std()) {
            return background;
        }
        let asked = self.wanted.map(|at| match self.last_started {
            Some(last) => at.max(last + EVENT_GAP),
            None => at,
        });
        [background, asked].into_iter().flatten().min()
    }

    /// Starts a refresh, whose answer arrives on `answers`.
    pub(crate) fn start(&mut self, now: Instant, answers: &mpsc::Sender<Looked>) {
        self.in_flight = true;
        self.wanted = None;
        self.last_started = Some(now);
        let (github, root, repository) =
            (self.github.clone(), self.root.clone(), self.repository.clone());
        let answers = answers.clone();
        tokio::spawn(async move {
            let _ = answers.send(look(github, root, repository).await).await;
        });
    }

    /// Takes a refresh's answer, returning what the snapshot should now say, or `None` to keep
    /// what it says: a failed question is no reason to blank the row.
    pub(crate) fn finish(&mut self, looked: Looked, now: Instant) -> Option<PullRequestPresence> {
        self.in_flight = false;
        let at = now.into_std();
        if looked.repository.is_some() {
            self.repository = looked.repository;
        }
        match looked.outcome {
            Ok(None) => {
                self.schedule.refreshed(at, Standing::NoPullRequest, None);
                Some(PullRequestPresence::None)
            }
            Ok(Some(status)) => {
                let remaining = status.rate_limit.map(|limit| limit.remaining);
                if let Some(limit) = status.rate_limit {
                    self.budget = Some((limit.remaining, limit.reset_at));
                }
                let (standing, presence) = match status.pull_request {
                    None => (Standing::NoPullRequest, PullRequestPresence::None),
                    Some(pr) => {
                        let standing = match pr.state {
                            PullRequestState::Open => {
                                Standing::Open { checks_pending: pr.checks.pending > 0 }
                            }
                            PullRequestState::Closed | PullRequestState::Merged => {
                                Standing::Finished
                            }
                        };
                        (standing, PullRequestPresence::Found(pr))
                    }
                };
                self.schedule.refreshed(at, standing, remaining);
                if remaining == Some(0) {
                    self.schedule.rate_limited(at, self.refill(now));
                }
                Some(presence)
            }
            Err(GitHubError::RateLimited) => {
                self.schedule.rate_limited(at, self.refill(now));
                None
            }
            Err(error) => match unavailable(&error) {
                Some(reason) => {
                    self.repository = None;
                    self.schedule.refreshed(at, Standing::Unavailable, None);
                    Some(PullRequestPresence::Unavailable { reason })
                }
                None => {
                    tracing::debug!(%error, "pull request refresh failed; keeping the last answer");
                    self.schedule.refreshed(at, Standing::Failed, None);
                    None
                }
            },
        }
    }

    /// When the budget refills: GitHub's last word on it, or a few minutes when it gave none.
    fn refill(&self, now: Instant) -> std::time::Instant {
        let unix_now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let wait = self
            .budget
            .map(|(_, reset_at)| reset_at.saturating_sub(unix_now))
            .filter(|wait| *wait > 0)
            .map_or(IDLE, Duration::from_secs);
        (now + wait).into_std()
    }

    fn request(&mut self, now: Instant) {
        let at = now + SETTLE;
        self.wanted = Some(self.wanted.map_or(at, |wanted| wanted.min(at)));
    }
}

/// One refresh: where the branch is pushed, which repository it belongs to, and its pull request.
async fn look<G: Discovery + Branches + ?Sized>(
    github: Arc<G>,
    root: PathBuf,
    repository: Option<Repository>,
) -> Looked {
    let Some(head) = luxe_core::github::branch_head(&root).await else {
        // A detached HEAD has no branch, so no pull request of its own
        return Looked { repository, outcome: Ok(None) };
    };
    let repository = match repository {
        Some(repository) => repository,
        None => match github.repository(&root).await {
            Ok(repository) => repository,
            Err(error) => return Looked { repository: None, outcome: Err(error) },
        },
    };
    let outcome = github.branch_status(&repository, &head).await.map(Some);
    Looked { repository: Some(repository), outcome }
}

/// The sentence `/pr` shows for a failure asking again will not fix, or `None` for one it might.
fn unavailable(error: &GitHubError) -> Option<String> {
    let reason = match error {
        GitHubError::NotInstalled => {
            "The GitHub CLI (`gh`) is not installed, so luxe cannot see this branch's pull request."
        }
        GitHubError::NotAuthenticated { .. } => {
            "The GitHub CLI is not signed in. Run `gh auth login`, and luxe picks it up the next \
             time the terminal has focus."
        }
        GitHubError::NotGitHub => "This repository has no remote on GitHub.",
        GitHubError::NotFound => {
            "GitHub has no repository this checkout points at that this account can see."
        }
        _ => return None,
    };
    Some(reason.to_string())
}

#[cfg(test)]
mod tests;
