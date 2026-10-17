use super::super::{FailingCheckName, MergeMethod, Reviewer, ReviewerKind, Viewer};
use super::*;
use crate::github::{Author, CheckRollup, Stack, StackEntry};

fn pr() -> PullRequestOverview {
    PullRequestOverview {
        node_id: String::new(),
        number: 142,
        title: "Route digests by category".into(),
        body: String::new(),
        url: "https://github.com/acme/notify/pull/142".into(),
        repository: "acme/notify".into(),
        author: Author { login: "nora".into(), bot: false },
        state: PullRequestState::Open,
        draft: false,
        base_ref: "main".into(),
        head_ref: "digest-routing".into(),
        head_oid: "4c1e2a0f9b".into(),
        cross_repository: false,
        additions: 10,
        deletions: 2,
        changed_files: 1,
        commits: 1,
        updated_at: 0,
        labels: Vec::new(),
        merge_status: MergeStatus::Clean,
        review_decision: None,
        reviewers: Vec::new(),
        checks: CheckRollup { passed: 4, ..CheckRollup::default() },
        failing: Vec::new(),
        unresolved_threads: 0,
        stack: None,
        viewer: Viewer { author: true, can_push: true, can_merge: true },
        merge_methods: vec![MergeMethod::Squash],
        merge_settings: Default::default(),
        conversation_count: 0,
    }
}

fn reviewer(login: &str, verdict: Verdict) -> Reviewer {
    Reviewer { login: login.into(), kind: ReviewerKind::User, verdict, at: None }
}

fn labels(checks: &[MergeCheck]) -> Vec<(&'static str, CheckMark)> {
    checks.iter().map(|row| (row.label, row.mark)).collect()
}

/// Nothing in the way: the conflicts row reassures, and the last row offers the merge to
/// someone who may merge, and only to them.
#[test]
fn a_clear_pull_request_ends_on_the_merge() {
    let checks = merge_checks(&pr());
    assert_eq!(
        labels(&checks),
        [("Checks", CheckMark::Pass), ("Conflicts", CheckMark::Pass), ("Ready", CheckMark::Pass)]
    );
    assert_eq!(checks[0].detail, "4 passed");
    assert_eq!(checks[1].detail, "none with main");
    assert_eq!(checks[2].action, Some(CheckAction::Merge));
    assert_eq!(checks[2].detail, "squash and merge into main");
    assert_eq!(blocking(&checks), 0);
    let rebase = PullRequestOverview { merge_methods: vec![MergeMethod::Rebase], ..pr() };
    assert_eq!(merge_checks(&rebase)[2].detail, "rebase and merge into main");
    // GitHub's order, whatever order the repository's settings arrive in
    let every = PullRequestOverview {
        merge_methods: vec![MergeMethod::Rebase, MergeMethod::Squash, MergeMethod::Merge],
        ..pr()
    };
    assert_eq!(
        every.merge_options(),
        [MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase]
    );
    assert_eq!(merge_checks(&every)[2].detail, "merge commit into main");
    // The method this account used last, as the confirmation opens on it
    let remembered = PullRequestOverview {
        merge_settings: super::super::MergeSettings {
            last_method: Some(MergeMethod::Rebase),
            ..Default::default()
        },
        ..every.clone()
    };
    assert_eq!(merge_checks(&remembered)[2].detail, "rebase and merge into main");
    let none = PullRequestOverview { merge_methods: Vec::new(), ..pr() };
    assert_eq!(none.merge_options(), [MergeMethod::Merge]);

    let reader = PullRequestOverview { viewer: Viewer::default(), ..pr() };
    assert_eq!(merge_checks(&reader).last().unwrap().action, None);
    // Mergeable by GitHub's word, but a required check never reported: no merge row
    let blocked = PullRequestOverview { merge_status: MergeStatus::Blocked, ..pr() };
    assert!(merge_checks(&blocked).iter().all(|row| row.label != "Ready"));
}

