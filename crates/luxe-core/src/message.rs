//! Conversation content model — messages, roles, and content blocks.
//!
//! Mirrors the shape used by real LLM providers so adapter code in later phases
//! can map to/from this without loss. No provider-specific fields live here.

use serde::{Deserialize, Serialize};

/// Message role: who produced this message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A single piece of content inside a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text.
    Text(String),
    /// Reasoning/thinking output. `text` is the human-readable reasoning (or provider summary);
    /// the remaining fields are opaque replay tokens that must be round-tripped **unmodified** or
    /// the provider rejects the block. `signature` is Anthropic's attestation; `id` +
    /// `encrypted` are the OpenAI Responses reasoning item id (`rs_...`) and its
    /// `encrypted_content`; `opaque` is the `reasoning_opaque` GitHub Copilot's chat completions
    /// hands back. Only the field(s) for the originating provider are set, and each adapter reads
    /// only its own, so one provider's token never reaches another that would reject it.
    Thinking {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encrypted: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        opaque: Option<String>,
    },
    /// Encrypted reasoning Anthropic chose not to expose. `data` is opaque and, like a signed
    /// thinking block, must be replayed verbatim during a tool-use turn.
    RedactedThinking { data: String },
    /// A tool call from the model — id names the call, name is the tool, arguments are JSON.
    ToolCall { id: String, name: String, arguments: serde_json::Value },
    /// Result from a tool invocation (text or image; `is_error` flags failure).
    ToolResult { call_id: String, content: Vec<ToolOutput>, is_error: bool },
    /// Inline image (e.g. from clipboard paste). Base64-encoded with MIME type.
    ///
    /// `name` is where the image came from — the file `read` opened, the path `screenshot` wrote,
    /// the filename that was pasted. Optional and additive, so an older transcript still loads. It
    /// is provenance, not a capability: a front-end offering to *open* it has to establish for
    /// itself that the path exists and is an image, because a model chose the string.
    Image {
        data: String,
        mime: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

/// A single output piece inside a `ToolResult`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content", rename_all = "snake_case")]
pub enum ToolOutput {
    Text(String),
    /// Base64-encoded image payload. `name` carries the provenance described on
    /// [`ContentBlock::Image`], with the same caveat.
    Image {
        data: String,
        mime: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

impl ToolOutput {
    /// The prose of a result's outputs, joined by newlines, with the images left out.
    ///
    /// Empty when the result is all pixels, so a caller that has to say something filters on that.
    pub fn joined_text(outputs: &[ToolOutput]) -> String {
        outputs
            .iter()
            .filter_map(|output| match output {
                ToolOutput::Text(text) => Some(text.as_str()),
                ToolOutput::Image { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The images of a result's outputs, as `(mime, base64 data)`.
    pub fn images(outputs: &[ToolOutput]) -> impl Iterator<Item = (&str, &str)> {
        outputs.iter().filter_map(|output| match output {
            ToolOutput::Image { mime, data, .. } => Some((mime.as_str(), data.as_str())),
            ToolOutput::Text(_) => None,
        })
    }
}

/// A message from one role with zero or more content blocks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    /// The message's prose, with its text blocks joined by newlines.
    ///
    /// Reasoning, tool calls and images are skipped — this answers "what does this message *say*",
    /// which is what a resend, a transcript search, and the turn-provenance summary each want.
    /// Empty when the message carries no text at all, so a caller that means "said something"
    /// filters on `!text.trim().is_empty()`.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Convenience: build a single-text user message.
    pub fn user(text: impl Into<String>) -> Self {
        Self { role: Role::User, content: vec![ContentBlock::Text(text.into())] }
    }

    /// Convenience: build a single-text assistant reply.
    pub fn assistant(text: impl Into<String>) -> Self {
        Self { role: Role::Assistant, content: vec![ContentBlock::Text(text.into())] }
    }

    /// Convenience: build a tool-result message for a given call id.
    pub fn tool_result(call_id: &str, text: impl Into<String>, is_error: bool) -> Self {
        Self {
            role: Role::Tool,
            content: vec![ContentBlock::ToolResult {
                call_id: call_id.to_string(),
                content: vec![ToolOutput::Text(text.into())],
                is_error,
            }],
        }
    }
}

#[cfg(test)]
mod tests;
