use super::an_hour_after;
use crate::render::github::conversation;
use luxe_core::github::overview::ConversationEntry;
use serde_json::{json, Value};

/// A `github_conversation_read` frame's opening entry, copied from a live session, its long
/// dependabot body cut to its first line.
const OPENING: &str = r#"{"kind": "opening", "author": {"login": "dependabot", "bot": true}, "body": "Bumps the actions group with 2 updates in the / directory.", "at": 1792259399}"#;

const CHANGED: u64 = 1_792_259_399;

/// When `CHANGED` was, as GitHub writes a comment's time.
const CHANGED_TEXT: &str = "2026-10-17T17:49:59Z";

fn drawn(entries: Vec<Value>, opened_by_you: bool) -> String {
    let entries: Vec<ConversationEntry> =
        serde_json::from_value(json!(entries)).expect("entries as core sends them");
    conversation(&an_hour_after(CHANGED), &entries, opened_by_you)
}

fn comment(author: &str, bot: bool, mine: bool, body: &str) -> Value {
    json!({"id": "IC_1", "author": author, "bot": bot, "mine": mine, "body": body,
        "created_at": CHANGED_TEXT, "url": "https://github.com/o/r/pull/1#issuecomment-1"})
}

/// A comment as the timeline's own entry.
fn said(author: &str, bot: bool, mine: bool, body: &str) -> Value {
    let mut entry = comment(author, bot, mine, body);
    entry["kind"] = json!("comment");
    entry
}

fn review(verdict: &str, line_comments: usize, bot: bool) -> Value {
    let comment = comment("mara", bot, false, "Looks fine.");
    json!({"kind": "review", "comment": comment, "verdict": verdict, "line_comments": line_comments})
}

fn thread(path: &str, resolved: bool, outdated: bool, said: &[(&str, &str)]) -> Value {
    let comments: Vec<Value> = said
        .iter()
        .map(|(author, body)| {
            json!({"author": author,
        "body": body, "created_at": CHANGED_TEXT, "url": "https://github.com/o/r/pull/1#r1"})
        })
        .collect();
    json!({"kind": "thread", "id": "PRRT_1", "path": path, "line": 42, "start_line": null,
        "side": "right", "outdated": outdated, "resolved": resolved, "file_level": false,
        "comments": comments, "total_comments": said.len(), "anchor_text": null,
        "can_reply": false, "can_resolve": false})
}

/// The break this catches: the real opening entry drawn as a person's, without its age, or with
/// its body rendered before its HTML was made readable.
#[test]
fn the_opening_reads_as_the_tuis() {
    let opening: Value = serde_json::from_str(OPENING).expect("an opening as core sends it");
    let html = drawn(vec![opening], false);
    assert!(html.contains("<section class=\"gh-card tone-quiet\">"), "{html}");
    let bot = "<span class=\"hue-muted\">@dependabot</span><span class=\"dim\"> bot</span>";
    assert!(html.contains(bot), "{html}");
    assert!(html.contains(">opened</span>"), "{html}");
    assert!(html.contains(">3600s ago</span>"), "{html}");
    assert!(html.contains("<md>R:Bumps the actions group"), "{html}");
}

/// The break this catches: your own description or comment not named as yours, a person's
/// comment drawn quiet, or a comment's time dropped.
#[test]
fn a_comment_names_its_speaker() {
    let opening = json!({"kind": "opening", "author": {"login": "me", "bot": false}, "body": "x", "at": CHANGED});
    let html = drawn(vec![opening], true);
    assert!(html.contains("<span class=\"gh-mine\">@me</span>"), "{html}");
    let html = drawn(vec![said("jo", false, false, "Hi.")], false);
    assert!(html.contains("<section class=\"gh-card tone-person\">"), "{html}");
    assert!(html.contains("<span class=\"hue-secondary\">@jo</span>"), "{html}");
    assert!(html.contains(">commented</span>"), "{html}");
    assert!(html.contains(">3600s ago</span>"), "{html}");
    let html = drawn(vec![said("me", false, true, "Hi.")], false);
    assert!(html.contains("<span class=\"gh-mine\">@me</span>"), "{html}");
    let html = drawn(vec![said("", false, false, "Hi.")], false);
    assert!(html.contains("<span class=\"hue-muted\">@ghost</span>"), "{html}");
}

