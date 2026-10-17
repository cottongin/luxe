//! `impact` tool — the blast radius of a change (design-code-graph.md §8).
//!
//! Answers "if I change X, what else is affected?" from the offline structural code graph: the
//! transitive callers, subclasses, and covering tests of a symbol or file, plus what it depends on
//! and any missing test coverage. It reads the warm [`GraphService`] (built lazily, cached, and
//! kept fresh by the `FileChanged` hook), so it's fast and needs no language server.

use super::*;
use crate::graph::{
    blast_radius, verify_callers, BlastRadius, GraphService, InMemoryGraph, LspReferenceSource,
    DEFAULT_MAX_DEPTH,
};

/// Reports the transitive impact of changing a symbol or file.
pub struct ImpactTool {
    graph: Arc<GraphService>,
    lsp: Option<Arc<crate::WorkspaceLspClient>>,
}

impl ImpactTool {
    /// Create the tool over the shared warm graph service.
    pub fn new(graph: Arc<GraphService>) -> Self {
        Self { graph, lsp: None }
    }

    /// Attach the warm LSP service so `verify: true` can confirm heuristic callers against the
    /// server's precise references (degrades gracefully when no server answers).
    pub fn with_lsp(mut self, lsp: Arc<crate::WorkspaceLspClient>) -> Self {
        self.lsp = Some(lsp);
        self
    }

    /// LSP verification of the blast radius's seed callers, appended to the output. `None` when no
    /// LSP service is attached.
    async fn verify_section(
        &self,
        graph: &InMemoryGraph,
        br: &BlastRadius,
        cx: &ToolContext,
    ) -> Option<String> {
        const MAX_VERIFY_SEEDS: usize = 10;
        let lsp = self.lsp.as_ref()?;
        let source = LspReferenceSource::new(lsp.clone(), cx.cwd.clone());

        let mut verified = std::collections::BTreeSet::new();
        let mut recovered = std::collections::BTreeSet::new();
        let mut answered = false;
        for seed in br.seeds.iter().take(MAX_VERIFY_SEEDS) {
            if let Some(callers) = verify_callers(graph, seed, &source).await {
                answered = true;
                for caller in callers {
                    if caller.in_lsp {
                        verified.insert(caller.file.clone());
                        if !caller.in_graph {
                            recovered.insert(caller.file);
                        }
                    }
                }
            }
        }

        if !answered {
            return Some("\nVerification: no language server answered (heuristic only).\n".into());
        }
        let mut out = String::from("\nLSP-verified callers (rust-analyzer):\n");
        if verified.is_empty() {
            out.push_str("  (none confirmed)\n");
        }
        for file in &verified {
            out.push_str(&format!("  {file}\n"));
        }
        if !recovered.is_empty() {
            out.push_str("LSP recovered (missed by the heuristic graph):\n");
            for file in &recovered {
                out.push_str(&format!("  {file}\n"));
            }
        }
        Some(out)
    }
}

#[async_trait]
impl Tool for ImpactTool {
    fn name(&self) -> &str {
        "impact"
    }

    fn argument_aliases(&self) -> &'static [&'static str] {
        &["symbol", "file", "path"]
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "impact".to_string(),
            description:
                "Compute the blast radius of a change from a fast offline code graph: the \
                          transitive callers, dependents, subclasses, and covering tests of a \
                          symbol or file (\"if I change X, what else is affected?\"), plus what it \
                          relies on and any seed with no covering test. Give a symbol name, a \
                          qualified name (file::Symbol), or a file path (relative to cwd)."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": "A symbol name, qualified name (file::Symbol), or file path relative to cwd" },
                    "depth": { "type": "integer", "description": "Max transitive depth to walk (default 2, max 5)" },
                    "refresh": { "type": "boolean", "description": "Rebuild the graph before querying" },
                    "verify": { "type": "boolean", "description": "Confirm heuristic callers against the language server's precise references (slower; needs a warm server)" }
                },
                "required": ["target"]
            }),
        }
    }

    async fn run(&self, args: serde_json::Value, cx: &ToolContext) -> ToolResultData {
        // `target` is the documented spelling; a model arriving from `read`/`read_symbol` holds
        // the word it just used, and that call means the same thing.
        let target = ["target", "symbol", "file", "path"]
            .iter()
            .find_map(|key| args.get(*key).and_then(|value| value.as_str()))
            .filter(|text| !text.is_empty());
        let Some(target) = target else {
            return ToolResultData::error("Error: 'target' is required");
        };
        let depth = args
            .get("depth")
            .and_then(serde_json::Value::as_u64)
            .map(|d| d as u32)
            .unwrap_or(DEFAULT_MAX_DEPTH)
            .clamp(1, 5);
        let refresh = args.get("refresh").and_then(serde_json::Value::as_bool).unwrap_or(false);

        let verify = args.get("verify").and_then(serde_json::Value::as_bool).unwrap_or(false);

        let target = project_relative(target, cx);
        let graph = match self.graph.graph(refresh).await {
            Ok(graph) => graph,
            Err(error) => return ToolResultData::error(format!("Error: {error}")),
        };
        let br = blast_radius(&graph, &target, depth);
        let mut out = br.render(&target);
        if verify {
            if let Some(section) = self.verify_section(&graph, &br, cx).await {
                out.push_str(&section);
            }
        }
        ToolResultData::text(out)
    }
}

/// Make an absolute path under `cwd` project-relative (graph keys are project-relative). A symbol
/// or already-relative path passes through unchanged.
fn project_relative(target: &str, cx: &ToolContext) -> String {
    Path::new(target)
        .strip_prefix(&cx.cwd)
        .map(luxe_core::paths::display)
        .unwrap_or_else(|_| target.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::{mpsc, watch};

    fn context(cwd: &Path) -> ToolContext {
        let (event_tx, _rx) = mpsc::channel(8);
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        ToolContext {
            cwd: cwd.to_path_buf(),
            session_id: "test".to_string(),
            cancelled: Arc::new(cancel_rx),
            event_tx,
            hook_ctx: None,
            question_broker: None,
        }
    }

    /// A throwaway project, owned by the test that asked for it.
    ///
    /// One directory per test: a shared one deleted at the start and end of each test is a race
    /// under `cargo test`, where one test deletes the tree the other is walking.
    fn scratch() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/auth.rs"), "pub fn login() {}\npub fn logout() {}\n")
            .unwrap();
        std::fs::write(
            root.join("src/session.rs"),
            "use crate::auth::login;\npub fn start() { login() }\n",
        )
        .unwrap();
        dir
    }

    #[tokio::test]
    async fn impact_reports_callers_and_test_gap() {
        let dir = scratch();
        let tool = ImpactTool::new(Arc::new(GraphService::new(dir.path())));
        let cx = context(dir.path());

        let result = tool.run(json!({ "target": "login" }), &cx).await;
        assert!(!result.is_error);
        let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
        // The caller file appears as a dependent, and login has no covering test. Forward slashes
        // on every platform: this line is read by the model and handed straight back to `read`.
        assert!(text.contains("src/session.rs"), "caller listed:\n{text}");
        assert!(text.contains("no covering test"), "test-gap flagged:\n{text}");
    }

    #[tokio::test]
    async fn impact_unknown_target_is_not_an_error() {
        let dir = scratch();
        let tool = ImpactTool::new(Arc::new(GraphService::new(dir.path())));
        let cx = context(dir.path());

        let result = tool.run(json!({ "target": "nope_missing" }), &cx).await;
        assert!(!result.is_error);
        let ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
        assert!(text.contains("No graph node found"));
    }
}
