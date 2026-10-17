//! The language-server tools — diagnostics, navigation, symbol discovery and impact — over the
//! shared workspace service, which opens documents, waits for diagnostics and answers navigation.

use crate::{DiagnosticSeverity, LspClient, Position};
use async_trait::async_trait;
use luxe_core::{ContentBlock, Tool, ToolContext, ToolResultData, ToolSchema};
use serde_json::json;
use std::path::Path;
use std::sync::Arc;

mod diagnostics;
mod discovery;
mod impact;
mod navigation;

pub use diagnostics::LspDiagnosticsTool;
pub use discovery::{ModuleReportTool, ReadSymbolTool, SymbolSearchTool};
pub use impact::ImpactTool;
pub use navigation::LspNavigationTool;

/// The spellings [`file_arg`] reads, for [`luxe_core::Tool::argument_aliases`].
const FILE_ALIASES: &[&str] = &["path", "file", "file_path", "filePath"];

/// The file a call names.
///
/// `path` is the word every tool in the set advertises, so nothing here is a dialect to remember.
/// The rest are tolerated because they were once advertised (`file`, until these tools were brought
/// in line) or because another harness trained a model on them — the same recovery path `edit`
/// keeps, for the same reason: rejecting a spelling costs a turn and teaches nothing.
fn file_arg(args: &serde_json::Value) -> Option<&str> {
    FILE_ALIASES
        .iter()
        .find_map(|key| args.get(*key).and_then(|value| value.as_str()))
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::{mpsc, watch};

    fn test_context(tmp_dir: &std::path::Path) -> ToolContext {
        let (event_tx, _event_rx) = mpsc::channel(64);
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        ToolContext {
            cwd: tmp_dir.to_path_buf(),
            session_id: "test-session".to_string(),
            cancelled: Arc::new(cancel_rx),
            event_tx,
            hook_ctx: None,
            question_broker: None,
        }
    }

    #[tokio::test]
    async fn lsp_diagnostics_requires_paths() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let service = Arc::new(crate::WorkspaceLspClient::new());
        let tool = LspDiagnosticsTool::new(service);
        let cx = test_context(tmp_dir.path());
        let result = tool.run(json!({}), &cx).await;
        assert!(result.is_error);
        assert_eq!(
            result.content,
            vec![ContentBlock::Text("Error: provide `file` or `paths`".into())]
        );
    }

    #[tokio::test]
    async fn lsp_diagnostics_unconfigured_language() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let service = Arc::new(crate::WorkspaceLspClient::new());
        let tool = LspDiagnosticsTool::new(service);
        let cx = test_context(tmp_dir.path());
        let result = tool.run(json!({"file": "test.unconfigured"}), &cx).await;
        assert!(result.is_error);
        let ContentBlock::Text(text) = &result.content[0] else {
            panic!("expected Text block");
        };
        assert!(text.contains("no language server configured for .unconfigured"), "{text}");
        assert!(text.contains("lsp-servers.json"), "points at configuring one: {text}");
    }

    /// A model that has just called `read(path=…)` reaches for `read_symbol(path=…)` next. That is
    /// the same call, so it must not be an error.
    #[test]
    fn the_file_argument_answers_to_the_spelling_its_neighbors_use() {
        for key in ["path", "file", "file_path", "filePath"] {
            let args = json!({ key: "src/lib.rs" });
            assert_eq!(super::file_arg(&args), Some("src/lib.rs"), "{key}");
        }
        assert_eq!(super::file_arg(&json!({})), None);
        assert_eq!(super::file_arg(&json!({"path": ""})), None, "empty is not a path");
        // The advertised spelling wins when a call somehow carries both.
        let both = json!({"path": "canonical.rs", "file": "alias.rs"});
        assert_eq!(super::file_arg(&both), Some("canonical.rs"));
    }
}
