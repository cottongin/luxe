//! ast-grep output — what a call reports, and what an applied rewrite tells the rest of luxe.

use super::*;

pub(super) fn search_outcome(
    found: Result<Vec<AstGrepMatch>, AstGrepError>,
) -> luxe_core::ToolResultData {
    match found {
        Ok(matches) => ToolResultData::text(render_matches(&matches, false)),
        Err(e) => ToolResultData::error(format!("ast-grep search error: {e}")),
    }
}

/// Report a replace, first announcing each file an applied rewrite changed.
pub(super) async fn finish_replace(
    tool: &AstGrepTool,
    found: Result<Vec<AstGrepMatch>, AstGrepError>,
    applied: bool,
    cx: &ToolContext,
) -> luxe_core::ToolResultData {
    let injected = match (&found, applied) {
        (Ok(matches), true) => announce(tool, matches, cx).await,
        _ => Vec::new(),
    };
    let mut result = replace_outcome(found, applied);
    if !injected.is_empty() {
        let diagnostics = format!("\n--- Diagnostics ---\n{}", injected.join("\n"));
        result.content.push(luxe_core::ContentBlock::Text(diagnostics));
    }
    result
}

/// `FileChanged` and the read tracker for every rewritten file, as an `edit` would get.
async fn announce(tool: &AstGrepTool, matches: &[AstGrepMatch], cx: &ToolContext) -> Vec<String> {
    let mut files: Vec<&PathBuf> = matches.iter().map(|item| &item.file).collect();
    files.sort();
    files.dedup();
    let mut injected = Vec::new();
    for file in files {
        let path = cx.cwd.join(file);
        injected.extend(cx.file_changed(&path, "edit").await);
        if let Some(tracker) = &tool.tracker {
            tracker.record(&cx.session_id, &path);
        }
    }
    injected
}

fn replace_outcome(
    found: Result<Vec<AstGrepMatch>, AstGrepError>,
    applied: bool,
) -> luxe_core::ToolResultData {
    match found {
        Ok(matches) => {
            let mut body = render_matches(&matches, applied);
            if !applied && !matches.is_empty() {
                body.push_str("\npreview only — re-run with apply=true to write");
            }
            ToolResultData::text(body)
        }
        Err(e) => ToolResultData::error(format!("ast-grep replace error: {e}")),
    }
}

/// Render matches as a compact, model-friendly listing (not raw JSON).
pub(super) fn render_matches(matches: &[AstGrepMatch], applied: bool) -> String {
    if matches.is_empty() {
        return "no matches".to_string();
    }
    let mut out = String::new();
    let verb = if applied { "rewrote" } else { "found" };
    out.push_str(&format!("{verb} {} match(es)\n", matches.len()));
    for item in matches {
        out.push_str(&format!(
            "{}:{}:{}\n",
            luxe_core::paths::display(&item.file),
            item.line,
            item.column
        ));
        out.push_str(&excerpt(&item.text, "  - "));
        if let Some(replacement) = &item.replacement {
            out.push_str(&excerpt(replacement, "  + "));
        }
    }
    // Every other search tool bounds what it hands back; this one did not, and a structural
    // pattern matching whole function bodies across a large repo produced a payload that
    // *exceeded the model's context window* and killed the turn outright. A search must degrade
    // to "here is the head, narrow it" — never to a dead turn.
    crate::output_limit::cap_lines(
        &out,
        MAX_MATCH_LINES,
        "lines",
        "Narrow the pattern or pass `paths` to scope the search.",
    )
}

/// One match's source, capped: a structural pattern can match an entire function body, and the
/// opening lines are what identify it.
fn excerpt(text: &str, prefix: &str) -> String {
    let mut out = String::new();
    let total = text.lines().count();
    for line in text.lines().take(MAX_MATCH_BODY_LINES) {
        out.push_str(&format!("{prefix}{line}\n"));
    }
    if total > MAX_MATCH_BODY_LINES {
        out.push_str(&format!("{prefix}… ({} more lines)\n", total - MAX_MATCH_BODY_LINES));
    }
    out
}
