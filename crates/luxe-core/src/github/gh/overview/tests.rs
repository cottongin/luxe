//! The overview query against real answers, and against GitHub itself.

use super::*;
use crate::github::overview::{merge_checks, CheckAction};

fn fixture(name: &str) -> PullRequestOverview {
    let body = match name {
        "clean_bot" => include_str!("../../fixtures/pr_overview_clean_bot.json"),
        "merged" => include_str!("../../fixtures/pr_overview_merged.json"),
        "conflicts" => include_str!("../../fixtures/pr_overview_conflicts_changes_requested.json"),
        "approved" => include_str!("../../fixtures/pr_overview_blocked_approved.json"),
        "failing" => include_str!("../../fixtures/pr_overview_failing_review_required.json"),
        "auto_merge_on" => include_str!("../../fixtures/pr_overview_auto_merge_on.json"),
        "auto_merge_available" => {
            include_str!("../../fixtures/pr_overview_auto_merge_available.json")
        }
        "auto_merge_enabled" => include_str!("../../fixtures/pr_overview_auto_merge_enabled.json"),
        other => panic!("no fixture {other}"),
    };
    parse_pull_request_overview(body).expect("the fixture parses")
}

/// A dependabot pull request on a repository the account administers: clean, every method
/// allowed, and nothing to act on but the merge.
#[test]
fn a_clean_pull_request_reads_whole() {
    let pr = fixture("clean_bot");
    assert_eq!((pr.number, pr.repository.as_str()), (197, "nora-quinn/luxe-dev"));
    assert_eq!(pr.author, Author { login: "dependabot".into(), bot: true });
    assert_eq!((pr.base_ref.as_str(), pr.merge_status), ("master", MergeStatus::Clean));
    assert_eq!((pr.additions, pr.deletions, pr.changed_files, pr.commits), (35, 60, 1, 1));
    assert_eq!(pr.checks.passed, 5);
    assert!(pr.failing.is_empty() && pr.reviewers.is_empty());
    assert_eq!(
        pr.labels.iter().map(|label| label.name.as_str()).collect::<Vec<_>>(),
        ["dependencies", "rust"]
    );
    assert_eq!(pr.merge_options(), [MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase]);
    assert_eq!(pr.viewer, Viewer { author: false, can_push: true, can_merge: true });
    assert_eq!(pr.updated_at, 1_790_895_769, "2026-10-01T23:02:49Z");
    assert_eq!(merge_checks(&pr).last().and_then(|row| row.action), Some(CheckAction::Merge));
    // Clean, so there is nothing for auto-merge to wait on, and the repository keeps branches
    assert_eq!(
        pr.merge_settings,
        MergeSettings {
            last_method: Some(MergeMethod::Merge),
            deletes_branch: false,
            can_auto_merge: false,
            can_cancel_auto_merge: false,
            auto_merge: None,
        }
    );
    assert_eq!(pr.default_merge_method(), MergeMethod::Merge);
}

/// vercel/next.js #98546, with auto-merge on: the method it will use is read and said as a row,
/// and an account that cannot merge there may neither turn it on nor off.
#[test]
fn auto_merge_that_is_on_is_said() {
    let pr = fixture("auto_merge_on");
    assert_eq!(pr.merge_settings.auto_merge, Some(MergeMethod::Squash));
    assert!(!pr.merge_settings.can_auto_merge && !pr.merge_settings.can_cancel_auto_merge);
    let row = merge_checks(&pr).into_iter().find(|row| row.label == "Auto-merge").expect("said");
    assert_eq!(row.detail, format!("squash and merge into {} when ready", pr.base_ref));
    assert_eq!(row.action, None, "not this account's to change");
}

/// On a scratch repository this account administers, with a required check that never reports:
/// blocked, so auto-merge is offered; on, it says how it will merge and offers turning it off.
#[test]
fn auto_merge_is_offered_while_blocked_and_cancellable_once_on() {
    let available = fixture("auto_merge_available");
    assert_eq!(available.node_id, "PR_kwDOU5qvE88AAAABGZIsWQ");
    assert_eq!(available.merge_status, MergeStatus::Blocked);
    let row = |pr: &PullRequestOverview| {
        merge_checks(pr).into_iter().find(|row| row.label == "Auto-merge").expect("a row")
    };
    assert_eq!(row(&available).action, Some(CheckAction::EnableAutoMerge));
    assert_eq!(row(&available).detail, "off");

    let enabled = fixture("auto_merge_enabled");
    assert_eq!(enabled.merge_settings.auto_merge, Some(MergeMethod::Squash));
    assert_eq!(row(&enabled).action, Some(CheckAction::DisableAutoMerge));
    assert_eq!(row(&enabled).detail, "squash and merge into main when ready");
}

/// cli/cli deletes a merged branch by itself, and an account that cannot merge there may not turn
/// auto-merge on either, however blocked the pull request is.
#[test]
fn a_repository_that_deletes_branches_says_so() {
    let pr = fixture("approved");
    assert!(pr.merge_settings.deletes_branch);
    assert!(!pr.merge_settings.can_auto_merge && pr.merge_settings.auto_merge.is_none());
}

#[test]
fn a_merged_pull_request_has_nothing_in_its_way() {
    let pr = fixture("merged");
    assert_eq!(pr.state, PullRequestState::Merged);
    assert!(merge_checks(&pr).is_empty());
}

