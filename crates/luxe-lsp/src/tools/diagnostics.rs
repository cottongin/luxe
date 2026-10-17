//! The `lsp_diagnostics` tool — warm, batchable, severity-filterable diagnostics.
//!
//! Uses the shared warm [`WorkspaceLspClient`] service for reused servers and settle-aware reads.

use super::*;
use crate::protocol::Diagnostic;
use crate::workspace::WorkspaceLspClient;
use std::time::Duration;

/// Default settle budget when the caller doesn't pass `waitMs`.
const DEFAULT_WAIT: Duration = Duration::from_secs(10);

/// JSON schema for the `lsp_diagnostics` tool arguments.
fn lsp_diagnostics_schema() -> ToolSchema {
    ToolSchema {
        name: "lsp_diagnostics".to_string(),
        description: "Fetch diagnostics via a warm language server (rust-analyzer, pyright, \
                      tsserver, gopls). Reuses a running server and waits for it to settle, so \
                      results are real, not empty pre-index snapshots. Check one file (`path`) or \
                      a batch (`paths`), optionally filtered by `severity`."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to a file to check (relative to cwd)" },
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Batch of files to check (relative to cwd). Use instead of `path`."
                },
                "severity": {
                    "type": "string",
                    "enum": ["error", "warning", "information", "hint", "all"],
                    "description": "Minimum severity to include (default: all)."
                },
                "waitMs": {
                    "type": "integer",
                    "description": "Max time to wait for the server to settle per file (default 10000)."
                }
            }
        }),
    }
}

/// The `lsp_diagnostics` tool.
pub struct LspDiagnosticsTool {
    service: Arc<WorkspaceLspClient>,
}

impl LspDiagnosticsTool {
    /// Creates a diagnostics tool backed by the shared warm service.
    pub fn new(service: Arc<WorkspaceLspClient>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl Tool for LspDiagnosticsTool {
    fn name(&self) -> &str {
        "lsp_diagnostics"
    }

    fn argument_aliases(&self) -> &'static [&'static str] {
        super::FILE_ALIASES
    }

    fn schema(&self) -> ToolSchema {
        lsp_diagnostics_schema()
    }

    async fn run(&self, args: serde_json::Value, cx: &ToolContext) -> ToolResultData {
        let requested = requested_paths(&args);
        if requested.is_empty() {
            return ToolResultData::error("Error: provide `file` or `paths`");
        }
        let min_severity = min_severity(args.get("severity").and_then(|v| v.as_str()));
        let wait = args
            .get("waitMs")
            .and_then(serde_json::Value::as_u64)
            .map_or(DEFAULT_WAIT, Duration::from_millis);

        let mut output = String::new();
        let mut errored = 0usize;
        for rel in &requested {
            let full = cx.cwd.join(rel);
            // Named the way every other tool names a file, relative to the project
            let shown = full.strip_prefix(&cx.cwd).unwrap_or(&full);
            match self.diagnostics_for(&full, wait).await {
                Ok((diags, notices)) => {
                    output.push_str(&format_file(shown, &diags, min_severity, &notices))
                }
                Err(e) => {
                    errored += 1;
                    output.push_str(&format!("{}: {e}\n", luxe_core::paths::display(shown)));
                }
            }
        }
        // Only a total failure (every requested file errored) is a tool error; a mixed batch
        // still reports per-file.
        ToolResultData {
            content: vec![ContentBlock::Text(output)],
            is_error: errored == requested.len(),
        }
    }
}

impl LspDiagnosticsTool {
    /// Fetches diagnostics for one file via the warm service.
    async fn diagnostics_for(
        &self,
        full_path: &Path,
        wait: Duration,
    ) -> Result<(Vec<Diagnostic>, Vec<String>), String> {
        let uri = crate::uri::from_path(full_path);

        let client = { self.service.client_for_path(full_path).await };
        let Some(client) = client else {
            return Err(self.service.missing_server_hint(full_path));
        };
        let content = std::fs::read_to_string(full_path)
            .map_err(|e| format!("read {}: {e}", luxe_core::paths::display(full_path)))?;
        let mut guard = client.lock().await;
        let diags = guard
            .get_diagnostics_with_wait(&uri, &content, wait)
            .await
            .map_err(|e| format!("diagnostics failed: {e}"))?;
        // Explain an empty result: a misconfigured server often says why via a notice.
        let notices = if diags.is_empty() { guard.server_notices() } else { Vec::new() };
        Ok((diags, notices))
    }
}

/// Collects the requested paths from `paths` (batch) or `file` (single).
fn requested_paths(args: &serde_json::Value) -> Vec<String> {
    if let Some(paths) = args.get("paths").and_then(|v| v.as_array()) {
        return paths.iter().filter_map(|p| p.as_str().map(str::to_string)).collect();
    }
    super::file_arg(args).map(|s| vec![s.to_string()]).unwrap_or_default()
}

/// Maps a `severity` argument to the highest numeric severity to include (1=error … 4=hint).
fn min_severity(arg: Option<&str>) -> u8 {
    match arg {
        Some("error") => 1,
        Some("warning") => 2,
        Some("information") => 3,
        _ => 4, // "hint" or "all" or unset — include everything
    }
}

/// Most diagnostics listed for one file before the remainder is summarized.
const MAX_DIAGNOSTICS: usize = 50;

fn format_file(
    full_path: &Path,
    diagnostics: &[Diagnostic],
    min_severity: u8,
    notices: &[String],
) -> String {
    let filtered: Vec<&Diagnostic> =
        diagnostics.iter().filter(|d| severity_rank(d.severity) <= min_severity).collect();
    if filtered.is_empty() {
        let mut out = format!("No diagnostics for {}\n", luxe_core::paths::display(full_path));
        // A server that can't do its job usually said so — surface it rather than imply "clean".
        for notice in notices {
            out.push_str(&format!("  note: language server reported {notice}\n"));
        }
        return out;
    }
    let mut out = format!(
        "Diagnostics for {} ({} found):\n",
        luxe_core::paths::display(full_path),
        filtered.len()
    );
    // One bad refactor or one generated file yields hundreds of diagnostics that are overwhelmingly
    // the same error repeated. The header already carries the true count, so bound the list itself.
    for (i, diag) in filtered.iter().take(MAX_DIAGNOSTICS).enumerate() {
        out.push_str(&format!(
            "{}. [{}] {}:{}: {}\n",
            i + 1,
            severity_str(diag.severity),
            diag.range.start.line + 1,
            diag.range.start.character + 1,
            diag.message
        ));
    }
    if filtered.len() > MAX_DIAGNOSTICS {
        out.push_str(&format!(
            "… and {} more. Fix these first, then re-run — repeats usually share one cause.\n",
            filtered.len() - MAX_DIAGNOSTICS
        ));
    }
    out
}

fn severity_rank(severity: Option<DiagnosticSeverity>) -> u8 {
    match severity {
        Some(DiagnosticSeverity::Error) => 1,
        Some(DiagnosticSeverity::Warning) => 2,
        Some(DiagnosticSeverity::Information) => 3,
        Some(DiagnosticSeverity::Hint) | None => 4,
    }
}

fn severity_str(severity: Option<DiagnosticSeverity>) -> &'static str {
    match severity {
        Some(DiagnosticSeverity::Error) => "ERROR",
        Some(DiagnosticSeverity::Warning) => "WARN",
        Some(DiagnosticSeverity::Information) => "INFO",
        Some(DiagnosticSeverity::Hint) => "HINT",
        None => "UNKNOWN",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Position as ProtoPosition, Range};

