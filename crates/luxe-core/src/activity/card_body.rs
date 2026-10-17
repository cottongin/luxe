//! Card body — the multi-line payload a tool card renders under its header.
//!
//! Taken from the call's arguments, not its result, and redacted before it leaves core.

use super::bash_display::strip_redundant_cd;
use super::describe::{bash_description, web_search_queries};
use super::partial_json::PartialObject;
use super::ToolCardBody;
use std::path::Path;

/// The multi-line payload a tool card shows under its header, taken from the call's arguments.
///
/// A card that renders the *result* can only show what the model's token budget allowed: a capped
/// wall of `+` rows, with no language to highlight and no honest count of what was cut. The
/// arguments are the real payload and the front-end already has them.
///
/// Only tools whose payload *is* the point get one. A `bash` command earns a body only when it spans
/// lines — a one-liner is already on the header in full, and cards for it recur in bursts.
/// Everything else stays a single line until the reader asks for more.
///
/// Redacted here rather than at each call site, so no caller can forward a secret by forgetting to.
///
/// Args: `(tool, arguments, root)`, `root` being where a `bash` call's shell starts.
pub fn tool_card_body(
    tool: &str,
    arguments: &serde_json::Value,
    root: impl AsRef<Path>,
) -> Option<ToolCardBody> {
    use std::borrow::Cow;
    let root = root.as_ref();
    let text = |key: &str| arguments.get(key).and_then(serde_json::Value::as_str);
    let payload = match tool {
        // Described, the command is the detail under the header rather than the point of the card,
        // so it shows however short it is, folded to its first line until opened
        "bash" if bash_description(arguments).is_some() => {
            let command = strip_redundant_cd(text("command")?, root).trim_end_matches('\n');
            let redacted = crate::redact::redact_secrets(command, &[]);
            return (!redacted.trim().is_empty()).then(|| ToolCardBody {
                collapsed_lines: Some(1),
                ..ToolCardBody::capped(&redacted)
            });
        }
        "write" | "bash" => text(payload_key(tool)?).map(Cow::Borrowed),
        // One query is already on the header in full; several are a list, and which angles a
        // search covered is most of what there is to judge about it.
        "web_search" | "websearch" => {
            let queries = web_search_queries(arguments);
            (queries.len() > 1).then(|| Cow::Owned(queries.join("\n")))
        }
        _ => None,
    }?;
    shaped_card_body(tool, &payload, root)
}

/// The argument that *is* a tool's card body, for the tools whose payload arrives as one string.
///
/// Named separately from [`tool_card_body`] because the streaming path needs the key rather than
/// the finished value: the whole point there is to read a member that has not closed yet, which no
/// amount of JSON parsing will hand over.
fn payload_key(tool: &str) -> Option<&'static str> {
    match tool {
        "write" => Some("content"),
        "bash" => Some("command"),
        _ => None,
    }
}

/// A payload as the card draws it: shell-stripped, bounded, and with its secrets removed.
fn shaped_card_body(tool: &str, payload: &str, root: &Path) -> Option<ToolCardBody> {
    // Stripped before the newline test, not after: a one-liner that only spans lines because of
    // its own `cd` prefix has nothing under the header worth a body.
    let payload = match tool {
        "bash" => {
            let shown = strip_redundant_cd(payload, root);
            shown.trim_end().contains('\n').then_some(shown)?
        }
        _ => payload,
    };
    let redacted = crate::redact::redact_secrets(payload, &[]);
    let trimmed = redacted.trim_end_matches('\n');
    (!trimmed.trim().is_empty()).then(|| ToolCardBody::capped(trimmed))
}

/// The card body for a call the model is **still writing**, from the payload that has arrived.
///
/// The other half of [`super::describe_partial_call`], and the reason it cannot be done the same way: a
/// `write`'s payload is the member that closes *last*, so the honest "complete members only" rule
/// that names the call would never show a byte of the file. The point here is the opposite — to
/// watch it arrive — so this reads into the member still streaming and decodes as much of it as
/// forms a whole prefix, stopping before a `\u00` the model has not finished writing.
///
/// `total_lines` counts what has landed, so the card's `+N more lines` footer climbs as the file is
/// written. When the call is dispatched the same body is rebuilt from the committed arguments, and
/// the two agree — nothing on screen jumps.
///
/// Args: `(tool, partial_arguments, root)`.
pub fn partial_card_body(
    tool: &str,
    partial_arguments: &str,
    root: impl AsRef<Path>,
) -> Option<ToolCardBody> {
    let root = root.as_ref();
    let partial = PartialObject(partial_arguments);
    // A payload that has fully landed describes itself exactly; only fall back to the fragment.
    if let Some(arrived) = partial.complete_members() {
        if let Some(body) = tool_card_body(tool, &arrived, root) {
            return Some(body);
        }
    }
    let payload = partial.string_member(payload_key(tool)?)?;
    shaped_card_body(tool, &payload, root)
}

#[cfg(test)]
mod tests;
