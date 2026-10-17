use super::*;
use luxe_core::ContentBlock;
use std::path::Path;
use std::sync::Arc;

mod plan;

pub(super) fn context(cwd: &Path) -> ToolContext {
    let (event_tx, _) = tokio::sync::mpsc::channel(8);
    let (_, cancelled) = tokio::sync::watch::channel(false);
    ToolContext {
        cwd: cwd.to_path_buf(),
        session_id: "t".to_string(),
        cancelled: Arc::new(cancelled),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    }
}

pub(super) async fn apply(
    edit: &Value,
    cwd: &Path,
    tracker: Option<&ReadTracker>,
) -> ToolResultData {
    let cx = context(cwd);
    apply_workspace_edit(edit, Writes { cx: &cx, tracker }).await
}

pub(super) fn edit_at(sl: u32, sc: u32, el: u32, ec: u32, new_text: &str) -> Value {
    serde_json::json!({
        "range": { "start": { "line": sl, "character": sc }, "end": { "line": el, "character": ec } },
        "newText": new_text
    })
}

/// A `documentChanges` edit, in the given file order.
pub(super) fn changes(files: &[(&Path, Vec<Value>)]) -> Value {
    let changes: Vec<Value> = files
        .iter()
        .map(|(path, edits)| {
            serde_json::json!({
                "textDocument": { "uri": crate::uri::from_path(path), "version": 1 },
                "edits": edits
            })
        })
        .collect();
    serde_json::json!({ "documentChanges": changes })
}

pub(super) fn text_of(result: &ToolResultData) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn text_edits_apply_in_descending_order() {
    let src = "let foo = 1;\nlet bar = foo + foo;\n";
    // Rename `foo` → `qux` at its 3 occurrences (L1 c4-7, L2 c10-13, L2 c16-19).
    let edits = vec![
        (0, 4, 0, 7, "qux".to_string()),
        (1, 10, 1, 13, "qux".to_string()),
        (1, 16, 1, 19, "qux".to_string()),
    ];
    let (out, applied) = apply_text_edits(src, edits);
    assert_eq!(out, "let qux = 1;\nlet bar = qux + qux;\n");
    assert_eq!(applied, 3);
}

#[test]
fn position_to_byte_handles_multibyte_utf16() {
    // "café" then an edit after it: é is 1 UTF-16 unit but 2 bytes.
    let src = "let café = 1;\n";
    let starts = line_start_bytes(src);
    // character 8 (after "let café") → byte 9 (é = 2 bytes).
    assert_eq!(position_to_byte(src, &starts, 0, 8), Some(9));
}

#[tokio::test]
async fn apply_workspace_edit_writes_and_reports() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("m.rs");
    std::fs::write(&file, "let foo = 1;\nlet bar = foo;\n").unwrap();
    let edit = changes(&[(&file, vec![edit_at(0, 4, 0, 7, "qux"), edit_at(1, 10, 1, 13, "qux")])]);
    let result = apply(&edit, dir.path(), None).await;
    assert!(!result.is_error, "{result:?}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "let qux = 1;\nlet bar = qux;\n");
}

#[test]
fn collect_edits_reads_changes_and_document_changes() {
    let changes = serde_json::json!({
        "changes": { "file:///a.rs": [
            { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } }, "newText": "xyz" }
        ]}
    });
    let files = collect_workspace_edits(&changes).files;
    assert_eq!(files[0].0, "file:///a.rs");
    assert_eq!(files[0].1.len(), 1);

    let doc_changes = serde_json::json!({
        "documentChanges": [
            { "textDocument": { "uri": "file:///b.rs", "version": 1 }, "edits": [
                { "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 5 } }, "newText": "q" }
            ]}
        ]
    });
    let files = collect_workspace_edits(&doc_changes).files;
    assert_eq!(files[0].0, "file:///b.rs");
    assert_eq!(files[0].1[0].4, "q");
}
