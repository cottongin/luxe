//! Pull requests — what `gh pr view` and `gh issue view` say, read into our types.

use super::*;

// Saved verbatim from `gh` 2.101.0 against public repositories; see `fixtures/`.
pub(super) const PR_MERGED: &str = include_str!("../../fixtures/pr_view.json");
pub(super) const PR_FAILING: &str = include_str!("../../fixtures/pr_view_failing.json");
pub(super) const PR_STATUSES: &str = include_str!("../../fixtures/pr_view_statuses.json");
pub(super) const PR_BOT: &str = include_str!("../../fixtures/pr_view_bot.json");
pub(super) const ISSUE: &str = include_str!("../../fixtures/issue_view.json");
pub(super) const PR_CLOSED: &str = include_str!("../../fixtures/pr_view_closed.json");
pub(super) const THREADS: &str = include_str!("../../fixtures/review_threads.json");
pub(super) const THREADS_LEFT: &str = include_str!("../../fixtures/review_threads_left.json");

/// A merged, approved pull request: every field the brief needs, read off `cli/cli#14429`.
#[test]
fn a_pull_request_is_read_from_pr_view() {
    let pr = parse_pull_request(PR_MERGED).unwrap();
    assert_eq!(pr.number, 14429);
    assert_eq!(pr.title, "Fix remote branch deletion for owner case mismatch");
    assert!(pr.body.starts_with("Fixes #14404"), "{}", pr.body);
    assert_eq!(pr.author, Author { login: "williammartin".into(), bot: false });
    assert_eq!(pr.state, PullRequestState::Merged);
    assert_eq!(pr.review_decision, Some(ReviewDecision::Approved));
    assert!(!pr.draft && !pr.cross_repository);
    assert_eq!(pr.base_ref, "trunk");
    assert_eq!(pr.base_oid, "7f0590c2b8050bb04c5af52291062e6825dda9df");
    assert_eq!(pr.head_oid, "a9d9d84ee3e848f0e1ea97eaef837126d3a7d983");
    assert_eq!(pr.checks, CheckRollup { passed: 11, ..CheckRollup::default() });
    assert_eq!(pr.closing_issues, vec![IssueRef { repository: "cli/cli".into(), number: 14404 }]);
}

/// A draft from a fork, waiting on review, with a failing check among skipped and passing ones.
#[test]
fn a_draft_fork_with_mixed_checks_is_read_as_such() {
    let pr = parse_pull_request(PR_FAILING).unwrap();
    assert_eq!(pr.state, PullRequestState::Open);
    assert!(pr.draft && pr.cross_repository);
    assert_eq!(pr.review_decision, Some(ReviewDecision::ReviewRequired));
    assert_eq!(pr.checks, CheckRollup { passed: 10, failed: 1, pending: 0, skipped: 14 });
    assert_eq!(pr.checks.total(), 25);
}

/// Commit statuses count too, and an empty review decision means none is asked for.
#[test]
fn commit_statuses_count_and_an_empty_decision_is_none() {
    let pr = parse_pull_request(PR_STATUSES).unwrap();
    assert_eq!(pr.checks, CheckRollup { passed: 2, pending: 19, ..CheckRollup::default() });
    assert_eq!(pr.checks.total(), 21);
    assert_eq!(pr.review_decision, None);
    assert!(pr.cross_repository);
}

/// Closed without merging, after a reviewer asked for changes.
#[test]
fn a_closed_pull_request_with_changes_requested_is_read_as_such() {
    let pr = parse_pull_request(PR_CLOSED).unwrap();
    assert_eq!(pr.state, PullRequestState::Closed);
    assert_eq!(pr.review_decision, Some(ReviewDecision::ChangesRequested));
}

/// An app is a bot, however `gh` spells its login.
#[test]
fn an_app_author_is_a_bot() {
    let pr = parse_pull_request(PR_BOT).unwrap();
    assert_eq!(pr.author, Author { login: "app/dependabot".into(), bot: true });
}

/// Check outcomes GitHub documents but the fixtures above do not happen to contain.
#[test]
fn every_documented_check_outcome_lands_in_a_bucket() {
    let run = |status: &str, conclusion: &str| CheckEntry::CheckRun {
        status: status.into(),
        conclusion: conclusion.into(),
    };
    let status = |state: &str| CheckEntry::StatusContext { state: state.into() };
    let rollup = roll_up(&[
        run("IN_PROGRESS", ""),
        run("QUEUED", ""),
        run("COMPLETED", "NEUTRAL"),
        run("COMPLETED", "TIMED_OUT"),
        run("COMPLETED", "CANCELLED"),
        run("COMPLETED", "ACTION_REQUIRED"),
        run("COMPLETED", "STARTUP_FAILURE"),
        run("COMPLETED", "STALE"),
        run("COMPLETED", "SOMETHING_NEW"),
        status("EXPECTED"),
        status("ERROR"),
        status("FAILURE"),
        status("SOMETHING_NEW"),
        CheckEntry::Other,
    ]);
    assert_eq!(rollup, CheckRollup { passed: 0, failed: 7, pending: 3, skipped: 1 });
}

#[test]
fn an_issue_is_read_from_issue_view() {
    let issue = parse_issue(ISSUE).unwrap();
    assert_eq!(issue.number, 14404);
    assert!(issue.title.starts_with("`gh pr merge NNN --delete-branch`"), "{}", issue.title);
    assert!(issue.url.ends_with("/issues/14404"));
    assert_eq!(issue.author, "dweomer");
    assert_eq!((issue.comments.len(), issue.total_comments), (4, 4));
    assert!(issue.comments.iter().all(|comment| !comment.author.is_empty()));
}

