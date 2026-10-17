//! Debug Adapter Protocol message envelope and the payload types the tool formats.
//!
//! Only the shapes we read back are typed; request `arguments` are built as `serde_json::Value` in
//! the client. See the DAP spec: <https://microsoft.github.io/debug-adapter-protocol/specification>.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A DAP wire message (request, response, or event). Fields are optional so one type parses all
/// three variants coming from the adapter.
#[derive(Debug, Clone, Deserialize)]
pub struct ProtocolMessage {
    /// Adapter-assigned sequence number for this message.
    #[serde(default)]
    pub seq: i64,
    /// `"request"`, `"response"`, or `"event"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Response: the request seq this answers.
    #[serde(default)]
    pub request_seq: Option<i64>,
    /// Response: whether the request succeeded.
    #[serde(default)]
    pub success: Option<bool>,
    /// Response/request: the command name.
    #[serde(default)]
    pub command: Option<String>,
    /// Event: the event name.
    #[serde(default)]
    pub event: Option<String>,
    /// Response: a human-readable failure message.
    #[serde(default)]
    pub message: Option<String>,
    /// Response body / event body.
    #[serde(default)]
    pub body: Option<Value>,
}

/// A DAP thread (`threads` response).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    /// Adapter thread id.
    pub id: i64,
    pub name: String,
}

/// A source location referenced by a stack frame or breakpoint.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Source {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
}

/// A stack frame (`stackTrace` response).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackFrame {
    /// Frame id used to scope `scopes`/`evaluate`.
    pub id: i64,
    /// Frame (function) name.
    pub name: String,
    /// Source location, if resolved.
    #[serde(default)]
    pub source: Option<Source>,
    /// 1-based line.
    #[serde(default)]
    pub line: i64,
    /// 1-based column.
    #[serde(default)]
    pub column: i64,
}

/// A variable scope (`scopes` response).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scope {
    /// Scope name (e.g. "Locals").
    pub name: String,
    /// Reference for a `variables` request.
    #[serde(rename = "variablesReference")]
    pub variables_reference: i64,
    /// Whether retrieving this scope is expensive.
    #[serde(default)]
    pub expensive: bool,
    /// What the scope holds (`arguments`, `locals`, `registers`), when the adapter says
    #[serde(rename = "presentationHint", default)]
    pub presentation_hint: Option<String>,
}

/// A variable (`variables` / `evaluate` response).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    /// Rendered value.
    pub value: String,
    /// Declared type, if the adapter provides it.
    #[serde(rename = "type", default)]
    pub type_name: Option<String>,
    /// Non-zero when the variable is structured (expandable).
    #[serde(rename = "variablesReference", default)]
    pub variables_reference: i64,
}

/// A breakpoint verification result (`setBreakpoints` response).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Breakpoint {
    /// Whether the adapter bound the breakpoint.
    #[serde(default)]
    pub verified: bool,
    /// Resolved line, if the adapter moved it.
    #[serde(default)]
    pub line: Option<i64>,
    /// Explanation when unverified.
    #[serde(default)]
    pub message: Option<String>,
}

/// Body of a `stopped` event.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StoppedBody {
    /// Why execution stopped (e.g. "breakpoint", "step", "exception").
    #[serde(default)]
    pub reason: String,
    /// Thread that stopped, if reported.
    #[serde(rename = "threadId", default)]
    pub thread_id: Option<i64>,
    #[serde(default)]
    pub description: Option<String>,
    /// Extra detail (e.g. exception text).
    #[serde(default)]
    pub text: Option<String>,
}
