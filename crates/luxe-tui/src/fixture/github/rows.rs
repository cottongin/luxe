//! GitHub panel rows — the search results each tab lists, their facts, labels and timestamps.

use luxe_core::github::overview::{Label, RowFacts};
use luxe_core::github::{CheckRollup, ListItem, ListedSection, PullRequestState, ReviewDecision};

/// The fixture's now: 2026-09-24, mid-afternoon.
pub(super) const NOW: u64 = 1_790_265_600;

pub(super) const MINUTE: u64 = 60;

pub(super) const HOUR: u64 = 60 * MINUTE;

pub(super) const DAY: u64 = 24 * HOUR;

pub(super) fn url(number: u64) -> String {
    format!("https://github.com/acme/notify/pull/{number}")
}

pub(super) fn issue_url(number: u64) -> String {
    format!("https://github.com/acme/notify/issues/{number}")
}

fn row(pull_request: bool, number: u64, title: &str, author: &str, ago: u64) -> ListItem {
    ListItem {
        pull_request,
        number,
        title: title.into(),
        url: if pull_request { url(number) } else { issue_url(number) },
        repository: "acme/notify".into(),
        author: author.into(),
        draft: number == 139,
        updated_at: stamp(NOW - ago),
        state: PullRequestState::Open,
        facts: None,
    }
}

/// A section as a search brings it: whole, or the first page of `total`.
pub(super) fn listed(name: &str, items: Vec<ListItem>, total: Option<u64>) -> ListedSection {
    let total = total.unwrap_or(items.len() as u64);
    let next = (total > items.len() as u64).then(|| "Y3Vyc29yOjMw".to_string());
    ListedSection { name: name.into(), items, error: None, total: Some(total), next }
}

pub(super) fn review_requested() -> Vec<ListItem> {
    vec![
        row(true, 182, "Parse notification categories into an enum", "copilot-swe-agent", 2 * HOUR),
        row(true, 176, "Retry webhook deliveries with backoff", "mara-okafor", 5 * HOUR),
        row(true, 171, "Drop the legacy digest formatter", "jon-avery", DAY),
    ]
}

pub(super) fn mine() -> Vec<ListItem> {
    vec![
        row(true, 142, "Route digests by category", "nora-quinn", 25 * MINUTE),
        row(true, 139, "Batch digest sends per tenant", "nora-quinn", 3 * DAY),
    ]
}

pub(super) fn issues() -> Vec<ListItem> {
    vec![
        row(false, 318, "A typo in a webhook creates a phantom category", "mara-okafor", 2 * HOUR),
        row(false, 305, "Digest emails ignore the reader's time zone", "kai-holm", DAY),
        row(false, 297, "Document the notification categories", "jon-avery", 4 * DAY),
        row(false, 290, "Rate-limit outbound webhooks per tenant", "lena-hart", 9 * DAY),
    ]
}

/// Every open pull request, most recently active first: yours, the ones waiting on you, and the
/// ones nobody addressed to you, a bot's among them.
pub(super) fn pull_requests() -> Vec<ListItem> {
    let mut rows = review_requested();
    rows.extend(mine());
    rows.push(row(true, 185, "Bump tokio from 1.40 to 1.41", "dependabot", 40 * MINUTE));
    rows.push(row(true, 168, "Expose digest metrics on /metrics", "kai-holm", 2 * DAY));
    rows.sort_by_key(|row| std::cmp::Reverse(row.updated_at.clone()));
    rows
}

pub(super) fn all_issues() -> Vec<ListItem> {
    let mut rows = issues();
    rows.push(row(false, 312, "Unsubscribe links 404 for SSO tenants", "kai-holm", 6 * HOUR));
    rows.push(row(false, 284, "Webhook signatures accept stale timestamps", "lena-hart", 12 * DAY));
    rows.sort_by_key(|row| std::cmp::Reverse(row.updated_at.clone()));
    rows
}

pub(super) fn merged() -> Vec<ListItem> {
    let merged = |number, title, author, ago| ListItem {
        state: PullRequestState::Merged,
        ..row(true, number, title, author, ago)
    };
    vec![
        merged(181, "Parse the category header case-insensitively", "mara-okafor", 3 * HOUR),
        merged(174, "Split the dispatcher into its own module", "jon-avery", DAY),
        merged(170, "Bump serde from 1.0.210 to 1.0.214", "dependabot", 2 * DAY),
    ]
}

pub(super) fn label(name: &str, color: &str) -> Label {
    Label { name: name.into(), color: color.into() }
}

/// What each row says under its title, by address.
pub(super) fn facts() -> Vec<(String, RowFacts)> {
    let checks = |passed, failed, pending| CheckRollup { passed, failed, pending, skipped: 0 };
    let pr = |additions, deletions, checks, review_decision, bot| RowFacts {
        author_bot: bot,
        additions,
        deletions,
        checks,
        review_decision,
        ..RowFacts::default()
    };
    let issue = |labels: Vec<Label>, comments, linked_pull_request| RowFacts {
        labels,
        comments,
        linked_pull_request,
        ..RowFacts::default()
    };
    let bug = || label("bug", "d73a4a");
    vec![
        (url(182), pr(50, 12, checks(11, 1, 0), Some(ReviewDecision::ChangesRequested), true)),
        (url(176), pr(212, 40, checks(6, 0, 3), Some(ReviewDecision::ReviewRequired), false)),
        (url(171), pr(8, 164, checks(12, 0, 0), Some(ReviewDecision::Approved), false)),
        (url(142), pr(96, 31, checks(11, 1, 0), Some(ReviewDecision::ChangesRequested), false)),
        (url(139), pr(140, 22, checks(4, 0, 8), None, false)),
        (issue_url(318), issue(vec![bug(), label("webhooks", "0e8a16")], 4, Some(182))),
        (issue_url(305), issue(vec![bug()], 3, None)),
        (issue_url(297), issue(vec![label("docs", "0075ca")], 0, None)),
        (issue_url(290), issue(vec![label("enhancement", "a2eeef")], 1, None)),
        (url(185), pr(4, 4, checks(12, 0, 0), None, true)),
        (url(168), pr(88, 6, checks(10, 0, 2), Some(ReviewDecision::ReviewRequired), false)),
        (url(181), pr(14, 3, checks(12, 0, 0), Some(ReviewDecision::Approved), false)),
        (url(174), pr(310, 288, checks(12, 0, 0), Some(ReviewDecision::Approved), false)),
        (url(170), pr(2, 2, checks(12, 0, 0), None, true)),
        (issue_url(312), issue(vec![bug(), label("sso", "fbca04")], 2, None)),
        (issue_url(284), issue(vec![label("security", "b60205")], 5, None)),
    ]
}

/// GitHub's `2026-09-24T19:05:02Z` for `seconds` since the Unix epoch.
pub(super) fn stamp(seconds: u64) -> String {
    // Howard Hinnant's `civil_from_days`, the inverse of what `rfc3339_seconds` reads
    let days = (seconds / DAY) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 { shifted + 3 } else { shifted - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    let rest = seconds % DAY;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / HOUR,
        rest % HOUR / MINUTE,
        rest % MINUTE
    )
}
