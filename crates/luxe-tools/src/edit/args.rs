//! Argument normalization for `edit` — permissive at the edge, strict in the core.
//!
//! The tool's real invariants are uniqueness, atomicity and the read guard. The *spelling* of its
//! arguments is not one of them, and enforcing it costs a turn every time: Claude Code trained a
//! generation of models on `file_path`/`old_string`/`new_string`, OpenCode on
//! `filePath`/`oldString`/`newString`, and rejecting either teaches nothing, because the reply that
//! comes back is a retry, not a correction. So: accept the dialects, normalize once, then be strict
//! about what actually matters. The schema still advertises exactly one spelling, so nothing here
//! is a second contract to keep.
//!
//! # Why the advertised spelling is `oldText`, not `old_text`
//!
//! Advertised as `old_text`/`new_text`, a large share of Sonnet's edit calls arrive with `edits` as
//! a mangled *string* beginning `[{"new_text">`: a separator emitted as `">` instead of `": "`
//! right after a key ending in `_text`, and nowhere else. The camelCase spelling all but removes
//! it. The provider's tolerant JSON finalizer then cannot parse the array, so it stringifies it up
//! to the point it can resync and *promotes the surviving object members into the outer arguments
//! object*: `{"edits": "[{\"new_text\">…", "old_text": "…", "path": "…"}`, one field swallowed,
//! its sibling hoisted, and `path` present only when the model happened to emit it first.
//!
//! So the aliases below are a *recovery* path, not a fix: the corruption happens before any of
//! this code runs. The error messages speak the advertised spelling, because they are what
//! re-primes the format, and a call after a clean one is far likelier to garble than the retry
//! after a refusal. And every failure quotes what arrived: a model sees only the arguments it meant
//! to send, so an error naming keys is useless once the *value* is what went wrong.
//!
//! # The one garble that is read rather than refused
//!
//! `oldText` is not immune. claude-sonnet-5-5 and claude-haiku-5-5 emit `[{"oldText">` too, and
//! with Anthropic validating the arguments the result is always the same shape: `edits` is the
//! string `[{"oldText">` followed by the search text, whole, and `newText` is hoisted beside it,
//! whole, and retrying mostly reproduces it. Both halves arrive intact, so [`recover_hoisted`]
//! reads that one shape as the single edit it was, and nothing else: anything missing either half
//! is still refused. It writes no guess, since the search text must still match the file exactly
//! and only once.
//!
//! Two rarer forms of the same slip come back whole too, and are read the same way: the opener
//! written as the model's own `<parameter name="oldText">`, and the whole array kept inside the
//! string with only the separators garbled ([`repair_separators`]).

use serde_json::Value;
use std::borrow::Cow;

/// One normalized replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditSpec {
    /// Text to find.
    pub(crate) old_text: String,
    /// Text to put in its place.
    pub(crate) new_text: String,
    /// Replace every occurrence rather than requiring exactly one.
    pub(crate) replace_all: bool,
}

/// A normalized `edit` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditRequest {
    /// File to edit, as given.
    pub(crate) path: String,
    /// Replacements, in call order.
    pub(crate) edits: Vec<EditSpec>,
}

/// Why a call could not be understood, with enough detail to fix it in one move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArgsError {
    /// No recognizable path key.
    MissingPath { received: Vec<String> },
    /// No recognizable edits, and no single top-level edit either.
    MissingEdits { received: Vec<String> },
    /// `edits` was present but empty.
    EmptyEdits,
    /// `edits` arrived as something no edit can be read out of.
    ///
    /// Args: `(key as spelled, JSON type that arrived, head of the value)`.
    UnusableEdits { key: &'static str, found: &'static str, excerpt: String },
    /// One edit lacked a find or replace value. `index` is `None` for a top-level single edit.
    EditField { index: Option<usize>, field: &'static str, received: Vec<String> },
}

