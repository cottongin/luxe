//! standards — the size limits the system prompt sets, held at the moment an edit crosses one.
//!
//! The prompt asks for functions of 4–20 lines and files of 500 or less, and a model agrees and
//! then writes a 60-line function anyway, because nothing tells it while it can still act. This
//! tells it, once, when a change takes a file or a function past a limit. Only crossings count:
//! a codebase full of long functions the agent did not write is not news on every edit, the same
//! rule the diagnostics after an edit follow. The outline comes from outside `core` (tree-sitter
//! lives in `luxe-lsp`), injected like the test-gap advisor.

use std::path::Path;

const MAX_FUNCTION_LINES: usize = 20;

const MAX_FILE_LINES: usize = 500;

/// Most functions named in one note; the rest are counted.
const MAX_NAMED: usize = 5;

/// Where function boundaries come from: a parser outside `core`.
pub trait OutlineSource: Send + Sync {
    /// The functions and methods in `source`, or `None` for a language it cannot parse.
    fn functions(&self, path: &Path, source: &str) -> Option<Vec<FunctionSize>>;

    /// The named scopes in `source` that can hold code: functions, methods, types and modules,
    /// not fields or constants. `None` for a language it cannot parse, which is also the default.
    /// A search labels what it found with the scopes around it.
    fn scopes(&self, _path: &Path, _source: &str) -> Option<Vec<Scope>> {
        None
    }
}

/// One named scope and the lines it spans, 0-based and inclusive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    /// The bare name, `delete` or `QuerySet`
    pub name: String,
    /// Its first line, where its declaration starts
    pub start_line: usize,
    /// Its last line
    pub end_line: usize,
}

/// One function's name, qualified by its container where it has one, and its length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSize {
    /// `Class.method`, or a bare name at the top level
    pub name: String,
    /// Lines from its first to its last, inclusive
    pub lines: usize,
}

/// A file's size and its functions past the limit, at one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct SizeSnapshot {
    file_lines: usize,
    long: Vec<FunctionSize>,
}

/// What `source` measures now.
pub(super) fn snapshot(outline: &dyn OutlineSource, path: &Path, source: &str) -> SizeSnapshot {
    let long = outline
        .functions(path, source)
        .unwrap_or_default()
        .into_iter()
        .filter(|function| function.lines > MAX_FUNCTION_LINES)
        .collect();
    SizeSnapshot { file_lines: source.lines().count(), long }
}

/// A note naming each limit `after` crosses that `before` had not, or `None` when it crosses none.
pub(super) fn crossings(path: &str, before: &SizeSnapshot, after: &SizeSnapshot) -> Option<String> {
    let mut parts = Vec::new();
    if after.file_lines > MAX_FILE_LINES && before.file_lines <= MAX_FILE_LINES {
        parts.push(format!(
            "{path} is now {} lines, past the {MAX_FILE_LINES}-line standard.",
            after.file_lines
        ));
    }
    let grown: Vec<&FunctionSize> = after
        .long
        .iter()
        .filter(|function| !before.long.iter().any(|old| old.name == function.name))
        .collect();
    if !grown.is_empty() {
        let named: Vec<String> = grown
            .iter()
            .take(MAX_NAMED)
            .map(|function| format!("`{}` ({} lines)", function.name, function.lines))
            .collect();
        let more = grown.len().saturating_sub(named.len());
        let tail = if more > 0 { format!(" and {more} more") } else { String::new() };
        parts.push(format!(
            "Past the {MAX_FUNCTION_LINES}-line function standard now: {}{tail}.",
            named.join(", ")
        ));
    }
    if parts.is_empty() {
        return None;
    }
    parts.push("Split them unless this project's own conventions say otherwise.".to_string());
    Some(parts.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads `name lines` pairs, one per line, as the outline.
    struct Listed;

    impl OutlineSource for Listed {
        fn functions(&self, _path: &Path, source: &str) -> Option<Vec<FunctionSize>> {
            Some(
                source
                    .lines()
                    .filter_map(|line| line.strip_prefix("fn "))
                    .filter_map(|rest| rest.split_once(' '))
                    .map(|(name, lines)| FunctionSize {
                        name: name.to_string(),
                        lines: lines.parse().unwrap_or(0),
                    })
                    .collect(),
            )
        }
    }

    fn measure(source: &str) -> SizeSnapshot {
        snapshot(&Listed, Path::new("x.rs"), source)
    }

    fn padded(functions: &str, total: usize) -> String {
        let mut text = functions.to_string();
        while text.lines().count() < total {
            text.push('\n');
        }
        text
    }

    /// A function that grows past 20 lines is named; one that was already past it is not.
    #[test]
    fn a_function_crossing_the_limit_is_named_and_an_old_one_is_not() {
        let before = measure("fn legacy 80\nfn small 12\n");
        let after = measure("fn legacy 95\nfn small 21\nfn fresh 30\n");
        let note = crossings("x.rs", &before, &after).expect("two crossings");
        assert!(note.contains("`small` (21 lines), `fresh` (30 lines)"), "{note}");
        assert!(!note.contains("legacy"), "{note}");
        assert!(note.contains("unless this project's own conventions"), "{note}");
    }

    /// Exactly at a limit is within it; one past is over.
    #[test]
    fn the_limits_are_inclusive() {
        let at = measure(&padded("fn edge 20", MAX_FILE_LINES));
        assert_eq!(crossings("x.rs", &SizeSnapshot::default(), &at), None);
        let over = measure(&padded("fn edge 21", MAX_FILE_LINES + 1));
        let note = crossings("x.rs", &SizeSnapshot::default(), &over).expect("both crossed");
        assert!(note.contains("x.rs is now 501 lines"), "{note}");
        assert!(note.contains("`edge` (21 lines)"), "{note}");
    }

    /// A file already past 500 lines is not news when it grows again.
    #[test]
    fn a_file_already_over_says_nothing_more() {
        let before = measure(&padded("", 700));
        let after = measure(&padded("", 720));
        assert_eq!(crossings("x.rs", &before, &after), None);
    }

    #[test]
    fn a_long_list_is_counted() {
        let many: String = (0..8).map(|n| format!("fn f{n} 30\n")).collect();
        let note = crossings("x.rs", &SizeSnapshot::default(), &measure(&many)).expect("crossed");
        assert!(note.contains("and 3 more"), "{note}");
        assert!(!note.contains("`f7`"), "{note}");
    }
}
