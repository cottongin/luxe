use super::an_hour_after;
use crate::render::github::{header, overview};
use luxe_core::github::overview::Overview;
use serde_json::{json, Value};

/// A `github_overview_read` frame's overview, copied from a live session, its long dependabot
/// body cut to its first line.
const PULL_REQUEST: &str = r#"{"kind": "pull_request", "overview": {"number": 235, "node_id": "PR_kwDOTyoMn88AAAABHrSs7g", "title": "ci: bump the actions group across 1 directory with 2 updates", "body": "Bumps the actions group with 2 updates.", "url": "https://github.com/nora-quinn/luxe-dev/pull/235", "repository": "nora-quinn/luxe-dev", "author": {"login": "dependabot", "bot": true}, "state": "open", "draft": false, "base_ref": "master", "head_ref": "dependabot/github_actions/actions-9d10962cbb", "head_oid": "60e64cfa2f3a722e189a82e06dc3be3808b548a5", "cross_repository": false, "additions": 8, "deletions": 8, "changed_files": 4, "commits": 1, "updated_at": 1792259399, "labels": [{"name": "dependencies", "color": "0366d6"}, {"name": "github_actions", "color": "000000"}], "merge_status": "unknown", "review_decision": null, "reviewers": [], "checks": {"passed": 8, "failed": 0, "pending": 0, "skipped": 0}, "failing": [], "unresolved_threads": 0, "stack": null, "viewer": {"author": false, "can_push": true, "can_merge": true}, "merge_methods": ["merge", "squash", "rebase"], "merge_settings": {"last_method": "merge", "deletes_branch": false, "can_auto_merge": false, "can_cancel_auto_merge": false, "auto_merge": null}, "conversation_count": 1}}"#;

/// An issue's overview in core's shape; the repository the session ran in has no issues to copy.
const ISSUE: &str = r#"{"kind": "issue", "overview": {"number": 41, "title": "Crash on start", "body": "It crashes.", "url": "https://github.com/o/r/issues/41", "repository": "o/r", "author": {"login": "mara", "bot": false}, "state": "open", "updated_at": 1792259399, "labels": [], "assignees": [], "milestone": null, "linked": [], "conversation_count": 0}}"#;

const CHANGED: u64 = 1_792_259_399;

fn value(json: &str) -> Value {
    serde_json::from_str(json).expect("an overview as core sends it")
}

fn parsed(value: Value) -> Overview {
    serde_json::from_value(value).expect("an overview as core sends it")
}

fn headed(value: Value) -> String {
    header(&an_hour_after(CHANGED), &parsed(value))
}

fn sections(value: Value) -> String {
    overview(&an_hour_after(CHANGED), &parsed(value))
}

/// The break this catches: the number, title, author, branches, size or age left out of the
/// header, a bot named as a person, or the state chip in the wrong color.
#[test]
fn a_pull_requests_header_reads_as_the_tuis() {
    let html = headed(value(PULL_REQUEST));
    assert!(html.contains("<span class=\"dim\">#235 </span>ci: bump the actions group"), "{html}");
    let bot = "<span class=\"hue-muted\">dependabot</span><span class=\"dim\"> bot</span>";
    assert!(html.contains(bot), "{html}");
    let branches = "master <span class=\"hue-muted\">\u{2190}</span> dependabot/github_actions";
    assert!(html.contains(branches), "{html}");
    assert!(html.contains("<span class=\"add\">+8</span>"), "{html}");
    assert!(html.contains("in 4 files</span>"), "{html}");
    assert!(html.contains("<span class=\"dim\">3600s ago</span>"), "{html}");
    assert!(html.contains("<span class=\"gh-state hue-success\">open</span>"), "{html}");
    assert!(html.contains("\u{25cf}</span> dependencies</span>"), "{html}");
}

/// The break this catches: "1 files", or a header that shows every label however many there are.
#[test]
fn the_header_counts_one_file_and_caps_its_labels() {
    let mut pull_request = value(PULL_REQUEST);
    pull_request["overview"]["changed_files"] = json!(1);
    let labels: Vec<Value> =
        (1..=5).map(|n| json!({"name": format!("l{n}"), "color": "000000"})).collect();
    pull_request["overview"]["labels"] = json!(labels);
    let html = headed(pull_request);
    assert!(html.contains("in 1 file</span>"), "{html}");
    assert!(html.contains("> l4</span>"), "{html}");
    assert!(!html.contains("> l5</span>"), "{html}");
}

/// The break this catches: an issue's header drawn with a pull request's branches, or its closed
/// state in the pull request's red.
#[test]
fn an_issues_header_has_no_branches() {
    let mut issue = value(ISSUE);
    issue["overview"]["state"] = json!("closed");
    let html = headed(issue);
    assert!(html.contains("<span class=\"dim\">#41 </span>Crash on start"), "{html}");
    assert!(!html.contains('\u{2190}'), "{html}");
    assert!(html.contains("<span class=\"gh-state hue-secondary\">closed</span>"), "{html}");
}