    fn diag(severity: DiagnosticSeverity, line: u32, message: &str) -> Diagnostic {
        Diagnostic {
            severity: Some(severity),
            range: Range {
                start: ProtoPosition { line, character: 0 },
                end: ProtoPosition { line, character: 0 },
            },
            message: message.to_string(),
            source: None,
            code: None,
        }
    }

    #[test]
    fn requested_paths_prefers_batch_then_single() {
        assert_eq!(
            requested_paths(&json!({ "paths": ["a.rs", "b.rs"] })),
            vec!["a.rs".to_string(), "b.rs".to_string()]
        );
        assert_eq!(requested_paths(&json!({ "file": "c.rs" })), vec!["c.rs".to_string()]);
        assert!(requested_paths(&json!({})).is_empty());
    }

    #[test]
    fn min_severity_maps_the_threshold() {
        assert_eq!(min_severity(Some("error")), 1);
        assert_eq!(min_severity(Some("warning")), 2);
        assert_eq!(min_severity(None), 4);
        assert_eq!(min_severity(Some("all")), 4);
    }

    #[test]
    fn format_file_filters_by_severity_and_counts() {
        let path = Path::new("/w/src/lib.rs");
        let diags = vec![
            diag(DiagnosticSeverity::Error, 0, "boom"),
            diag(DiagnosticSeverity::Warning, 4, "careful"),
            diag(DiagnosticSeverity::Hint, 9, "tidy"),
        ];
        let errors = format_file(path, &diags, min_severity(Some("error")), &[]);
        assert!(errors.contains("(1 found)"), "{errors}");
        assert!(errors.contains("[ERROR] 1:1: boom"), "{errors}");
        assert!(!errors.contains("careful"));
        let warns = format_file(path, &diags, min_severity(Some("warning")), &[]);
        assert!(warns.contains("(2 found)"), "{warns}");
        assert!(warns.contains("[WARN] 5:1: careful"), "{warns}");
        assert!(format_file(path, &diags, min_severity(None), &[]).contains("(3 found)"));
        assert!(format_file(path, &[], 4, &[]).contains("No diagnostics"));
    }

    #[test]
    fn format_file_surfaces_server_notices_only_when_empty() {
        let path = Path::new("/w/index.ts");
        let notices = vec!["[warning] bundled 1.0.0".to_string()];
        // Empty diagnostics -> the notice is surfaced so the result isn't a silent "clean".
        let empty = format_file(path, &[], 4, &notices);
        assert!(empty.contains("No diagnostics"), "{empty}");
        assert!(
            empty.contains("note: language server reported [warning] bundled 1.0.0"),
            "{empty}"
        );
        // With real diagnostics present, notices are not appended (they'd be noise).
        let with = format_file(path, &[diag(DiagnosticSeverity::Error, 0, "boom")], 4, &notices);
        assert!(with.contains("boom") && !with.contains("note:"), "{with}");
    }
}
