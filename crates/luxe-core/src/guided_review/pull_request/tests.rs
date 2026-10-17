use super::*;

/// Both kinds of pull request review name it the same way, with the owner only when it is on
/// another repository; a review of anything else has no such name.
#[test]
fn a_pull_requests_review_is_named_with_its_repository() {
    let own = ReviewTarget::OwnPullRequest { number: 42 };
    assert_eq!(label(&own, None).as_deref(), Some("pull request #42"), "not read yet");
    assert_eq!(label(&own, Some("acme/notify")).as_deref(), Some("pull request #42 · notify"));
    let read = ReviewTarget::PullRequest { number: 42, repository: None };
    assert_eq!(label(&read, Some("acme/notify")).as_deref(), Some("pull request #42 · notify"));
    let elsewhere =
        ReviewTarget::PullRequest { number: 42, repository: Some("acme/notify".into()) };
    assert_eq!(
        label(&elsewhere, Some("acme/notify")).as_deref(),
        Some("pull request #42 · acme/notify")
    );
    assert_eq!(label(&ReviewTarget::WorkingTree, Some("acme/notify")), None);
}
use crate::github::{Account, Author, CheckRollup, Issue, PullRequestState, ReviewThreads};

/// A GitHub double whose checkout is `me/app`, whose pull requests close five issues, and whose issue
/// #2 cannot be read.
struct Canned {
    checkout_remote: Option<String>,
}

#[async_trait::async_trait]
impl crate::github::Discovery for Canned {
    async fn account(&self, _host: Option<&str>) -> Result<Account, GitHubError> {
        unreachable!()
    }

    async fn repository(&self, _cwd: &Path) -> Result<Repository, GitHubError> {
        Ok(Repository {
            host: "github.com".into(),
            owner: "me".into(),
            name: "app".into(),
            default_branch: "main".into(),
            remote: self.checkout_remote.clone(),
        })
    }

    async fn search(
        &self,
        _host: &str,
        _query: &str,
        _after: Option<&str>,
    ) -> Result<crate::github::SearchPage, GitHubError> {
        Ok(crate::github::SearchPage::default())
    }
}

#[async_trait::async_trait]
impl crate::github::PullRequests for Canned {
    async fn pull_request(
        &self,
        repository: &Repository,
        number: u64,
    ) -> Result<PullRequest, GitHubError> {
        Ok(PullRequest {
            number,
            title: format!("Change on {}", repository.slug()),
            body: String::new(),
            url: String::new(),
            author: Author { login: "someone".into(), bot: false },
            draft: false,
            state: PullRequestState::Open,
            base_ref: "main".into(),
            base_oid: "a".repeat(40),
            head_ref: "topic".into(),
            head_oid: "b".repeat(40),
            cross_repository: true,
            review_decision: None,
            checks: CheckRollup::default(),
            closing_issues: (1..=5)
                .map(|number| IssueRef { repository: repository.slug(), number })
                .collect(),
        })
    }

    async fn issue(&self, _repository: &Repository, number: u64) -> Result<Issue, GitHubError> {
        if number == 2 {
            return Err(GitHubError::NotFound);
        }
        Ok(Issue {
            number,
            title: format!("Issue {number}"),
            body: "x".repeat(2_000),
            url: String::new(),
            author: "reporter".into(),
            comments: Vec::new(),
            total_comments: 0,
        })
    }

    async fn conversation(
        &self,

        _repository: &Repository,

        _number: u64,

        _pull_request: bool,
    ) -> Result<Vec<crate::github::overview::ConversationEntry>, GitHubError> {
        unreachable!("this double does not read conversations")
    }

    async fn issue_overview(
        &self,
        _repository: &Repository,
        _number: u64,
    ) -> Result<crate::github::overview::IssueOverview, GitHubError> {
        unreachable!("this double does not read overviews")
    }

    async fn pull_request_overview(
        &self,
        _repository: &Repository,
        _number: u64,
    ) -> Result<crate::github::overview::PullRequestOverview, GitHubError> {
        unreachable!("this double does not read overviews")
    }
}

#[async_trait::async_trait]
impl crate::github::Threads for Canned {
    async fn review_threads(
        &self,
        _repository: &Repository,
        _number: u64,
    ) -> Result<ReviewThreads, GitHubError> {
        Ok(ReviewThreads::default())
    }

    async fn reply_to_thread(
        &self,
        _repository: &Repository,
        _thread_id: &str,
        _body: &str,
    ) -> Result<String, GitHubError> {
        unreachable!("this double does not answer threads")
    }

