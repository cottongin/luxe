#![allow(clippy::unwrap_used)]

use super::*;
use luxe_core::ContentBlock;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

fn context(cwd: &std::path::Path, session_id: &str) -> ToolContext {
    let (event_tx, _event_rx) = mpsc::channel(64);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    ToolContext {
        cwd: cwd.to_path_buf(),
        session_id: session_id.to_string(),
        cancelled: Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    }
}

/// The tool, its store root, and a context on one session.
fn tool(tmp: &tempfile::TempDir) -> (TodoTool, ToolContext) {
    (TodoTool::new(tmp.path().join("todos")), context(tmp.path(), "test-session"))
}

fn text(result: &ToolResultData) -> String {
    match &result.content[0] {
        ContentBlock::Text(text) => text.clone(),
        _ => panic!("expected a text block"),
    }
}

#[tokio::test]
async fn a_fresh_session_lists_nothing_and_is_not_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let (tool, cx) = tool(&tmp);

    let result = tool.run(json!({"action": "list"}), &cx).await;

    assert!(!result.is_error);
    assert_eq!(text(&result), "No todos");
}

/// The list is the whole point of the durable store: it has to come back after a restart.
#[tokio::test]
async fn the_list_survives_a_restart_and_belongs_to_its_own_session() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("todos");
    let cx = context(tmp.path(), "session-a");

    TodoTool::new(root.clone()).run(json!({"action": "add", "subject": "wire it"}), &cx).await;

    // A new tool over the same root is the restart.
    let listed = text(&TodoTool::new(root.clone()).run(json!({"action": "list"}), &cx).await);
    assert_eq!(listed, "[~] #1: wire it");

    // Another session is another list.
    let other = context(tmp.path(), "session-b");
    let listed = text(&TodoTool::new(root).run(json!({"action": "list"}), &other).await);
    assert_eq!(listed, "No todos");
}

#[tokio::test]
async fn todo_tool_complete() {
    let tmp = tempfile::tempdir().unwrap();
    let (tool, cx) = tool(&tmp);

    tool.run(json!({"action": "add", "subject": "Do thing B"}), &cx).await;
    let result = tool.run(json!({"action": "complete", "id": 1}), &cx).await;

    assert!(!result.is_error);
    assert_eq!(text(&result), "Completed todo #1: Do thing B");
    assert_eq!(text(&tool.run(json!({"action": "list"}), &cx).await), "[x] #1: Do thing B");
}

#[tokio::test]
async fn todo_tool_missing_action() {
    let tmp = tempfile::tempdir().unwrap();
    let (tool, cx) = tool(&tmp);

    assert!(tool.run(json!({}), &cx).await.is_error);
}

#[tokio::test]
async fn an_unknown_id_is_reported_rather_than_written() {
    let tmp = tempfile::tempdir().unwrap();
    let (tool, cx) = tool(&tmp);

    let missing = tool.run(json!({"action": "update", "id": 7, "status": "completed"}), &cx);
    assert_eq!(text(&missing.await), "Todo #7 not found");

    tool.run(json!({"action": "add", "subject": "real"}), &cx).await;
    let bad_status = tool.run(json!({"action": "update", "id": 1, "status": "napping"}), &cx);
    assert_eq!(text(&bad_status.await), "Invalid status: napping");
    // The rejected status left the item alone.
    assert_eq!(text(&tool.run(json!({"action": "list"}), &cx).await), "[~] #1: real");
}

#[tokio::test]
async fn todo_blocked_by_is_recorded_and_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    let (tool, cx) = tool(&tmp);

    // #1 is a prerequisite; #2 depends on it via `add { blocked_by }`.
    tool.run(json!({"action": "add", "subject": "lay foundation"}), &cx).await;
    tool.run(json!({"action": "add", "subject": "build wall", "blockedBy": [1]}), &cx).await;

    // `list` flags #2 as blocked while #1 is incomplete.
    let listed = text(&tool.run(json!({"action": "list"}), &cx).await);
    assert!(listed.contains("#2: build wall (blocked by #1)"), "got: {listed}");

    // Completing the blocker clears the flag.
    tool.run(json!({"action": "complete", "id": 1}), &cx).await;
    let listed = text(&tool.run(json!({"action": "list"}), &cx).await);
    assert!(!listed.contains("blocked by"), "block should clear once #1 done: {listed}");

    // `update { blocked_by }` sets dependencies after creation.
    tool.run(json!({"action": "add", "subject": "paint"}), &cx).await; // #3
    tool.run(json!({"action": "update", "id": 2, "blockedBy": [3]}), &cx).await;
    let listed = text(&tool.run(json!({"action": "list"}), &cx).await);
    assert!(listed.contains("#2: build wall (blocked by #3)"), "update sets deps: {listed}");

    // An empty list is not an instruction. Every call from a strict-schema model carries one,
    // so honoring it would wipe a real dependency the next time the item was touched at all.
    tool.run(json!({"action": "update", "id": 2, "blockedBy": []}), &cx).await;
    let listed = text(&tool.run(json!({"action": "list"}), &cx).await);
    assert!(listed.contains("blocked by #3"), "[] must not clear deps: {listed}");

    // Completing the blocker is what clears a block, and it needs no argument at all.
    tool.run(json!({"action": "complete", "id": 3}), &cx).await;
    let listed = text(&tool.run(json!({"action": "list"}), &cx).await);
    assert!(!listed.contains("blocked by"), "a finished blocker clears the flag: {listed}");
}