/// Each blocker names its evidence and carries the action that deals with it, for an account
/// that can act; a reader of someone else's pull request gets the facts and no keys.
#[test]
fn each_blocker_carries_its_own_action() {
    let busy = PullRequestOverview {
        draft: true,
        merge_status: MergeStatus::Dirty,
        review_decision: Some(ReviewDecision::ChangesRequested),
        reviewers: vec![
            reviewer("jon", Verdict::ChangesRequested),
            reviewer("mara", Verdict::Approved),
            reviewer("sam", Verdict::Requested),
        ],
        checks: CheckRollup { passed: 11, failed: 3, ..CheckRollup::default() },
        failing: vec![
            FailingCheckName { name: "test (ubuntu)".into(), required: true },
            FailingCheckName { name: "lint".into(), required: false },
        ],
        unresolved_threads: 2,
        ..pr()
    };
    let checks = merge_checks(&busy);
    assert_eq!(
        labels(&checks),
        [
            ("Draft", CheckMark::Fail),
            ("Checks", CheckMark::Fail),
            ("Review", CheckMark::Fail),
            ("Reviewers", CheckMark::Wait),
            ("Conflicts", CheckMark::Fail),
        ]
    );
    assert_eq!(checks[1].detail, "3 of 14 failing: test (ubuntu), lint, …");
    assert_eq!(checks[2].detail, "changes requested by jon · 2 threads open");
    assert_eq!(checks[3].detail, "awaiting sam");
    assert_eq!(checks[4].detail, "conflicts with main");
    let actions: Vec<_> = checks.iter().map(|row| row.action).collect();
    assert_eq!(
        actions,
        [
            Some(CheckAction::MarkReady),
            Some(CheckAction::FixChecks),
            Some(CheckAction::ResolveThreads),
            None,
            Some(CheckAction::ResolveConflicts),
        ]
    );
    assert_eq!(blocking(&checks), 5);

    let theirs = PullRequestOverview { viewer: Viewer::default(), ..busy };
    assert!(merge_checks(&theirs).iter().all(|row| row.action.is_none()));
}

#[test]
fn running_checks_and_a_stale_head_are_in_the_way() {
    let running = PullRequestOverview {
        checks: CheckRollup { passed: 9, pending: 3, ..CheckRollup::default() },
        merge_status: MergeStatus::Behind,
        ..pr()
    };
    let checks = merge_checks(&running);
    assert_eq!(checks[0].detail, "3 of 12 running");
    assert_eq!(checks[0].mark, CheckMark::Wait);
    assert_eq!(
        (checks[1].label, checks[1].detail.as_str(), checks[1].action),
        ("Up to date", "behind main", Some(CheckAction::UpdateBranch))
    );
    assert!(checks.iter().all(|row| row.label != "Ready"));
    let unknown = PullRequestOverview { merge_status: MergeStatus::Unknown, ..pr() };
    let checks = merge_checks(&unknown);
    assert_eq!(checks[1].detail, "checking");
    assert!(checks.iter().all(|row| row.label != "Ready"), "no merge on a guess");
}

/// An approval with threads still open waits on its author; review required waits on someone
/// else, so it offers nothing.
#[test]
fn the_review_row_says_who_and_what_is_left() {
    let approved = PullRequestOverview {
        review_decision: Some(ReviewDecision::Approved),
        reviewers: vec![reviewer("mara", Verdict::Approved), reviewer("li", Verdict::Approved)],
        ..pr()
    };
    let row = review_row(&approved).unwrap();
    assert_eq!((row.mark, row.detail.as_str()), (CheckMark::Pass, "approved by mara and 1 more"));
    let open = PullRequestOverview { unresolved_threads: 1, ..approved };
    let row = review_row(&open).unwrap();
    assert_eq!(row.detail, "approved by mara and 1 more · 1 thread open");
    assert_eq!((row.mark, row.action), (CheckMark::Wait, Some(CheckAction::ResolveThreads)));
    let required =
        PullRequestOverview { review_decision: Some(ReviewDecision::ReviewRequired), ..pr() };
    let row = review_row(&required).unwrap();
    assert_eq!((row.detail.as_str(), row.action), ("approval required", None));
    assert!(review_row(&pr()).is_none(), "no review asked for, no threads: no row");
}

#[test]
fn a_stacked_pull_request_waits_on_the_one_below() {
    let entry = |number| StackEntry {
        number,
        title: String::new(),
        head_ref: format!("b{number}"),
        draft: false,
        checked: true,
    };
    let stacked = PullRequestOverview {
        checks: CheckRollup::default(),
        stack: Some(Stack {
            trunk: "main".into(),
            trunk_is_default: true,
            below: vec![entry(140), entry(141)],
            above: Vec::new(),
        }),
        ..pr()
    };
    let checks = merge_checks(&stacked);
    assert_eq!(checks[0].action, Some(CheckAction::OpenBelow(141)), "the one directly below");
    assert_eq!(checks[1].detail, "none until it targets main");
    assert_eq!(checks[1].mark, CheckMark::Wait);
}

#[test]
fn a_finished_pull_request_has_nothing_in_its_way() {
    for state in [PullRequestState::Merged, PullRequestState::Closed] {
        assert!(merge_checks(&PullRequestOverview { state, ..pr() }).is_empty());
    }
}
