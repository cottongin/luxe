//! Results — what a tool's output becomes before the model and the transcript see it.

use super::*;

/// Scrub secret-shaped strings from a tool's text output before anything downstream sees it.
///
/// This is the one place tool output is redacted for persistence: the session transcript,
/// the `ToolEnded` event, command hooks, and the model's own continued context all consume
/// the result after this runs. Patterns plus every literal secret the process resolved;
/// inline images pass through so vision tools like `screenshot` still work.
pub(super) fn redact_tool_result(result: &mut ToolResultData) {
    use crate::message::ToolOutput;
    for block in &mut result.content {
        match block {
            ContentBlock::Text(t) => *t = crate::redact::process_secrets().redact(t),
            ContentBlock::ToolResult { content, .. } => {
                for out in content {
                    if let ToolOutput::Text(t) = out {
                        *t = crate::redact::process_secrets().redact(t);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Flatten a tool's content blocks into `ToolOutput`s for a `ToolResult`. Text and inline images
/// pass through (the latter lets tools like `screenshot` return pixels to vision models); nested
/// tool results collapse to text and anything else becomes a text placeholder.
///
/// A tool that returned nothing says so in words: an empty string is one Anthropic rejects, and a
/// result cannot simply be left out, because the call it answers would then be the orphan.
pub(super) fn tool_output_from_content(blocks: &[ContentBlock]) -> Vec<crate::message::ToolOutput> {
    use crate::message::ToolOutput;
    if blocks.is_empty() {
        return vec![ToolOutput::Text(crate::empty_blocks::NO_OUTPUT.to_string())];
    }
    blocks
        .iter()
        .map(|b| match b {
            ContentBlock::Text(t) => ToolOutput::Text(t.clone()),
            ContentBlock::Image { data, mime, name } => {
                ToolOutput::Image { data: data.clone(), mime: mime.clone(), name: name.clone() }
            }
            ContentBlock::ToolResult { content, .. } => ToolOutput::Text(
                content
                    .iter()
                    .map(|o| match o {
                        ToolOutput::Text(t) => t.clone(),
                        ToolOutput::Image { .. } => "[image]".to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => ToolOutput::Text("[non-text]".to_string()),
        })
        .collect()
}