/// A deleted item is a tombstone: gone from the list, but its id is never reused.
#[tokio::test]
async fn a_deleted_todo_leaves_the_list_without_freeing_its_id() {
    let tmp = tempfile::tempdir().unwrap();
    let (tool, cx) = tool(&tmp);

    tool.run(json!({"action": "add", "subjects": ["one", "two"]}), &cx).await;
    tool.run(json!({"action": "update", "id": 1, "status": "deleted"}), &cx).await;
    assert_eq!(text(&tool.run(json!({"action": "list"}), &cx).await), "[ ] #2: two");

    tool.run(json!({"action": "add", "subject": "three"}), &cx).await;
    let listed = text(&tool.run(json!({"action": "list"}), &cx).await);
    assert!(listed.contains("#3: three"), "the next id follows the tombstone: {listed}");
}

/// A plan has to cost one call. With one item per call a six-step plan cost six round-trips,
/// and a model weighing cost against benefit skipped planning altogether.
#[tokio::test]
async fn a_whole_plan_is_created_in_one_call() {
    let tmp = tempfile::tempdir().unwrap();
    let (tool, cx) = tool(&tmp);

    let result = tool
        .run(
            json!({
                "action": "add",
                "subjects": ["rename the field", "add total_quantity", "add tests"]
            }),
            &cx,
        )
        .await;

    assert!(!result.is_error, "batch add succeeds");
    let added = text(&result);
    assert!(added.contains("Added 3 todos"), "{added}");
    assert!(added.contains("#1") && added.contains("#3"), "ids are reported: {added}");

    let listed = text(&tool.run(json!({"action": "list"}), &cx).await);
    for subject in ["rename the field", "add total_quantity", "add tests"] {
        assert!(listed.contains(subject), "{subject} survived: {listed}");
    }
}

#[tokio::test]
async fn a_single_add_still_works_and_keeps_its_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let (tool, cx) = tool(&tmp);

    let result =
        tool.run(json!({"action": "add", "subject": "just one", "blockedBy": []}), &cx).await;
    assert_eq!(text(&result), "Added todo #1: just one\nStarted todo #1: just one");
}

/// Working down a plan in order is one `complete` per item: each starts the next open item,
/// skipping one still blocked, and an item already in progress is never displaced.
#[tokio::test]
async fn the_list_moves_itself_along() {
    let tmp = tempfile::tempdir().unwrap();
    let (tool, cx) = tool(&tmp);
    let list = || async { text(&tool.run(json!({"action": "list"}), &cx).await) };

    let added = text(&tool.run(json!({"action": "add", "subjects": ["a", "b", "c"]}), &cx).await);
    assert!(added.ends_with("\nStarted todo #1: a"), "{added}");
    tool.run(json!({"action": "update", "id": 2, "blockedBy": [3]}), &cx).await;

    let done = text(&tool.run(json!({"action": "complete", "id": 1}), &cx).await);
    assert_eq!(done, "Completed todo #1: a\nStarted todo #3: c", "#2 waits on #3");
    assert_eq!(list().await, "[x] #1: a\n[ ] #2: b (blocked by #3)\n[~] #3: c");

    // Adding while something is in progress leaves it be
    let added = text(&tool.run(json!({"action": "add", "subject": "d"}), &cx).await);
    assert_eq!(added, "Added todo #4: d");

    let done = text(&tool.run(json!({"action": "complete", "id": 3}), &cx).await);
    assert_eq!(done, "Completed todo #3: c\nStarted todo #2: b");
    // Finishing an item out of order does not start a second one
    let done = text(&tool.run(json!({"action": "complete", "id": 4}), &cx).await);
    assert_eq!(done, "Completed todo #4: d");
    assert_eq!(list().await, "[x] #1: a\n[~] #2: b\n[x] #3: c\n[x] #4: d");
}

