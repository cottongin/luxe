//! GitHub overviews — the pull requests and the issue the panel opens on.

use super::rows::{issue_url, label, url, DAY, HOUR, MINUTE, NOW};
use luxe_core::github::overview::{
    FailingCheckName, IssueOverview, LinkedPullRequest, MergeMethod, MergeStatus,
    PullRequestOverview, Reviewer, ReviewerKind, Verdict, Viewer,
};
use luxe_core::github::{Author, CheckRollup, PullRequestState, ReviewDecision};

pub(super) const DESCRIPTION: &str =
    "Digests went to every subscriber whatever the category, so an \
incident page and a weekly product note landed in the same email.

This routes each digest by the category the webhook now parses:

- `Incident` digests go out at once, to on-call only
- `Deploy` digests batch hourly
- everything else keeps the daily send

Tenants without routing rules fall back to the daily send, so nobody stops getting mail on \
upgrade.

Closes #301.";

pub(super) fn own_pull_request() -> PullRequestOverview {
    PullRequestOverview {
        node_id: String::new(),
        number: 142,
        title: "Route digests by category".into(),
        body: DESCRIPTION.into(),
        url: url(142),
        repository: "acme/notify".into(),
        author: Author { login: "nora-quinn".into(), bot: false },
        state: PullRequestState::Open,
        draft: false,
        base_ref: "main".into(),
        head_ref: "digest-routing".into(),
        head_oid: "4c1e2a0f9b".into(),
        cross_repository: false,
        additions: 96,
        deletions: 31,
        changed_files: 7,
        commits: 6,
        updated_at: NOW - 25 * MINUTE,
        labels: vec![label("backend", "1d76db"), label("notifications", "5319e7")],
        merge_status: MergeStatus::Blocked,
        review_decision: Some(ReviewDecision::ChangesRequested),
        reviewers: vec![
            reviewer("jon-avery", ReviewerKind::User, Verdict::ChangesRequested, 3 * HOUR),
            reviewer("mara-okafor", ReviewerKind::User, Verdict::Approved, DAY),
            reviewer("coderabbitai", ReviewerKind::Bot, Verdict::Commented, DAY),
            reviewer("lena-hart", ReviewerKind::User, Verdict::Requested, DAY),
            reviewer("acme/platform", ReviewerKind::Team, Verdict::Requested, DAY),
        ],
        checks: CheckRollup { passed: 11, failed: 1, ..CheckRollup::default() },
        failing: vec![FailingCheckName { name: "test (ubuntu-latest)".into(), required: true }],
        unresolved_threads: 2,
        stack: None,
        viewer: Viewer { author: true, can_push: true, can_merge: true },
        merge_methods: vec![MergeMethod::Squash, MergeMethod::Merge, MergeMethod::Rebase],
        merge_settings: Default::default(),
        conversation_count: 7,
    }
}

/// The same pull request a day on: fixed, approved, nothing in the way.
pub(super) fn ready_pull_request() -> PullRequestOverview {
    PullRequestOverview {
        merge_status: MergeStatus::Clean,
        review_decision: Some(ReviewDecision::Approved),
        reviewers: vec![
            reviewer("jon-avery", ReviewerKind::User, Verdict::Approved, 40 * MINUTE),
            reviewer("mara-okafor", ReviewerKind::User, Verdict::Approved, DAY),
            reviewer("lena-hart", ReviewerKind::User, Verdict::Approved, 2 * HOUR),
        ],
        checks: CheckRollup { passed: 12, ..CheckRollup::default() },
        failing: Vec::new(),
        unresolved_threads: 0,
        updated_at: NOW - 40 * MINUTE,
        conversation_count: 11,
        ..own_pull_request()
    }
}

fn reviewer(login: &str, kind: ReviewerKind, verdict: Verdict, ago: u64) -> Reviewer {
    Reviewer { login: login.into(), kind, verdict, at: Some(NOW - ago) }
}

const ISSUE_BODY: &str = "A webhook that sends `categroy: \"deploy\"` — the typo is real, from \
the Jenkins plugin — creates a category named after whatever the field held, and the digest \
renders it as an empty heading.

Expected: an unknown category is refused, or at least lands in `Other`.

Seen on three tenants this week.";

pub(super) fn issue() -> IssueOverview {
    IssueOverview {
        number: 318,
        title: "A typo in a webhook creates a phantom category".into(),
        body: ISSUE_BODY.into(),
        url: issue_url(318),
        repository: "acme/notify".into(),
        author: Author { login: "mara-okafor".into(), bot: false },
        state: PullRequestState::Open,
        updated_at: NOW - 2 * HOUR,
        labels: vec![label("bug", "d73a4a"), label("webhooks", "0e8a16")],
        assignees: vec!["nora-quinn".into()],
        milestone: Some("v0.9".into()),
        linked: vec![LinkedPullRequest {
            number: 182,
            title: "Parse notification categories into an enum".into(),
            state: PullRequestState::Open,
            draft: false,
            author: "copilot-swe-agent".into(),
            closes: true,
        }],
        conversation_count: 4,
    }
}
