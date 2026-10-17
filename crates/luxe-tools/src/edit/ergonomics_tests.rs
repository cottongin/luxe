//! Ergonomics — what a mistaken edit says back.

#![allow(clippy::unwrap_used)]

use super::*;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::{mpsc, watch};

fn context(root: &std::path::Path) -> ToolContext {
    let (event_tx, _event_rx) = mpsc::channel(64);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    ToolContext {
        cwd: root.to_path_buf(),
        session_id: "test-session".to_string(),
        cancelled: Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    }
}

async fn run(root: &TempDir, args: serde_json::Value) -> ToolResultData {
    EditTool::new().run(args, &context(root.path())).await
}

fn text_of(result: &ToolResultData) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn project(contents: &str) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.rs"), contents).expect("write");
    dir
}

/// A Windows checkout is CRLF, the model writes LF, and the edit used to land anyway — the
/// whole-line relaxation trims the stray `\r` — but wrote LF back, leaving the four edited
/// lines as the only LF lines in the file. `git diff` then reports every line as changed.
#[tokio::test]
async fn an_edit_to_a_crlf_file_keeps_the_file_crlf() {
    let dir = project("fn main() {\r\n    let x = 1;\r\n    go();\r\n}\r\n");
    let result = run(
        &dir,
        json!({"path": "a.rs",
               "oldText": "    let x = 1;\n    go();",
               "newText": "    let x = 2;\n    go_twice();"}),
    )
    .await;

    assert!(!result.is_error, "{}", text_of(&result));
    let after = std::fs::read_to_string(dir.path().join("a.rs")).unwrap();
    assert_eq!(after, "fn main() {\r\n    let x = 2;\r\n    go_twice();\r\n}\r\n");
    assert_eq!(after.matches('\n').count(), after.matches("\r\n").count(), "no mixed endings");
}

/// And the converse: a unix file stays unix even if the model sends CRLF (some do, having read
/// a Windows file earlier in the same session).
#[tokio::test]
async fn an_edit_carrying_crlf_does_not_convert_a_unix_file() {
    let dir = project("a();\nb();\n");
    let result =
        run(&dir, json!({"path": "a.rs", "oldText": "a();\r\nb();", "newText": "c();\r\nd();"}))
            .await;

    assert!(!result.is_error, "{}", text_of(&result));
    assert_eq!(std::fs::read_to_string(dir.path().join("a.rs")).unwrap(), "c();\nd();\n");
}

/// The reported failure: Sonnet reached for Claude Code's argument names and was rejected.
#[tokio::test]
async fn a_call_in_another_harness_dialect_still_applies() {
    let dir = project("let x = 1;\n");
    let result = run(
        &dir,
        json!({"file_path": "a.rs", "old_string": "let x = 1;", "new_string": "let x = 2;"}),
    )
    .await;
    assert!(!result.is_error, "{}", text_of(&result));
    assert_eq!(std::fs::read_to_string(dir.path().join("a.rs")).unwrap(), "let x = 2;\n");
}

/// The spelling is the fix, so it is pinned rather than left to a consistency pass.
///
/// `old_text`/`new_text` cost roughly two in five Sonnet edit calls: the separator after a key
/// ending in `_text` came back as `">` instead of `": "`, which the provider's JSON repair
/// turns into a stringified `edits` with its sibling field hoisted to the top level. Nothing
/// downstream can recover that without guessing at file content, and no wording of the error
/// prevents it — only the name does. The snake_case spellings stay *accepted* (see
/// [`args`]); what must never come back is advertising one.
#[test]
fn the_schema_advertises_no_argument_whose_name_ends_in_text() {
    let schema = edit_schema();
    let item = &schema.parameters["properties"]["edits"]["items"];
    // `serde_json` preserves insertion order here, so this is both the order the provider is
    // sent *and* the order it was written in — which is the order a model fills them in.
    let advertised: Vec<&str> = item["properties"]
        .as_object()
        .expect("item properties")
        .keys()
        .map(String::as_str)
        .collect();

    assert_eq!(
        advertised,
        ["oldText", "newText", "replaceAll"],
        "the advertised spelling is the whole fix, and it reads in the order it is applied"
    );
    assert_eq!(item["required"], json!(["oldText", "newText"]));

    let text = serde_json::to_string(&schema.parameters).expect("serializes");
    for poisoned in ["old_text", "new_text"] {
        assert!(!text.contains(poisoned), "the schema still names `{poisoned}`: {text}");
    }
}

