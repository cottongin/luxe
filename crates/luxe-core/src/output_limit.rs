//! Output limits — how much of a tool's answer reaches the model, shared by the tools that answer.
//!
//! A tool result enters the conversation and stays there, re-read on every request until
//! compaction, so a single unbounded answer is a cost the whole rest of the session pays. The
//! ceiling lives here, in core, because the tools that answer live in two crates — `read` in
//! `luxe-tools`, the MCP gateway in `luxe-mcp` — and every tool should promise the same one.

/// The most characters one tool result puts into the conversation: about 15,000 tokens.
pub const RESULT_MAX_CHARS: usize = 60_000;

/// Caps output to `max_chars`, keeping the head and stating what was dropped.
///
/// The line cap is the usual instrument, but it is blind to a *minified* file: a bundle or a large
/// JSON blob is one enormous line, so a line-bounded read can still return half a megabyte.
pub fn cap_chars(text: &str, max_chars: usize, hint: &str) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars).collect();
    format!("{kept}\n\n… {} more characters not shown ({total} total). {hint}", total - max_chars)
}

/// The nearest char boundary at or before `index`, so a byte cut never splits a codepoint.
pub fn floor_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// The nearest char boundary at or after `index`.
pub fn ceil_boundary(text: &str, mut index: usize) -> usize {
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

/// `text` without its insignificant whitespace, when the whole of it is a JSON object or array.
///
/// Servers commonly answer in pretty-printed JSON, and the indentation is a third or more of what
/// the model is sent: 29% of a list of Asana-shaped tasks and 42% of one task, measured. The pass
/// is textual on purpose. Parsing into a value and printing it again would reorder keys, round an
/// integer past 2^53 and touch nothing that matters to the reader; this removes spaces, tabs and
/// line breaks outside strings and leaves every other byte exactly as the server sent it. Anything
/// that is not valid JSON is `None`, and passes through untouched.
pub fn compact_json(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return None;
    }
    serde_json::from_str::<serde::de::IgnoredAny>(trimmed).ok()?;
    let mut compact = String::with_capacity(trimmed.len());
    let (mut in_string, mut escaped) = (false, false);
    for character in trimmed.chars() {
        if in_string {
            compact.push(character);
            match character {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
        } else if character == '"' {
            in_string = true;
            compact.push(character);
        } else if !matches!(character, ' ' | '\t' | '\n' | '\r') {
            compact.push(character);
        }
    }
    Some(compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_chars_keeps_the_head_and_counts_what_it_dropped() {
        let capped = cap_chars(&"x".repeat(1000), 100, "Narrow the range.");
        assert!(capped.starts_with(&"x".repeat(100)));
        assert!(capped.contains("900 more characters not shown (1000 total). Narrow the range."));
        assert_eq!(cap_chars("short", 100, "hint"), "short");
    }

    /// The indentation goes; the order, the numbers and every character inside a string stay.
    #[test]
    fn compaction_removes_whitespace_and_nothing_else() {
        let pretty = r#"{
  "zeta": 1,
  "alpha": [ 18446744073709551617, 1.0e10, true, null ],
  "notes": "two  spaces,\ttab, \"quoted { braces }\" and a \\ backslash",
  "empty": { }
}"#;
        assert_eq!(
            compact_json(pretty).as_deref(),
            Some(
                r#"{"zeta":1,"alpha":[18446744073709551617,1.0e10,true,null],"notes":"two  spaces,\ttab, \"quoted { braces }\" and a \\ backslash","empty":{}}"#
            )
        );
        assert_eq!(compact_json("  [ 1 , 2 ]\n").as_deref(), Some("[1,2]"));
    }

    /// Only a whole JSON object or array is touched: prose, a bare value and broken JSON pass.
    #[test]
    fn anything_but_a_json_document_is_left_alone() {
        for text in [
            "Found 3 tasks:\n  - one\n  - two",
            "\"just a string\"",
            "42",
            "{ \"unterminated\": ",
            "[1, 2] and then prose",
            "",
        ] {
            assert_eq!(compact_json(text), None, "{text:?}");
        }
    }
}