impl ArgsError {
    /// A message that names what arrived, what was expected, and the shape to send.
    pub(super) fn message(&self) -> String {
        match self {
            Self::MissingPath { received } => format!(
                "Error: no file path. Received keys: {}. Use `path` (aliases: `file_path`, \
                 `filePath`, `file`).\n{USAGE}",
                render_keys(received)
            ),
            Self::MissingEdits { received } => format!(
                "Error: no edits. Received keys: {}. Pass `edits` as an array of \
                 {{oldText, newText}}, or a single `oldText`/`newText` pair at the top \
                 level.\n{USAGE}",
                render_keys(received)
            ),
            Self::EmptyEdits => {
                format!("Error: `edits` was empty — there is nothing to apply.\n{USAGE}")
            }
            Self::UnusableEdits { key, found, excerpt } => format!(
                "Error: `{key}` arrived as {found}, not an array of {{oldText, newText}}, so no \
                 edit could be read out of it. It begins:\n  {excerpt}\nSend `{key}` as a real \
                 JSON array — a quoted one is accepted only when the whole array parses. Nothing \
                 was written.\n{USAGE}"
            ),
            Self::EditField { index: Some(index), field, received } => format!(
                "Error: edit[{index}] has no `{field}`. Received keys: {}.\n{USAGE}",
                render_keys(received)
            ),
            // Named apart from the indexed case because the two are corrected differently, and a
            // model told `edit[0]` is missing a field it can see at the top level starts guessing
            // at key order instead.
            Self::EditField { index: None, field, received } => format!(
                "Error: this call passed one edit at the top level rather than an `edits` array, \
                 and it has no `{field}`. Received keys: {}.\n{USAGE}",
                render_keys(received)
            ),
        }
    }
}

/// The canonical shape, repeated on every failure so a correction needs no second look.
///
/// This string is not decoration. It is the demonstration that re-primes the format for the next
/// turn, so it has to spell every field the way [`super::edit_schema`] advertises it;
/// `the_shape_a_refusal_shows_is_the_shape_the_schema_asks_for` holds the two together.
const USAGE: &str = "Expected: {\"path\": \"src/lib.rs\", \"edits\": [{\"oldText\": \"…\", \
                     \"newText\": \"…\", \"replaceAll\": false}]}";

/// Keys accepted for the file path, canonical first.
const PATH_KEYS: [&str; 4] = ["path", "file_path", "filePath", "file"];
/// Keys accepted for the text being replaced, canonical first.
const OLD_KEYS: [&str; 6] = ["oldText", "old_text", "old_string", "oldString", "old", "search"];
/// Keys accepted for the replacement text, canonical first.
const NEW_KEYS: [&str; 6] = ["newText", "new_text", "new_string", "newString", "new", "replace"];
/// Keys accepted for the replace-every-occurrence flag, canonical first.
const ALL_KEYS: [&str; 3] = ["replaceAll", "replace_all", "all"];
/// Keys accepted for the list of edits.
const EDITS_KEYS: [&str; 3] = ["edits", "replacements", "changes"];

/// Every spelling [`normalize`] reads, for [`luxe_core::Tool::argument_aliases`].
pub(super) const ALIASES: [&str; 22] = [
    "path",
    "file_path",
    "filePath",
    "file",
    "oldText",
    "old_text",
    "old_string",
    "oldString",
    "old",
    "search",
    "newText",
    "new_text",
    "new_string",
    "newString",
    "new",
    "replace",
    "replaceAll",
    "replace_all",
    "all",
    "edits",
    "replacements",
    "changes",
];