/// The refusal has to demonstrate the same shape the schema asks for.
///
/// A model that gets the spelling wrong reads the error and retries, and the retry copies
/// whatever the error showed it — measurably so. Letting the two drift means one of them is
/// teaching the shape that fails.
#[tokio::test]
async fn the_shape_a_refusal_shows_is_the_shape_the_schema_asks_for() {
    let dir = project("x\n");
    let refusal = text_of(&run(&dir, json!({"filename": "a.rs"})).await);

    let schema = edit_schema();
    let advertised = schema.parameters["properties"]["edits"]["items"]["properties"]
        .as_object()
        .expect("item properties");
    for name in advertised.keys() {
        assert!(refusal.contains(name), "the refusal never shows `{name}`:\n{refusal}");
    }
    assert!(refusal.contains("\"path\""), "{refusal}");
}

/// The corruption this rewrite exists to prevent.
///
/// Uniqueness was validated against the *original* text and the edits then applied in sequence
/// with `str::replace`, so an earlier replacement whose output contained a later edit's search
/// text changed both — silently, and reported as success.
#[tokio::test]
async fn an_edit_never_rewrites_text_an_earlier_edit_produced() {
    let dir = project("alpha\nomega\n");
    let result = run(
        &dir,
        json!({"path": "a.rs", "edits": [
            {"oldText": "alpha", "newText": "omega"},
            {"oldText": "omega", "newText": "final"},
        ]}),
    )
    .await;
    assert!(!result.is_error, "{}", text_of(&result));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.rs")).unwrap(),
        "omega\nfinal\n",
        "each edit applies to the text it matched, not to the previous edit's output"
    );
}

#[tokio::test]
async fn overlapping_edits_are_refused_before_anything_is_written() {
    let dir = project("one two three\n");
    let result = run(
        &dir,
        json!({"path": "a.rs", "edits": [
            {"oldText": "one two", "newText": "1 2"},
            {"oldText": "two three", "newText": "2 3"},
        ]}),
    )
    .await;
    assert!(result.is_error);
    let message = text_of(&result);
    assert!(message.contains("same text"), "{message}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.rs")).unwrap(),
        "one two three\n",
        "nothing was written"
    );
}

#[tokio::test]
async fn replace_all_changes_every_occurrence_and_says_how_many() {
    let dir = project("go();\ngo();\ngo();\n");
    let result = run(
        &dir,
        json!({"path": "a.rs", "edits": [
            {"oldText": "go();", "newText": "stop();", "replaceAll": true},
        ]}),
    )
    .await;
    assert!(!result.is_error, "{}", text_of(&result));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.rs")).unwrap(),
        "stop();\nstop();\nstop();\n"
    );
    assert!(text_of(&result).contains("replaceAll"), "the extra occurrences are reported");
}

/// An edit that changes nothing is refused, because "applied" is a claim the model acts on.
///
/// It reported success and wrote the file back byte-identical, so the agent moved on believing
/// the change was in — marked the todo done, ran the gate against unmodified code, wrote it up
/// in the journal. A refusal costs one turn; a false success costs everything downstream of it.
#[tokio::test]
async fn an_edit_that_would_change_nothing_is_refused_rather_than_reported_as_applied() {
    let dir = project("let x = 1;\n");
    let result =
        run(&dir, json!({"path": "a.rs", "oldText": "let x = 1;", "newText": "let x = 1;"})).await;

    assert!(result.is_error, "{}", text_of(&result));
    let message = text_of(&result);
    assert!(message.contains("changes nothing"), "{message}");
    assert!(message.contains("Nothing was written"), "{message}");
    assert_eq!(std::fs::read_to_string(dir.path().join("a.rs")).unwrap(), "let x = 1;\n");
}