    async fn comment_on_pull_request(
        &self,
        _repository: &Repository,
        _number: u64,
        _body: &str,
    ) -> Result<String, GitHubError> {
        unreachable!("this double does not answer threads")
    }

    async fn resolve_thread(
        &self,
        _repository: &Repository,
        _thread_id: &str,
    ) -> Result<(), GitHubError> {
        unreachable!("this double does not answer threads")
    }
}

fn checkout_with_upstream() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "--quiet"],
        vec!["remote", "add", "origin", "git@github.com:me/app.git"],
        vec!["remote", "add", "upstream", "https://github.com/Acme/App.git"],
    ] {
        let status = std::process::Command::new("git").args(&args).current_dir(dir.path()).status();
        assert!(status.unwrap().success(), "git {args:?}");
    }
    dir
}

/// A fork reviews its upstream's pull request through the remote that points there, under a
/// target that keeps the repository's name.
#[tokio::test]
async fn an_upstream_pull_request_is_fetched_through_its_remote() {
    let checkout = checkout_with_upstream();
    let github = Canned { checkout_remote: Some("origin".into()) };

    let resolved = resolve_pull_request(&github, checkout.path(), 7, Some("acme/app"))
        .await
        .expect("upstream is linked");

    assert_eq!(resolved.pin.fetch_from, "upstream");
    assert_eq!(resolved.pin.repository, "acme/app");
    assert_eq!(resolved.context.title, "Change on acme/app");
    assert_eq!(
        resolved.target,
        ReviewTarget::PullRequest { number: 7, repository: Some("acme/app".into()) }
    );
}

/// The checkout's own repository with no remote naming it is a rename: fetch from the URL.
#[tokio::test]
async fn a_renamed_repository_fetches_from_its_url() {
    let checkout = checkout_with_upstream();
    let github = Canned { checkout_remote: None };

    let resolved = resolve_pull_request(&github, checkout.path(), 7, Some("ME/app")).await.unwrap();

    assert_eq!(resolved.pin.fetch_from, "https://github.com/me/app.git");
    assert_eq!(resolved.target, ReviewTarget::PullRequest { number: 7, repository: None });
}

/// A repository nothing here points at is refused by name, and so is one that is not a slug.
#[tokio::test]
async fn an_unlinked_repository_is_refused() {
    let checkout = checkout_with_upstream();
    let github = Canned { checkout_remote: Some("origin".into()) };
    for slug in ["stranger/app", "not-a-slug"] {
        let error = resolve_pull_request(&github, checkout.path(), 7, Some(slug))
            .await
            .expect_err("not linked");
        assert!(
            matches!(&error, PullRequestResolveError::NotLinked { repository } if repository == slug),
            "{error:?}"
        );
    }
}

/// Three issues are read and the rest counted; one that cannot be read keeps its number, and a
/// long description is cut.
#[tokio::test]
async fn closing_issues_are_read_up_to_the_limit() {
    let checkout = checkout_with_upstream();
    let github = Canned { checkout_remote: Some("origin".into()) };

    let context = resolve_pull_request(&github, checkout.path(), 7, None).await.unwrap().context;

    let numbers: Vec<_> = context.closing_issues.iter().map(|issue| issue.number).collect();
    assert_eq!(numbers, vec![1, 2, 3]);
    assert_eq!(context.more_closing_issues, 2);
    assert_eq!(context.closing_issues[0].title.as_deref(), Some("Issue 1"));
    assert_eq!(context.closing_issues[1].title, None, "unreadable, still listed");
    assert!(context.closing_issues[0].excerpt.chars().count() < 700, "bounded");
}

/// A login as a person would say it.
#[test]
fn an_app_is_named_without_its_prefix() {
    let mut context = PullRequestContext::new(
        &Repository {
            host: "github.com".into(),
            owner: "acme".into(),
            name: "app".into(),
            default_branch: "main".into(),
            remote: None,
        },
        &PullRequest {
            number: 1,
            title: "t".into(),
            body: String::new(),
            url: String::new(),
            author: Author { login: "app/dependabot".into(), bot: true },
            draft: false,
            state: PullRequestState::Merged,
            base_ref: String::new(),
            base_oid: String::new(),
            head_ref: String::new(),
            head_oid: String::new(),
            cross_repository: false,
            review_decision: None,
            checks: CheckRollup::default(),
            closing_issues: Vec::new(),
        },
        Vec::new(),
    );
    assert_eq!(context.author_name(), "dependabot");
    assert_eq!(context.status, PullRequestStatus::Merged);
    context.author = "octocat".into();
    assert_eq!(context.author_name(), "octocat");
}
