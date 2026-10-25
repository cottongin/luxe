use super::*;
use luxe_core::github::FailedStep;

fn target(pending: Option<usize>) -> Target {
    Target {
        oid: "1ee6ffe1e7cc5e51a79ec5df24e7e8db03f237b5".into(),
        name: "#119 · Bump getrandom".into(),
        pending,
    }
}

fn check(name: &str, log: CheckLog) -> FailingCheck {
    FailingCheck {
        name: name.into(),
        outcome: "failure".into(),
        url: format!("https://github.com/o/r/actions/runs/1/job/{name}"),
        summary: None,
        log,
    }
}

#[test]
fn nothing_failing_says_so_and_what_is_still_running() {
    assert_eq!(render(&target(Some(0)), &[]), "No checks are failing on #119 · Bump getrandom.");
    assert_eq!(
        render(&target(Some(2)), &[]),
        "No checks are failing on #119 · Bump getrandom. 2 checks are still running."
    );
    assert!(render(&target(Some(1)), &[]).ends_with("1 check is still running."));
    assert!(render(&target(None), &[]).ends_with("getrandom."));
}

/// Each kind of failure says where its log is, and a cut tail says how much it left out.
#[test]
fn every_failure_says_where_its_log_is() {
    let steps = CheckLog::Steps(vec![FailedStep {
        name: "Clippy (deny warnings)".into(),
        tail: vec!["error[E0425]: cannot find function".into(), "##[error]exit 101".into()],
        omitted: 59,
        after: 0,
    }]);
    let failing = [
        check("clippy", steps),
        FailingCheck {
            summary: Some("#20260924.53 failed".into()),
            ..check("Azure Pipelines CI", CheckLog::NotActions)
        },
        check("windows", CheckLog::Unreadable("HTTP 410: logs expired".into())),
        check("macos", CheckLog::NotRead),
        check("empty", CheckLog::Steps(Vec::new())),
        check(
            "short",
            CheckLog::Steps(vec![FailedStep {
                name: "Build".into(),
                tail: vec!["oops".into()],
                omitted: 0,
                after: 0,
            }]),
        ),
        check(
            "whole",
            CheckLog::Steps(vec![FailedStep {
                name: "UNKNOWN STEP".into(),
                tail: vec!["##[error]exit 1".into()],
                omitted: 5_000,
                after: 40,
            }]),
        ),
    ];
    let text = render(&target(Some(1)), &failing);
    assert!(text.starts_with(
        "7 checks failing on #119 · Bump getrandom (1ee6ffe). 1 check is still running."
    ));
    for expected in [
        "clippy — failure\n  https://github.com/o/r/actions/runs/1/job/clippy",
        "  step \"Clippy (deny warnings)\", last 2 of 61 lines:\n    error[E0425]",
        "Azure Pipelines CI — failure: #20260924.53 failed",
        "Not a GitHub Actions job",
        "Log unreadable: HTTP 410: logs expired",
        "Log not read: only the first few",
        "No failing step printed a log.",
        "  step \"Build\":\n    oops",
        "  the job's log (gh could not split it by step), the line up to its last error, of \
             5041:",
    ] {
        assert!(text.contains(expected), "{expected:?} in:\n{text}");
    }
}

#[test]
fn a_flood_of_failures_is_capped_and_says_so() {
    let tail: Vec<String> = (0..LOG_TAIL).map(|n| format!("line {n}")).collect();
    let failing: Vec<FailingCheck> = (0..20)
        .map(|n| {
            let step =
                FailedStep { name: "Test".into(), tail: tail.clone(), omitted: 900, after: 0 };
            check(&format!("job {n}"), CheckLog::Steps(vec![step]))
        })
        .collect();
    let text = render(&target(None), &failing);
    assert!(text.lines().count() <= MAX_LINES + 2, "{}", text.lines().count());
    assert!(text.contains("lines of check output"), "{text}");
}

#[test]
fn a_short_oid_is_seven_characters_or_all_of_it() {
    assert_eq!(short("1ee6ffe1e7cc"), "1ee6ffe");
    assert_eq!(short("abc"), "abc");
}

/// A GitHub double for this checkout's `me/app`, whose pull request #7 has one failing Actions job,
/// and for `other/site`, whose #19 has one too: reachable only by naming that repository.
struct Scripted {
    signed_in: bool,
    /// Whether the branch has a pull request
    pull_request: bool,
}

#[async_trait::async_trait]
impl luxe_core::github::Discovery for Scripted {
    async fn account(&self, _: Option<&str>) -> Result<luxe_core::github::Account, GitHubError> {
        unreachable!()
    }

    async fn repository(&self, _: &Path) -> Result<luxe_core::github::Repository, GitHubError> {
        if !self.signed_in {
            return Err(GitHubError::NotAuthenticated { host: None });
        }
        Ok(luxe_core::github::Repository {
            host: "github.com".into(),
            owner: "me".into(),
            name: "app".into(),
            default_branch: "main".into(),
            remote: Some("origin".into()),
        })
    }