/// The break this catches: a verdict in the wrong words or tone, a bot's review toned like a
/// person's, or the line comments miscounted.
#[test]
fn a_review_is_toned_by_its_verdict() {
    let html = drawn(vec![review("approved", 0, false)], false);
    assert!(html.contains("tone-approved"), "{html}");
    assert!(html.contains("<span class=\"hue-success\">approved</span>"), "{html}");
    assert!(!html.contains("line comment"), "{html}");
    let html = drawn(vec![review("changes_requested", 1, false)], false);
    assert!(html.contains("tone-changes"), "{html}");
    assert!(html.contains("<span class=\"hue-error\">requested changes</span>"), "{html}");
    assert!(html.contains(">1 line comment</span>"), "{html}");
    let html = drawn(vec![review("changes_requested", 3, true)], false);
    assert!(html.contains("tone-quiet"), "{html}");
    assert!(html.contains(">3 line comments</span>"), "{html}");
}

/// The break this catches: a long comment drawn whole, or a short one folded behind a button.
#[test]
fn a_long_comment_folds() {
    let long = (1..=8).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
    let html = drawn(vec![said("jo", false, false, &long)], false);
    assert!(html.contains("tone-person folded"), "{html}");
    assert!(html.contains("<button type=\"button\" class=\"gh-unfold\">… Show all</button>"));
    let short = (1..=7).map(|n| format!("line {n}\n")).collect::<String>();
    let html = drawn(vec![said("jo", false, false, &short)], false);
    assert!(!html.contains("folded") && !html.contains("gh-unfold"), "{html}");
}

/// The break this catches: an open thread not in the info tone, its place or reply count wrong,
/// its last word missing while folded, or a reply left out once it's opened.
#[test]
fn an_open_thread_shows_its_last_word() {
    let said = [("mara", "Why\nthis?"), ("jo", "Because.")];
    let html = drawn(vec![thread("src/api/v1/hook.rs", false, false, &said)], false);
    assert!(html.contains("<details class=\"gh-card gh-thread tone-unresolved\">"), "{html}");
    assert!(html.contains("<span class=\"hue-secondary\">@mara</span>"), "{html}");
    assert!(html.contains("<span class=\"dim\"> \u{b7} </span>\u{2026}/v1/hook.rs:42"), "{html}");
    assert!(html.contains("<span class=\"hue-info\">unresolved</span>"), "{html}");
    assert!(html.contains(">1 reply</span>"), "{html}");
    let last = "<span class=\"gh-last\"><span class=\"hue-muted\">@jo: </span>Because.</span>";
    assert!(html.contains(last), "{html}");
    assert!(html.contains("<md>R:Why\nthis?</md>"), "{html}");
}

/// The break this catches: a settled thread still toned as open, or a short path cut.
#[test]
fn a_settled_thread_is_quiet() {
    let said = [("mara", "Fixed.")];
    let html = drawn(vec![thread("crates/x/hook.rs", true, false, &said)], false);
    assert!(html.contains("tone-person"), "{html}");
    assert!(html.contains("</span>crates/x/hook.rs:42"), "{html}");
    assert!(html.contains("<span class=\"dim\">resolved</span>"), "{html}");
    assert!(!html.contains(" reply<"), "{html}");
    let html = drawn(vec![thread("src/hook.rs", false, true, &said)], false);
    assert!(html.contains("<span class=\"dim\">outdated</span>"), "{html}");
    assert!(html.contains("tone-person"), "{html}");
}
