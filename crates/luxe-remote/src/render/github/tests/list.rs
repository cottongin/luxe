use super::an_hour_after;
use crate::render::github::list;
use luxe_core::github::{rfc3339_seconds, ListedSection};
use serde_json::{json, Value};

/// A `github_listed` item, copied from a live session's "Pull requests" section.
const ITEM: &str = r#"{"pull_request": true, "number": 211, "title": "chore: refresh the embedded models.dev snapshot", "url": "https://github.com/nora-quinn/luxe-dev/pull/211", "repository": "nora-quinn/luxe-dev", "author": "github-actions", "draft": false, "updated_at": "2026-10-19T12:25:08Z", "state": "open", "facts": {"author_bot": true, "additions": 2, "deletions": 2, "checks": {"passed": 0, "failed": 0, "pending": 0, "skipped": 0}, "review_decision": null, "comments": 0, "labels": [], "linked_pull_request": null}}"#;

const HERE: Option<&str> = Some("nora-quinn/luxe-dev");

fn item() -> Value {
    serde_json::from_str(ITEM).expect("a listed item as core sends it")
}

fn section(items: Vec<Value>, total: Option<u64>) -> ListedSection {
    let section = json!({"name": "Pull requests", "items": items, "error": null, "total": total});
    serde_json::from_value(section).expect("a listed section as core sends it")
}

/// When `ITEM` last changed.
fn changed() -> u64 {
    rfc3339_seconds("2026-10-19T12:25:08Z").expect("the item's time reads")
}

/// One section holding `item`, drawn an hour after it changed.
fn drawn(item: Value) -> String {
    list(&an_hour_after(changed()), HERE, &section(vec![item], Some(1)))
}

/// The break this catches: a row the app can't open (number, kind or link missing), the mark in
/// the wrong state color, a bot named as a person, the age keeping its "ago", or the diff counts
/// dropped from line two.
#[test]
fn a_pull_request_row_reads_as_the_tuis() {
    let html = drawn(item());
    assert!(html.contains("data-number=\"211\" data-pull=\"true\""), "{html}");
    assert!(html.contains("data-url=\"https://github.com/nora-quinn/luxe-dev/pull/211\""));
    assert!(html.contains("<span class=\"gh-mark hue-success\">PR</span>"), "{html}");
    assert!(html.contains("<span class=\"dim\">#211 </span>"), "{html}");
    assert!(html.contains("refresh the embedded models.dev snapshot"), "{html}");
    let bot = "<span class=\"hue-muted\">github-actions</span><span class=\"dim\"> bot</span>";
    assert!(html.contains(bot), "{html}");
    assert!(html.contains("<span class=\"gh-right dim\">3600s</span>"), "{html}");
    assert!(html.contains("<span class=\"add\">+2</span>"), "{html}");
    assert!(!html.contains(">open<"), "an open pull request doesn't say so: {html}");
}

/// The break this catches: the checks or the review left out of line two, or their marks crossed.
#[test]
fn a_pull_request_row_says_where_it_stands() {
    let mut failing = item();
    failing["facts"]["checks"] = json!({"passed": 3, "failed": 1, "pending": 2, "skipped": 0});
    failing["facts"]["review_decision"] = json!("changes_requested");
    let html = drawn(failing);
    assert!(html.contains("<span class=\"hue-error\">N 1 failing</span>"), "{html}");
    assert!(html.contains("<span class=\"hue-error\">changes requested</span>"), "{html}");
    let mut running = item();
    running["facts"]["checks"] = json!({"passed": 3, "failed": 0, "pending": 2, "skipped": 0});
    running["facts"]["review_decision"] = json!("approved");
    let html = drawn(running);
    assert!(html.contains("<span class=\"hue-warning\">W running</span>"), "{html}");
    assert!(html.contains("<span class=\"hue-success\">approved</span>"), "{html}");
    let mut passed = item();
    passed["facts"]["checks"] = json!({"passed": 3, "failed": 0, "pending": 0, "skipped": 0});
    assert!(drawn(passed).contains("<span class=\"hue-success\">Y checks</span>"));
}