#[test]
fn subjects_wins_over_subject_and_blanks_are_dropped() {
    let batch = collect_subjects(&json!({
        "subjects": ["a", "  ", "b"], "subject": "ignored"
    }));
    assert_eq!(batch, vec!["a", "b"]);

    let single = collect_subjects(&json!({"subject": "  solo  "}));
    assert_eq!(single, vec!["solo"]);

    assert!(collect_subjects(&json!({"subjects": []})).is_empty());
}

/// A model under a strict JSON schema fills in every property it was shown, so an ordinary
/// single `add` arrives carrying `subjects: []` beside the real subject. Reading the empty
/// array as the answer rejected it — twice in a row in a real session, before the model gave up
/// and rephrased the call to get past a tool that was wrong about its own arguments.
#[tokio::test]
async fn a_default_filled_add_is_not_an_empty_one() {
    let tmp = tempfile::tempdir().unwrap();
    let (tool, cx) = tool(&tmp);

    let result = tool
        .run(
            json!({
                "action": "add", "activeForm": "", "blockedBy": [], "id": 0,
                "status": "pending", "subject": "scope the SSRF failure", "subjects": []
            }),
            &cx,
        )
        .await;

    assert!(!result.is_error, "{:?}", result.content);
    assert!(text(&result).starts_with("Added todo #1: scope the SSRF failure\n"));
}

/// The same default-filling makes every call carry `blocked_by: []` and `active_form: ""`. Read
/// literally, one `update` that meant only "start this" also wiped a real dependency and
/// labeled the item with an empty string.
#[tokio::test]
async fn a_default_filled_update_leaves_what_it_did_not_mention_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let (tool, cx) = tool(&tmp);

    tool.run(json!({"action": "add", "subject": "lay foundation"}), &cx).await;
    tool.run(json!({"action": "add", "subject": "build wall", "blockedBy": [1]}), &cx).await;
    tool.run(
        json!({
            "action": "update", "activeForm": "", "blockedBy": [], "id": 2,
            "status": "in_progress", "subject": "", "subjects": []
        }),
        &cx,
    )
    .await;

    let listed = text(&tool.run(json!({"action": "list"}), &cx).await);
    assert!(listed.contains("blocked by #1"), "the dependency survived: {listed}");

    // A dependency the model actually names still replaces the list wholesale.
    tool.run(json!({"action": "update", "id": 2, "blockedBy": [1, 3]}), &cx).await;
    let listed = text(&tool.run(json!({"action": "list"}), &cx).await);
    assert!(listed.contains("blocked by #1, #3"), "{listed}");
}

/// The card's header only has `#3` to show, so the result is where the item gets named.
#[tokio::test]
async fn an_update_names_the_item_it_moved() {
    let tmp = tempfile::tempdir().unwrap();
    let (tool, cx) = tool(&tmp);

    tool.run(json!({"action": "add", "subject": "wire the dock"}), &cx).await;
    let moved =
        text(&tool.run(json!({"action": "update", "id": 1, "status": "in_progress"}), &cx).await);
    assert_eq!(moved, "Updated todo #1 → in_progress: wire the dock");

    let relabeled =
        text(&tool.run(json!({"action": "update", "id": 1, "activeForm": "Wiring"}), &cx).await);
    assert_eq!(relabeled, "Updated todo #1: wire the dock");
}

/// The spellings these two arguments used to be advertised under still land.
///
/// Both are optional, so the failure mode of dropping one is not an error the model can see —
/// the label never appears, the dependency is never recorded, and the call reports success. A
/// session resumed from a transcript full of the old spelling would hit that on every update.
#[tokio::test]
async fn the_spellings_these_arguments_used_to_have_are_still_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    let (tool, cx) = tool(&tmp);

    tool.run(json!({"action": "add", "subject": "lay the foundation"}), &cx).await;
    tool.run(json!({"action": "add", "subject": "build the wall", "blocked_by": [1]}), &cx).await;
    tool.run(json!({"action": "update", "id": 2, "active_form": "Building"}), &cx).await;

    let listing = text(&tool.run(json!({"action": "list"}), &cx).await);
    assert!(listing.contains("blocked by #1"), "`blocked_by` was dropped: {listing}");

    let stored = TodoStore::new(tmp.path().join("todos")).read("test-session").unwrap();
    assert_eq!(stored[1].active_form.as_deref(), Some("Building"), "`active_form` was dropped");
}

#[tokio::test]
async fn an_add_with_nothing_to_add_says_which_argument_to_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let (tool, cx) = tool(&tmp);

    let result = tool.run(json!({"action": "add"}), &cx).await;

    assert!(result.is_error);
    assert!(text(&result).contains("subjects"), "names the batch form too");
}