/// Someone else's repository, read-only: a conflict, a reviewer asking for changes, and a bot's
/// comment that does not count as a verdict. No action is offered on any of it.
#[test]
fn a_conflicted_pull_request_with_changes_requested_offers_nothing_to_a_reader() {
    let pr = fixture("conflicts");
    assert_eq!(pr.merge_status, MergeStatus::Dirty);
    assert_eq!(pr.review_decision, Some(ReviewDecision::ChangesRequested));
    let verdicts: Vec<_> =
        pr.reviewers.iter().map(|r| (r.login.as_str(), r.kind, r.verdict)).collect();
    assert_eq!(
        verdicts,
        [
            ("BagToad", ReviewerKind::User, Verdict::ChangesRequested),
            ("copilot-pull-request-reviewer", ReviewerKind::Bot, Verdict::Commented),
        ]
    );
    assert!(pr.reviewers[0].at.is_some());
    assert_eq!(pr.viewer, Viewer::default());
    assert!(merge_checks(&pr).iter().all(|row| row.action.is_none()));
    assert!(merge_checks(&pr).iter().any(|row| row.label == "Conflicts"));
}

/// Approved and still blocked: GitHub says no, so there is no Ready row however green it looks.
#[test]
fn an_approved_pull_request_github_still_blocks_offers_no_merge() {
    let pr = fixture("approved");
    assert_eq!(
        (pr.merge_status, pr.review_decision),
        (MergeStatus::Blocked, Some(ReviewDecision::Approved))
    );
    assert!(merge_checks(&pr).iter().all(|row| row.label != "Ready"));
}

/// A failing check by name, reviewers asked and not yet answering, and a request to a team this
/// account cannot see — which GitHub sends as `null` and is left out rather than failing.
#[test]
fn failing_checks_and_pending_reviewers_are_named() {
    let pr = fixture("failing");
    assert_eq!(pr.checks.failed, 1);
    assert_eq!(pr.failing, [FailingCheckName { name: "govulncheck".into(), required: false }]);
    let requested: Vec<&str> = pr
        .reviewers
        .iter()
        .filter(|reviewer| reviewer.verdict == Verdict::Requested)
        .map(|reviewer| reviewer.login.as_str())
        .collect();
    assert_eq!(requested, ["williammartin", "BagToad"]);
    let rows = merge_checks(&pr);
    assert!(rows.iter().any(|row| row.detail == "1 of 18 failing: govulncheck"), "{rows:#?}");
}

/// The query against GitHub itself, on a merged pull request in a public repository that will
/// not change under it: it parses, and it says what it is.
#[tokio::test]
#[ignore = "needs a signed-in gh and network access"]
async fn a_real_gh_reads_a_public_pull_request() {
    use crate::github::{PullRequests, Repository};
    let repository = Repository {
        host: "github.com".into(),
        owner: "cli".into(),
        name: "cli".into(),
        default_branch: String::new(),
        remote: None,
    };
    let pr = super::super::GhCli::new()
        .pull_request_overview(&repository, 8_474)
        .await
        .expect("a public pull request reads");
    assert_eq!((pr.number, pr.repository.as_str()), (8_474, "cli/cli"));
    assert_eq!(pr.state, PullRequestState::Merged);
    assert!(!pr.viewer.can_merge, "a stranger's repository");
}

#[test]
fn a_missing_pull_request_is_not_found() {
    let body = r#"{"data":{"repository":{"nameWithOwner":"a/b","viewerPermission":"READ","mergeCommitAllowed":true,"squashMergeAllowed":true,"rebaseMergeAllowed":true,"pullRequest":null}}}"#;
    assert!(matches!(parse_pull_request_overview(body), Err(GitHubError::NotFound)));
}

/// Required checks lead, and a re-request makes a reviewer pending again.
#[test]
fn required_checks_lead_and_a_re_request_is_pending() {
    let node = |name: &str, required| CheckNode {
        name: Some(name.into()),
        context: None,
        conclusion: Some("FAILURE".into()),
        state: None,
        is_required: required,
    };
    let names: Vec<String> = failing(vec![node("lint", false), node("test", true)])
        .into_iter()
        .map(|check| check.name)
        .collect();
    assert_eq!(names, ["test", "lint"]);

    let review = ReviewNode {
        author: Some(Actor { typename: "User".into(), login: "jon".into() }),
        state: "APPROVED".into(),
        submitted_at: None,
    };
    let again = RequestNode {
        requested_reviewer: Some(Requested {
            typename: "User".into(),
            login: Some("jon".into()),
            slug: None,
            organization: None,
        }),
    };
    let team = RequestNode {
        requested_reviewer: Some(Requested {
            typename: "Team".into(),
            login: None,
            slug: Some("core".into()),
            organization: Some(Login { login: "acme".into() }),
        }),
    };
    let reviewers = reviewers(vec![review], Vec::new(), vec![again, team]);
    let shown: Vec<_> = reviewers.iter().map(|r| (r.login.as_str(), r.verdict)).collect();
    assert_eq!(shown, [("jon", Verdict::Requested), ("acme/core", Verdict::Requested)]);
    assert_eq!(reviewers[1].kind, ReviewerKind::Team);
}