    async fn search(
        &self,
        _host: &str,
        _query: &str,
        _after: Option<&str>,
    ) -> Result<luxe_core::github::SearchPage, GitHubError> {
        Ok(luxe_core::github::SearchPage::default())
    }
}

#[async_trait::async_trait]
impl luxe_core::github::PullRequests for Scripted {
    async fn pull_request(
        &self,
        repository: &luxe_core::github::Repository,
        number: u64,
    ) -> Result<luxe_core::github::PullRequest, GitHubError> {
        let (title, head_oid) = match (repository.slug().as_str(), number) {
            ("me/app", 7) => ("Topic", "7777777aaaa"),
            ("other/site", 19) => ("Site", "1919191bbbb"),
            _ => return Err(GitHubError::NotFound),
        };
        Ok(luxe_core::github::PullRequest {
            number,
            title: title.into(),
            body: String::new(),
            url: String::new(),
            author: luxe_core::github::Author { login: "me".into(), bot: false },
            draft: false,
            state: luxe_core::github::PullRequestState::Open,
            base_ref: "main".into(),
            base_oid: "b".into(),
            head_ref: "topic".into(),
            head_oid: head_oid.into(),
            cross_repository: false,
            review_decision: None,
            checks: luxe_core::github::CheckRollup { pending: 1, ..Default::default() },
            closing_issues: Vec::new(),
        })
    }

    async fn issue(
        &self,
        _: &luxe_core::github::Repository,
        _: u64,
    ) -> Result<luxe_core::github::Issue, GitHubError> {
        unreachable!()
    }

    async fn conversation(
        &self,

        _repository: &luxe_core::github::Repository,

        _number: u64,

        _pull_request: bool,
    ) -> Result<Vec<luxe_core::github::overview::ConversationEntry>, GitHubError> {
        unreachable!("this double does not read conversations")
    }

    async fn issue_overview(
        &self,
        _repository: &luxe_core::github::Repository,
        _number: u64,
    ) -> Result<luxe_core::github::overview::IssueOverview, GitHubError> {
        unreachable!("this double does not read overviews")
    }

    async fn pull_request_overview(
        &self,
        _repository: &luxe_core::github::Repository,
        _number: u64,
    ) -> Result<luxe_core::github::overview::PullRequestOverview, GitHubError> {
        unreachable!("this double does not read overviews")
    }
}

#[async_trait::async_trait]
impl luxe_core::github::Branches for Scripted {
    async fn branch_status(
        &self,
        _: &luxe_core::github::Repository,
        head: &luxe_core::github::BranchHead,
    ) -> Result<luxe_core::github::BranchStatus, GitHubError> {
        assert_eq!(head.name, "topic");
        if !self.pull_request {
            return Ok(luxe_core::github::BranchStatus { pull_request: None, rate_limit: None });
        }
        let pr = self.pull_request(&self.repository(Path::new("")).await?, 7).await?;
        Ok(luxe_core::github::BranchStatus {
            pull_request: Some(luxe_core::github::PullRequestStatus {
                number: pr.number,
                title: pr.title,
                url: pr.url,
                state: pr.state,
                draft: false,
                review_decision: None,
                checks: pr.checks,
                unresolved_threads: 0,
                head_oid: pr.head_oid,
                stack: None,
            }),
            rate_limit: None,
        })
    }

    async fn pull_request_head(
        &self,
        _repository: &luxe_core::github::Repository,
        _number: u64,
    ) -> Result<luxe_core::github::PullRequestHead, GitHubError> {
        unreachable!("this double reads no pull request heads")
    }

    async fn merged_below(
        &self,
        _repository: &luxe_core::github::Repository,
        _number: u64,
        _base: &str,
    ) -> Result<Option<luxe_core::restack::MergedBelow>, GitHubError> {
        Ok(None)
    }
}

#[async_trait::async_trait]
impl luxe_core::github::Checks for Scripted {
    async fn failing_checks(
        &self,
        repository: &luxe_core::github::Repository,
        head_oid: &str,
        log_tail: usize,
    ) -> Result<Vec<FailingCheck>, GitHubError> {
        assert_eq!(log_tail, LOG_TAIL);
        let owner = match head_oid {
            "7777777aaaa" => "me/app",
            "1919191bbbb" => "other/site",
            _ => return Ok(Vec::new()),
        };
        assert_eq!(repository.slug(), owner, "checks are read where the pull request lives");
        Ok(vec![check("test", CheckLog::NotRead)])
    }
}

#[tokio::test]
async fn a_named_pull_request_reads_its_head() {
    let github = Scripted { signed_in: true, pull_request: true };
    let text = checks(&github, Path::new("."), &["#7".into()]).await.unwrap();
    assert!(
        text.starts_with("1 check failing on #7 · Topic (7777777). 1 check is still running."),
        "{text}"
    );

    let error = checks(&github, Path::new("."), &["seven".into()]).await.unwrap_err();
    assert!(error.contains("`seven` is not a pull request number"), "{error}");
    let error = checks(&github, Path::new("."), &["8".into()]).await.unwrap_err();
    assert!(error.contains("never pushed has no checks"), "{error}");
    let error = checks(&Scripted { signed_in: false, pull_request: true }, Path::new("."), &[])
        .await
        .unwrap_err();
    assert!(error.contains("gh auth login"), "{error}");
}

