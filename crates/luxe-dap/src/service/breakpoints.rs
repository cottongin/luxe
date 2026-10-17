//! Breakpoints — setting them, and saying which ones did not take.

use super::{FunctionBreakpoint, SourceBreakpoint, CONTROL_TIMEOUT};
use crate::client::{DapClient, DapError};
use crate::protocol::Breakpoint;
use serde_json::{json, Value};

/// Send a `setBreakpoints` request for one source file (with optional conditions) and parse the
/// results.
pub async fn set_breakpoints(
    client: &DapClient,
    path: &str,
    breakpoints: &[SourceBreakpoint],
) -> Result<Vec<Breakpoint>, DapError> {
    let breakpoints: Vec<Value> = breakpoints
        .iter()
        .map(|bp| match &bp.condition {
            Some(condition) => json!({ "line": bp.line, "condition": condition }),
            None => json!({ "line": bp.line }),
        })
        .collect();
    let body = client
        .request(
            "setBreakpoints",
            json!({
                "source": { "path": path },
                "breakpoints": breakpoints,
                "sourceModified": false,
            }),
            CONTROL_TIMEOUT,
        )
        .await?;
    Ok(parse_array(&body, "breakpoints"))
}

/// Send a `setFunctionBreakpoints` request (break on entry to named functions).
pub async fn set_function_breakpoints(
    client: &DapClient,
    breakpoints: &[FunctionBreakpoint],
) -> Result<Vec<Breakpoint>, DapError> {
    let breakpoints: Vec<Value> = breakpoints
        .iter()
        .map(|bp| match &bp.condition {
            Some(condition) => json!({ "name": bp.name, "condition": condition }),
            None => json!({ "name": bp.name }),
        })
        .collect();
    let body = client
        .request("setFunctionBreakpoints", json!({ "breakpoints": breakpoints }), CONTROL_TIMEOUT)
        .await?;
    Ok(parse_array(&body, "breakpoints"))
}

/// Parse a typed array out of a response body field, dropping malformed entries.
pub(super) fn parse_array<T: serde::de::DeserializeOwned>(body: &Value, field: &str) -> Vec<T> {
    body.get(field)
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect())
        .unwrap_or_default()
}

/// Each requested breakpoint in `path` the adapter did not bind, with its reason.
///
/// Answers line up with requests by position, as the protocol says; a request with no answer at
/// all was not bound either.
pub(super) fn unverified(
    path: &str,
    requested: &[SourceBreakpoint],
    bound: &[Breakpoint],
) -> Vec<String> {
    requested
        .iter()
        .enumerate()
        .filter_map(|(index, request)| match bound.get(index) {
            Some(answer) if answer.verified => None,
            Some(answer) => {
                let why = answer.message.as_deref().unwrap_or("not verified by the adapter");
                Some(format!("{path}:{}: {why}", request.line))
            }
            None => Some(format!("{path}:{}: the adapter did not answer for it", request.line)),
        })
        .collect()
}