/// The break this catches: a blocker drawn with a passing mark or quiet, the count of what's
/// incomplete dropped, or the stack's "open #N" (the one action that only reads) missing.
#[test]
fn the_status_marks_what_blocks_a_merge() {
    let mut draft = value(PULL_REQUEST);
    draft["overview"]["draft"] = json!(true);
    draft["overview"]["stack"] = json!({"trunk": "master", "trunk_is_default": true, "above": [],
        "below": [{"number": 230, "title": "base", "head_ref": "base", "draft": false, "checked": true}]});
    let html = sections(draft);
    assert!(
        html.contains("<h3 class=\"gh-section\">Status<span class=\"gh-right dim\">"),
        "{html}"
    );
    assert!(html.contains("incomplete</span></h3>"), "{html}");
    let blocker = "<span class=\"gh-mark hue-error\">N</span><span class=\"gh-label\">Draft</span>\
                   <span class=\"gh-evidence\"><span class=\"\">still a draft</span>";
    assert!(html.contains(blocker), "{html}");
    let open = "<button type=\"button\" class=\"gh-open\" data-number=\"230\" data-pull=\"true\">\
                open #230</button>";
    assert!(html.contains(open), "{html}");
}

/// The break this catches: a passing check drawn as a blocker, or "0 incomplete" on a pull
/// request with nothing left to do.
#[test]
fn a_passing_check_is_quiet() {
    let mut ready = value(PULL_REQUEST);
    ready["overview"]["merge_status"] = json!("clean");
    let html = sections(ready);
    assert!(html
        .contains("<span class=\"gh-mark hue-success\">Y</span><span class=\"gh-label\">Checks"));
    assert!(html.contains("<span class=\"hue-muted\">"), "{html}");
    assert!(!html.contains("incomplete"), "{html}");
}

/// The break this catches: the approval count wrong, a verdict in the wrong color, a team not
/// named as one, or a Reviewers heading over nobody.
#[test]
fn reviewers_say_what_they_said() {
    let mut reviewed = value(PULL_REQUEST);
    reviewed["overview"]["reviewers"] = json!([
        {"login": "mara", "kind": "user", "verdict": "approved", "at": CHANGED},
        {"login": "core", "kind": "team", "verdict": "changes_requested", "at": null}
    ]);
    let html = sections(reviewed);
    assert!(html.contains(">1 of 2 approved</span></h3>"), "{html}");
    assert!(html.contains("<span class=\"gh-label\">mara</span>"), "{html}");
    assert!(html.contains("<span class=\"hue-success\">approved</span>"), "{html}");
    assert!(html.contains("\u{b7} 3600s ago"), "{html}");
    assert!(html.contains(
        "<span class=\"hue-error\">changes requested</span><span class=\"dim\"> \u{b7} team</span>"
    ));
    assert!(!sections(value(PULL_REQUEST)).contains("Reviewers"));
}

/// The break this catches: the excerpt quoting more or fewer than five lines that say something,
/// a blank line spending one of the five, the count of the rest wrong, or a body rendered without
/// its HTML made readable first.
#[test]
fn the_description_quotes_five_lines() {
    let body = |text: &str| {
        let mut pull_request = value(PULL_REQUEST);
        pull_request["overview"]["body"] = json!(text);
        sections(pull_request)
    };
    let long = body("one\n\ntwo\nthree\nfour\nfive\nsix\nseven");
    assert!(long.contains("<md>R:one\n\ntwo\nthree\nfour\nfive\n</md>"), "{long}");
    let more = "data-tab=\"conversation\">… 2 more lines in Conversation</button>";
    assert!(long.contains(more), "{long}");
    let short = body("one\ntwo\nthree\nfour\nfive");
    assert!(short.contains("five\n</md>") && !short.contains("gh-more"), "{short}");
    assert!(body("  ").contains("<p class=\"gh-note dim\">No description</p>"));
}

/// The break this catches: an issue's assignees, closing pull requests or milestone left out,
/// a pull request that only mentions the issue listed as closing it, or nothing said when nobody
/// is on it.
#[test]
fn an_issue_says_who_is_on_it() {
    let html = sections(value(ISSUE));
    assert!(html.contains("<span class=\"hue-muted\">unassigned</span>"), "{html}");
    assert!(html.contains("<span class=\"hue-muted\">none yet</span>"), "{html}");
    assert!(!html.contains("Milestone"), "{html}");
    let mut staffed = value(ISSUE);
    staffed["overview"]["assignees"] = json!(["mara", "jo"]);
    staffed["overview"]["milestone"] = json!("v1");
    staffed["overview"]["linked"] = json!([
        {"number": 7, "title": "Fix", "state": "open", "draft": false, "author": "jo", "closes": true},
        {"number": 8, "title": "See", "state": "open", "draft": false, "author": "jo", "closes": false}
    ]);
    let html = sections(staffed);
    assert!(html.contains("<span class=\"hue-secondary\">mara, jo</span>"), "{html}");
    assert!(html.contains("data-number=\"7\" data-pull=\"true\">#7</button>"), "{html}");
    assert!(!html.contains("data-number=\"8\""), "{html}");
    assert!(
        html.contains("<span class=\"gh-label\">Milestone</span><span class=\"gh-evidence\">v1")
    );
}