/// Normalizes whatever dialect arrived into one [`EditRequest`].
pub(crate) fn normalize(args: &Value) -> Result<EditRequest, ArgsError> {
    let keys = keys_of(args);
    let path = first_string(args, &PATH_KEYS)
        .ok_or_else(|| ArgsError::MissingPath { received: keys.clone() })?;
    if let Some(edit) = recover_hoisted(args) {
        return Ok(EditRequest { path, edits: vec![edit] });
    }

    // `edits` as an array is the documented shape; a lone object is the same call with one edit,
    // and a top-level `oldText`/`newText` pair is how a single-edit tool would have been called.
    let listed = listed_edits(args)?;
    let flat = OLD_KEYS.iter().any(|key| args.get(*key).is_some());

    // Paired with the array index the entry came from. `None` marks the pair passed at the top
    // level, which is corrected differently and so is never reported as `edit[n]`.
    let mut entries: Vec<(Option<usize>, &Value)> = match listed.as_deref() {
        Some(Value::Array(items)) => {
            items.iter().enumerate().map(|(at, e)| (Some(at), e)).collect()
        }
        Some(object) => vec![(Some(0), object)],
        None if flat => vec![(None, args)],
        // The key was there and carried nothing. Saying "no edits" while listing `edits` among the
        // keys received is the kind of contradiction a model argues with instead of fixing.
        None if EDITS_KEYS.iter().any(|key| args.get(*key).is_some()) => {
            return Err(ArgsError::EmptyEdits)
        }
        None => return Err(ArgsError::MissingEdits { received: keys }),
    };
    // A pair at the top level *beside* a usable array is one more edit, not a restatement of one.
    // Ignoring it applied fewer edits than were asked for and reported success anyway, which is
    // the one outcome worse than a refusal. Appending is safe because two edits over the same
    // text is exactly what `super::plan`'s overlap check exists to catch.
    if listed.is_some() && flat {
        entries.push((None, args));
    }
    if entries.is_empty() {
        return Err(ArgsError::EmptyEdits);
    }

    let mut edits = Vec::with_capacity(entries.len());
    for (at, entry) in entries {
        let received = keys_of(entry);
        let old_text = first_string(entry, &OLD_KEYS).ok_or(ArgsError::EditField {
            index: at,
            field: "oldText",
            received: received.clone(),
        })?;
        // An empty replacement is a deletion, so `newText: ""` is legitimate — but a *missing*
        // one is a truncated call, and applying it would silently delete the match.
        let new_text = first_string_allowing_empty(entry, &NEW_KEYS)
            .ok_or(ArgsError::EditField { index: at, field: "newText", received })?;
        let replace_all = ALL_KEYS
            .iter()
            .find_map(|key| entry.get(*key))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        edits.push(EditSpec { old_text, new_text, replace_all });
    }
    Ok(EditRequest { path, edits })
}

/// The `edits` value as JSON, recovering the quoted form and refusing the rest *by name*.
///
/// The refusal is load-bearing rather than pedantic: the mangled string this rejects arrives with
/// its sibling field hoisted beside it (see the module docs), and a partial reconstruction would
/// write a guess at the replacement into a real file.
///
/// A JSON-encoded string is not a dialect — it is the documented array with one accidental layer of
/// quoting, which every model produces eventually, so it is unwrapped and used. Anything else is
/// refused here rather than left to fall through to the single-edit path, which would read the
/// *whole call* as `edit[0]` and report a field missing from it: true, and useless while the
/// corruption sits in `edits`.
///
/// A garbled string is read only where nothing has to be guessed: a whole array with garbled
/// separators ([`repair_separators`]), and the hoisted shape, read before this runs
/// ([`recover_hoisted`]). Anything else would write a guess at the caller's meaning into a file.
fn listed_edits(args: &Value) -> Result<Option<Cow<'_, Value>>, ArgsError> {
    let Some((key, value)) = EDITS_KEYS.iter().find_map(|key| args.get(*key).map(|v| (*key, v)))
    else {
        return Ok(None);
    };
    match value {
        Value::Array(_) | Value::Object(_) => Ok(Some(Cow::Borrowed(value))),
        // Absent in all but spelling.
        Value::Null => Ok(None),
        Value::String(text) if text.trim().is_empty() => Ok(None),
        Value::String(text) => match serde_json::from_str::<Value>(text.trim()) {
            Ok(parsed @ (Value::Array(_) | Value::Object(_))) => Ok(Some(Cow::Owned(parsed))),
            _ => match repair_separators(text) {
                Some(parsed) => Ok(Some(Cow::Owned(parsed))),
                None => {
                    Err(ArgsError::UnusableEdits { key, found: "a string", excerpt: excerpt(text) })
                }
            },
        },
        other => Err(ArgsError::UnusableEdits {
            key,
            found: json_kind(other),
            excerpt: excerpt(&other.to_string()),
        }),
    }
}