/// The break this catches: `-R` dropped in silence, which reads the same number in this checkout's
/// repository instead, a different pull request entirely or none at all.
#[tokio::test]
async fn a_repository_named_with_r_is_the_one_read() {
    let github = Scripted { signed_in: true, pull_request: true };
    let spellings: [&[&str]; 3] = [
        &["19", "-R", "other/site"],
        &["--repo", "other/site", "#19"],
        &["--repo=other/site", "19"],
    ];
    for args in spellings {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        let text = checks(&github, Path::new("."), &args).await.unwrap();
        assert!(text.starts_with("1 check failing on other/site#19 · Site (1919191)."), "{text}");
    }

    let error = checks(&github, Path::new("."), &["19".into()]).await.unwrap_err();
    assert!(error.contains("never pushed has no checks"), "#19 isn't this checkout's: {error}");
}

/// The break this catches: an argument `checks` doesn't understand skipped rather than refused.
#[tokio::test]
async fn an_argument_checks_does_not_take_is_refused() {
    let github = Scripted { signed_in: true, pull_request: true };
    let refused = |args: &[&str]| {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        let github = &github;
        async move { checks(github, Path::new("."), &args).await.unwrap_err() }
    };

    let error = refused(&["7", "--json"]).await;
    assert!(error.contains("doesn't take `--json`"), "{error}");
    let error = refused(&["7", "8"]).await;
    assert!(error.contains("one pull request at a time"), "{error}");
    let error = refused(&["7", "-R"]).await;
    assert!(error.contains("`-R` needs a repository"), "{error}");
    let error = refused(&["-R", "other/site"]).await;
    assert_eq!(error, REPO_NEEDS_NUMBER, "no branch here belongs to that repository");
    let error = refused(&["7", "-R", "luxe"]).await;
    assert!(error.contains("`luxe` is not a repository"), "{error}");
}

/// `-R` reads a repository the way `gh -R` does, and refuses what GitHub couldn't name.
#[test]
fn a_repository_is_owner_and_name_or_host_owner_and_name() {
    let on_github = repository_from_slug("evanwhittaker/luxe").unwrap();
    assert_eq!(
        (on_github.host.as_str(), on_github.slug().as_str()),
        ("github.com", "evanwhittaker/luxe")
    );
    let elsewhere = repository_from_slug("GHE.example.com/team/app.rs").unwrap();
    assert_eq!(
        (elsewhere.host.as_str(), elsewhere.slug().as_str()),
        ("ghe.example.com", "team/app.rs")
    );

    for slug in ["luxe", "a/b/c/d", "owner/", "/name", "my owner/app", "owner/app?x=1", ""] {
        assert!(repository_from_slug(slug).is_err(), "`{slug}` names no repository");
    }
}

/// With nothing named, the branch's own pull request is the one read.
#[tokio::test]
async fn the_branchs_pull_request_is_the_default() {
    let dir = tempfile::tempdir().unwrap();
    let output = std::process::Command::new("git")
        .args(["init", "-q", "-b", "topic"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let text =
        checks(&Scripted { signed_in: true, pull_request: true }, dir.path(), &[]).await.unwrap();
    assert!(text.starts_with("1 check failing on #7 · Topic"), "{text}");
}

/// A branch with no pull request reads the checks on `HEAD`, and says it did.
#[tokio::test]
async fn without_a_pull_request_the_checks_on_head_are_read() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let output =
            std::process::Command::new("git").args(args).current_dir(dir.path()).output().unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    git(&["init", "-q", "-b", "topic"]);
    let github = Scripted { signed_in: true, pull_request: false };
    let error = checks(&github, dir.path(), &[]).await.unwrap_err();
    assert!(error.contains("no commit"), "{error}");

    git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "a"]);
    let head = git(&["rev-parse", "HEAD"]);
    let text = checks(&github, dir.path(), &[]).await.unwrap();
    assert_eq!(
        text,
        format!("No checks are failing on {} (this branch has no pull request).", &head[..7])
    );
}

/// Against the real `gh` and a pull request of this checkout's repository whose CI failed: each
/// failing job comes back with the compiler's error at the end of its tail. Run with
/// `LUXE_FAILED_PR=<number> cargo test -- --ignored`.
#[tokio::test]
#[ignore = "needs a signed-in gh, the network, and LUXE_FAILED_PR"]
async fn a_real_failed_run_ends_in_its_error() {
    let number = std::env::var("LUXE_FAILED_PR")
        .expect("LUXE_FAILED_PR names a pull request in this repository whose checks failed");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let github = luxe_core::github::GhCli::new();
    let text = checks(&github, &root, std::slice::from_ref(&number)).await.unwrap();
    assert!(text.contains(&format!("failing on #{number}")), "{text}");
    assert!(text.contains("##[error]Process completed with exit code"), "{text}");
}