/// The check asks whether the *file* moves, which is not the same as `oldText != newText`.
///
/// Line endings are normalized on both sides before an edit is placed, so a call that differs
/// only in `\r\n` versus `\n` resolves to writing back what is already there. That reads as a
/// change in the arguments and is none on disk.
#[tokio::test]
async fn an_edit_that_only_changes_line_endings_is_the_same_no_op() {
    let dir = project("a();\nb();\n");
    let result =
        run(&dir, json!({"path": "a.rs", "oldText": "a();\nb();", "newText": "a();\r\nb();"}))
            .await;

    assert!(result.is_error, "{}", text_of(&result));
    assert!(text_of(&result).contains("changes nothing"), "{}", text_of(&result));
}

/// One dead edit in a batch is still refused, and the whole batch stays unwritten.
///
/// A whole-file comparison after applying would miss this: edit[0] moves the file, so the file
/// moved, so nothing looks wrong — and edit[1] is silently counted among the "2 edit(s)
/// applied". Placing the check per edit is what makes the count honest.
#[tokio::test]
async fn a_no_op_beside_a_real_edit_is_named_by_its_index_and_neither_lands() {
    let dir = project("alpha\nbeta\n");
    let result = run(
        &dir,
        json!({"path": "a.rs", "edits": [
            {"oldText": "alpha", "newText": "gamma"},
            {"oldText": "beta", "newText": "beta"},
        ]}),
    )
    .await;

    assert!(result.is_error, "{}", text_of(&result));
    assert!(text_of(&result).contains("edit[1]"), "names which one: {}", text_of(&result));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.rs")).unwrap(),
        "alpha\nbeta\n",
        "atomic: the real edit beside it did not land either"
    );
}

/// Deleting text is a change, and the no-op check must not read an empty replacement as one.
#[tokio::test]
async fn an_empty_replacement_still_deletes() {
    let dir = project("keep();\ndrop();\n");
    let result = run(&dir, json!({"path": "a.rs", "oldText": "drop();\n", "newText": ""})).await;

    assert!(!result.is_error, "{}", text_of(&result));
    assert_eq!(std::fs::read_to_string(dir.path().join("a.rs")).unwrap(), "keep();\n");
}

/// An ambiguity error has to say where, or "add more context" is advice with no target.
#[tokio::test]
async fn an_ambiguous_edit_names_the_lines_it_matched() {
    let dir = project("go();\nwait();\ngo();\n");
    let result = run(&dir, json!({"path": "a.rs", "oldText": "go();", "newText": "stop();"})).await;
    assert!(result.is_error);
    let message = text_of(&result);
    assert!(message.contains("matched 2 times"), "{message}");
    assert!(message.contains("lines 1, 3"), "and where: {message}");
    assert!(message.contains("replaceAll"), "and the way out: {message}");
}

/// "not found" alone is undiagnosable — a typo, a stale read and the wrong file all look the
/// same. Pointing at the closest line makes the next attempt a correction.
#[tokio::test]
async fn numbers_copied_from_read_are_left_out_and_said_so() {
    let dir = project("fn main() {\n    let total = 1;\n    go();\n}\n");
    let result = run(
        &dir,
        json!({"path": "a.rs", "edits": [{
            "oldText": "2:     let total = 1;\n3:     go();",
            "newText": "2:     let total = 2;\n3:     go_twice();"
        }]}),
    )
    .await;
    assert!(!result.is_error, "{}", text_of(&result));
    assert!(text_of(&result).contains("line numbers `read` shows were left out"));
    let after = std::fs::read_to_string(dir.path().join("a.rs")).unwrap();
    assert_eq!(after, "fn main() {\n    let total = 2;\n    go_twice();\n}\n");
}