/// The single edit inside the provider's mangling of `[{"oldText">…`, or `None` for anything else.
///
/// The shape is exact: `edits` a string opening with a search key and `">`, the rest of it the
/// search text, and the replacement under its own key at the top level. The mirror image, with the
/// replacement swallowed and the search text hoisted, is read the same way.
fn recover_hoisted(args: &Value) -> Option<EditSpec> {
    let garbled = EDITS_KEYS.iter().find_map(|key| args.get(*key)).and_then(Value::as_str)?;
    let (key, swallowed) = garbled_opener(garbled)?;
    let (old_text, new_text) = if OLD_KEYS.contains(&key) {
        (swallowed.to_string(), first_string_allowing_empty(args, &NEW_KEYS)?)
    } else if NEW_KEYS.contains(&key) {
        (first_string(args, &OLD_KEYS)?, swallowed.to_string())
    } else {
        return None;
    };
    // An empty search would match everywhere, the same rule `normalize` holds
    (!old_text.is_empty()).then(|| EditSpec {
        old_text,
        new_text,
        replace_all: ALL_KEYS
            .iter()
            .find_map(|key| args.get(*key))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// The key a garbled `edits` string opens with and everything after it: `[{"oldText">…` or the
/// model's own `<parameter name="oldText">…`.
fn garbled_opener(garbled: &str) -> Option<(&str, &str)> {
    let start = garbled.trim_start();
    if let Some(tagged) = start.strip_prefix("<parameter name=\"") {
        return tagged.split_once("\">");
    }
    let opened = start.strip_prefix('[').unwrap_or(start);
    opened.trim_start().strip_prefix('{')?.trim_start().strip_prefix('"')?.split_once("\">")
}

/// A JSON array or object kept whole inside an `edits` string, its separators written as `">`
/// after a search or replacement key, read with those separators put back.
///
/// Only the separator right after one of those keys is touched, and the result must then parse as
/// JSON, so anything else garbled is still refused. Raw line breaks inside the strings are escaped
/// first, because the same slip writes them unescaped.
fn repair_separators(text: &str) -> Option<Value> {
    let mut repaired = text.replace('\n', "\\n").replace('\t', "\\t");
    let mut touched = false;
    for key in OLD_KEYS.iter().chain(NEW_KEYS.iter()) {
        let garbled = format!("\"{key}\">");
        if repaired.contains(&garbled) {
            repaired = repaired.replace(&garbled, &format!("\"{key}\":\""));
            touched = true;
        }
    }
    let parsed = serde_json::from_str::<Value>(repaired.trim()).ok().filter(|_| touched)?;
    matches!(parsed, Value::Array(_) | Value::Object(_)).then_some(parsed)
}

/// The JSON type that arrived, spelled the way an error message wants to read.
fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// The head of a value that could not be used, as one line.
///
/// The load-bearing part of an unusable-value error: the sender is working from what it *meant* to
/// send, so the only thing that ends the retry loop is seeing the corruption itself.
fn excerpt(text: &str) -> String {
    const MAX_CHARS: usize = 80;
    let trimmed = text.trim_start();
    let first_line = trimmed.lines().next().unwrap_or_default();
    let mut head: String = first_line.chars().take(MAX_CHARS).collect();
    if head.len() < trimmed.len() {
        head.push('…');
    }
    head
}

fn keys_of(value: &Value) -> Vec<String> {
    value.as_object().map(|map| map.keys().cloned().collect()).unwrap_or_default()
}

fn render_keys(keys: &[String]) -> String {
    if keys.is_empty() {
        return "(none)".to_string();
    }
    keys.join(", ")
}

/// The first alias present and non-empty.
///
/// The non-empty part is load-bearing for `oldText`, and not merely tidiness: an empty needle
/// matches at *every* byte boundary in the file (`"abc".match_indices("")` yields four hits), so an
/// edit that reached [`crate::edit::locate`] with one and `replaceAll` set would interleave the
/// replacement between every character and write back a file orders of magnitude larger than the
/// one it read. Rejecting it here as a missing field is what keeps that unreachable.
fn first_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}

/// The first alias present, empty string included.
fn first_string_allowing_empty(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| value.get(*key).and_then(Value::as_str)).map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests;
