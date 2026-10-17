//! Tool call — one call as it streams in, and reading its argument buffer.

/// Internal tracking for a tool call while its arguments stream in.
#[derive(Clone)]
pub(crate) struct ToolCallInfo {
    /// Stream index used to route argument deltas to the right call.
    pub(in crate::engine) index: usize,
    /// Provider-assigned call id, echoed back on the matching `ToolResult`.
    pub(in crate::engine) id: String,
    pub(in crate::engine) name: String,
    pub(in crate::engine) arguments: String,
}

/// Why a call's arguments are not a JSON value, or `None` when they are or there are none.
pub(crate) fn malformed_arguments(raw: &str) -> Option<serde_json::Error> {
    if raw.trim().is_empty() {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(raw).err()
}

/// Parse a streamed argument buffer into JSON, defaulting to `{}` when the model sent
/// nothing or malformed JSON (some models emit empty arguments for no-arg tools).
pub(crate) fn parse_tool_arguments(raw: &str) -> serde_json::Value {
    if raw.trim().is_empty() {
        return serde_json::json!({});
    }
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::json!({}))
}