/// Only the latest few comments are kept, and none GitHub hid as spam or abuse.
#[test]
fn an_issue_keeps_its_latest_visible_comments() {
    let comment = |n: usize, hidden: bool| {
        format!(r#"{{"author":{{"login":"u{n}"}},"body":"c{n}","isMinimized":{hidden}}}"#)
    };
    let comments: Vec<String> = (1..=8).map(|n| comment(n, n == 7)).collect();
    let body = format!(
        r#"{{"number":1,"title":"t","body":"b","url":"u","author":null,"comments":[{}]}}"#,
        comments.join(",")
    );
    let issue = parse_issue(&body).unwrap();
    let kept: Vec<&str> = issue.comments.iter().map(|comment| comment.body.as_str()).collect();
    assert_eq!(kept, ["c3", "c4", "c5", "c6", "c8"]);
    assert_eq!(issue.total_comments, 8);
    assert_eq!(issue.author, "", "a deleted account");
}

pub(super) const PR_HEAD_OWN: &str = include_str!("../../fixtures/pr_head_own.json");
pub(super) const PR_HEAD_FORK: &str = include_str!("../../fixtures/pr_head_fork.json");
pub(super) const PR_HEAD_FORK_NO_EDITS: &str =
    include_str!("../../fixtures/pr_head_fork_no_edits.json");
pub(super) const PR_HEAD_FORK_DEFAULT: &str =
    include_str!("../../fixtures/pr_head_fork_default_branch.json");
/// Real answers: your own branch, a fork (renamed) that allows maintainer edits, one that does
/// not, and one from the fork's own default branch.
#[test]
fn a_pull_requests_head_says_where_it_lives_and_who_can_push() {
    let own = parse_pull_request_head(PR_HEAD_OWN).unwrap();
    assert_eq!(
        (own.head_ref.as_str(), own.local_branch.as_str(), own.cross_repository),
        ("feat/issue-to-pull-request", "feat/issue-to-pull-request", false)
    );
    assert_eq!(own.push, PushAccess::Allowed, "ADMIN on the repository");

    let fork = parse_pull_request_head(PR_HEAD_FORK).unwrap();
    assert_eq!((fork.owner.as_str(), fork.name.as_str()), ("jarrensj", "gh-cli"));
    assert_eq!(fork.local_branch, "remove-claude-md");
    assert_eq!(fork.push, PushAccess::NoWriteAccess, "READ on both sides");

    let off = parse_pull_request_head(PR_HEAD_FORK_NO_EDITS).unwrap();
    assert_eq!(off.push, PushAccess::NoWriteAccess);

    let default = parse_pull_request_head(PR_HEAD_FORK_DEFAULT).unwrap();
    assert_eq!(default.head_ref, "trunk");
    assert_eq!(default.local_branch, "HIHACK1911/trunk", "not in place of your own trunk");
}

/// Every refusal says why, and names the fork when there is one.
#[test]
fn a_refusal_says_why_a_fix_could_not_reach_the_pull_request() {
    let fork = parse_pull_request_head(PR_HEAD_FORK).unwrap();
    let said = |push| PullRequestHead { push, ..fork.clone() }.refusal(14474);
    assert_eq!(said(PushAccess::Allowed), None);
    assert!(said(PushAccess::NoWriteAccess).unwrap().contains("comes from jarrensj/gh-cli"));
    assert!(said(PushAccess::MaintainerEditsOff).unwrap().contains("did not allow edits"));
    assert!(said(PushAccess::HeadRepositoryGone).unwrap().contains("was deleted"));
    let own = PullRequestHead { cross_repository: false, push: PushAccess::NoWriteAccess, ..fork };
    assert!(own.refusal(7).unwrap().starts_with("This account cannot push to this repository"));
}

/// The same answers from a maintainer's side: a maintainer pushes to a fork only when its
/// author allowed edits, and a deleted fork has nowhere to push.
#[test]
fn a_maintainer_pushes_to_a_fork_only_when_its_author_allowed_it() {
    let as_maintainer = |body: &str| {
        body.replacen("\"viewerPermission\":\"READ\"", "\"viewerPermission\":\"MAINTAIN\"", 1)
    };
    assert_eq!(
        parse_pull_request_head(&as_maintainer(PR_HEAD_FORK)).unwrap().push,
        PushAccess::Allowed
    );
    assert_eq!(
        parse_pull_request_head(&as_maintainer(PR_HEAD_FORK_NO_EDITS)).unwrap().push,
        PushAccess::MaintainerEditsOff
    );
    let gone = as_maintainer(PR_HEAD_FORK).replace(
        "\"headRepository\":{\"name\":\"gh-cli\",\"viewerPermission\":\"READ\"}",
        "\"headRepository\":null",
    );
    assert_eq!(parse_pull_request_head(&gone).unwrap().push, PushAccess::HeadRepositoryGone);
    // Your own fork, written to directly
    let yours = PR_HEAD_FORK.replace(
        "{\"name\":\"gh-cli\",\"viewerPermission\":\"READ\"}",
        "{\"name\":\"gh-cli\",\"viewerPermission\":\"ADMIN\"}",
    );
    assert_eq!(parse_pull_request_head(&yours).unwrap().push, PushAccess::Allowed);
    for permission in ["WRITE", "MAINTAIN", "ADMIN"] {
        assert!(can_write(Some(permission)), "{permission}");
    }
    assert!(!can_write(Some("TRIAGE")) && !can_write(Some("READ")) && !can_write(None));
}