/// The numbers have to be the real ones: text that matches elsewhere, numbers that skip, or a
/// match that is still ambiguous is an edit the caller did not mean, and is refused as written.
#[tokio::test]
async fn numbers_that_do_not_name_the_matched_lines_are_not_stripped() {
    let source = "fn main() {\n    go();\n    go();\n}\n";
    for old_text in ["9:     go();", "2:     go();\n4:     go();", "x2:     go();"] {
        let dir = project(source);
        let result = run(
            &dir,
            json!({"path": "a.rs", "edits": [{"oldText": old_text, "newText": "stop();"}]}),
        )
        .await;
        assert!(result.is_error, "{old_text}: {}", text_of(&result));
        assert_eq!(std::fs::read_to_string(dir.path().join("a.rs")).unwrap(), source);
    }
    // a prefix `read` never writes, and a number naming one line beside `replaceAll`
    let single = "fn main() {\n    let total = 1;\n}\n";
    for edit in [
        json!({"oldText": "+2:     let total = 1;", "newText": "    let total = 2;"}),
        json!({"oldText": "2:     let total = 1;", "newText": "    let total = 2;", "replaceAll": true}),
    ] {
        let dir = project(single);
        let result = run(&dir, json!({"path": "a.rs", "edits": [edit.clone()]})).await;
        assert!(result.is_error, "{edit}: {}", text_of(&result));
        assert_eq!(std::fs::read_to_string(dir.path().join("a.rs")).unwrap(), single);
    }
    // and a number does not make an ambiguous match unique: that stays the caller's to fix
    let dir = project(source);
    let result = run(
        &dir,
        json!({"path": "a.rs", "edits": [{"oldText": "3:     go();", "newText": "    stop();"}]}),
    )
    .await;
    assert!(result.is_error, "{}", text_of(&result));
    assert_eq!(std::fs::read_to_string(dir.path().join("a.rs")).unwrap(), source);
}

#[tokio::test]
async fn a_near_miss_reports_what_is_actually_there() {
    let dir = project("fn main() {\n    let total = 1;\n}\n");
    let result =
        run(&dir, json!({"path": "a.rs", "oldText": "    let total = 2;", "newText": "x"})).await;
    assert!(result.is_error);
    let message = text_of(&result);
    assert!(message.contains("Closest match is line 2"), "{message}");
    assert!(message.contains("let total = 1;"), "and what is there: {message}");
    assert!(message.contains("Nothing was written"), "{message}");
}

/// A tolerated match is never a silent one.
#[tokio::test]
async fn a_relaxed_match_applies_and_is_reported() {
    let dir = project("impl T {\n        fn go() {\n            work();\n        }\n}\n");
    let result = run(
        &dir,
        json!({"path": "a.rs", "edits": [{
            "oldText": "fn go() {\n    work();\n}",
            "newText": "fn go() {\n    work_twice();\n}",
        }]}),
    )
    .await;
    assert!(!result.is_error, "{}", text_of(&result));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.rs")).unwrap(),
        "impl T {\n        fn go() {\n            work_twice();\n        }\n}\n",
        "the replacement is re-indented to the file's depth"
    );
    assert!(text_of(&result).contains("re-indenting"), "and the tolerance is declared");
}

#[tokio::test]
async fn a_call_that_cannot_be_understood_says_what_arrived_and_what_was_expected() {
    let dir = project("x\n");
    let result = run(&dir, json!({"filename": "a.rs", "patch": "…"})).await;
    assert!(result.is_error);
    let message = text_of(&result);
    assert!(message.contains("filename"), "names the keys it got: {message}");
    assert!(message.contains("`path`"), "names the one to use: {message}");
    assert!(message.contains("\"oldText\""), "and shows the whole shape: {message}");
}