/// The break this catches: a draft or a merged pull request that doesn't say so, or its mark in
/// the open color.
#[test]
fn a_row_names_any_state_but_open() {
    let mut draft = item();
    draft["draft"] = json!(true);
    let html = drawn(draft);
    assert!(html.contains("<span class=\"gh-mark hue-muted\">PR</span>"), "{html}");
    assert!(html.contains("<span class=\"hue-muted\">draft</span>"), "{html}");
    let mut merged = item();
    merged["state"] = json!("merged");
    assert!(drawn(merged).contains("<span class=\"hue-secondary\">merged</span>"));
}

/// The break this catches: an issue drawn with a pull request's mark or diff counts, its labels
/// left out, or the pull request on it not named.
#[test]
fn an_issue_row_shows_its_labels_and_its_pull_request() {
    let mut issue = item();
    issue["pull_request"] = json!(false);
    issue["facts"]["labels"] = json!([{"name": "bug", "color": "d73a4a"}]);
    issue["facts"]["linked_pull_request"] = json!(9);
    let html = drawn(issue.clone());
    assert!(html.contains("data-pull=\"false\""), "{html}");
    assert!(html.contains(">IS</span>"), "{html}");
    assert!(html.contains("<span class=\"hue-muted\">bug</span>"), "{html}");
    assert!(html.contains("<span class=\"hue-warning\">W PR #9</span>"), "{html}");
    assert!(!html.contains("class=\"add\""), "an issue has no diff: {html}");
    issue["facts"]["linked_pull_request"] = json!(null);
    issue["facts"]["comments"] = json!(1);
    assert!(drawn(issue).contains("<span class=\"hue-muted\">1 comment</span>"));
}

/// The break this catches: a row from another repository that doesn't say which, or one from
/// this checkout that names it anyway.
#[test]
fn a_row_names_its_repository_only_when_it_is_elsewhere() {
    let one = section(vec![item()], Some(1));
    let here = list(&an_hour_after(changed()), HERE, &one);
    assert!(!here.contains("<span class=\"hue-muted\">nora-quinn/luxe-dev</span>"), "{here}");
    let elsewhere = list(&an_hour_after(changed()), Some("someone/else"), &one);
    let named = "<span class=\"hue-muted\">nora-quinn/luxe-dev</span>";
    assert!(elsewhere.contains(named), "{elsewhere}");
}

/// The break this catches: a count that hides how many more GitHub has, says "of" when there
/// are no more, or runs a big number together.
#[test]
fn the_count_says_how_many_of_how_many() {
    let cx = an_hour_after(0);
    let count = |total| list(&cx, HERE, &section(vec![item()], total));
    assert!(count(Some(3250)).contains(">1 of 3,250</p>"), "{}", count(Some(3250)));
    assert!(count(Some(120_000)).contains(">1 of 120,000</p>"), "{}", count(Some(120_000)));
    assert!(count(Some(1)).contains(">1 shown</p>"), "{}", count(Some(1)));
    assert!(count(None).contains(">1 shown</p>"), "{}", count(None));
}

/// The break this catches: an empty tab drawn as a blank, or a failed search's reason lost or
/// let through unescaped.
#[test]
fn an_empty_or_failed_tab_says_so() {
    let cx = an_hour_after(0);
    let empty = list(&cx, HERE, &section(Vec::new(), Some(0)));
    assert_eq!(empty, "<p class=\"gh-note dim\">Nothing matches this search</p>");
    let mut failed = section(Vec::new(), None);
    failed.error = Some("gh <auth> failed".into());
    let html = list(&cx, HERE, &failed);
    assert_eq!(html, "<p class=\"gh-note hue-error\">gh &lt;auth&gt; failed</p>");
}

/// The break this catches: a stranger's title reaching the page as markup.
#[test]
fn a_title_is_escaped() {
    let mut hostile = item();
    hostile["title"] = json!("<img src=x onerror=alert(1)>");
    let html = drawn(hostile);
    assert!(html.contains("&lt;img"), "{html}");
    assert!(!html.contains("<img"), "{html}");
}
