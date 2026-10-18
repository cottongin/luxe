use super::*;
use luxe_core::hooks::file_changed::FunctionSize;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

/// A stand-in for the tree-sitter outline: in a `.py` file, a class over lines 1–9 (0-based 0–8)
/// with a method over lines 3–6, and nothing it can read in any other file.
struct OneClass;

impl OutlineSource for OneClass {
    fn functions(&self, _: &Path, _: &str) -> Option<Vec<FunctionSize>> {
        None
    }

    fn scopes(&self, path: &Path, _: &str) -> Option<Vec<Scope>> {
        let scope =
            |name: &str, start_line, end_line| Scope { name: name.into(), start_line, end_line };
        (path.extension()? == "py").then(|| vec![scope("QuerySet", 0, 8), scope("delete", 2, 5)])
    }
}

fn context(cwd: &Path) -> ToolContext {
    let (event_tx, _event_rx) = mpsc::channel(64);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    ToolContext {
        cwd: cwd.to_path_buf(),
        session_id: "test-session".to_string(),
        cancelled: Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    }
}

/// Each match is labeled with the scopes around it, outermost first, and only where the label
/// changes. The definition line takes its parent's label, since it names itself; a file no outline
/// can read gets no label at all; and the path comes once, above its lines.
#[tokio::test]
async fn a_match_is_labeled_with_the_scopes_around_it() {
    let dir = tempfile::tempdir().unwrap();
    let python =
        "class QuerySet:\n    \n    def delete(self):\n        self.delete_batch()\n        \
                  return self\n    \n    \nOTHER = 1\n    \ndef delete():\n    pass\n";
    std::fs::write(dir.path().join("a.py"), python).unwrap();
    std::fs::write(dir.path().join("notes.md"), "delete it\n").unwrap();
    let tool = GrepTool::new().with_outline(Arc::new(OneClass));
    let args = json!({"pattern": "delete", "scopes": true});
    let result = tool.run(args, &context(dir.path())).await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    let expected = "a.py\n  [QuerySet]\n3:    def delete(self):\n  [QuerySet › delete]\n4:        \
                    self.delete_batch()\n  [top level]\n10:def delete():\nnotes.md\n1:delete it";
    assert_eq!(text, expected);
}

/// Labels cost a row each, so a search without `scopes` gets the grouped lines alone, outline or
/// not.
#[tokio::test]
async fn a_search_is_unlabeled_unless_it_asks() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.py"), "class A:\n    \n    def delete(self):\n").unwrap();
    let tool = GrepTool::new().with_outline(Arc::new(OneClass));
    let result = tool.run(json!({"pattern": "delete"}), &context(dir.path())).await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert_eq!(text, "a.py\n3:    def delete(self):");
}

/// The coarse modes have no lines to label, so they ask nothing of the outline.
#[tokio::test]
async fn the_files_mode_carries_no_scopes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.py"), "class A:\n    def delete(self): pass\n").unwrap();
    let tool = GrepTool::new().with_outline(Arc::new(OneClass));
    let args = json!({"pattern": "delete", "outputMode": "files"});
    let result = tool.run(args, &context(dir.path())).await;
    let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert_eq!(text.trim(), "a.py");
}
