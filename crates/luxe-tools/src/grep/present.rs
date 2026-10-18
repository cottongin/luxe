//! Present — a content search laid out for reading: each file named once, its lines under it, and
//! the scope around a run of matches where an outline can say.
//!
//! The rows are counted and capped as ripgrep would print them, `file:line:text`, so the bounds and
//! the totals are the ones the other modes keep. Only the layout changes. A path repeated on every
//! line is a large share of a broad search, and the function a match sits in is what the next
//! `read` would have gone to find out.

use super::{Hit, HitKind, Search, GREP_MAX_CHARS};

/// The capped, grouped answer, and how many of `hits` it shows.
pub(super) fn content(hits: &[Hit], search: &Search<'_>) -> (String, usize) {
    let lengths = hits.iter().map(|hit| hit.flat().chars().count());
    let kept = crate::output_limit::lines_within(lengths, search.limit, GREP_MAX_CHARS);
    let shown = grouped(&hits[..kept], search.before + search.after > 0);
    let text = match kept < hits.len() {
        true => crate::output_limit::with_note(
            &shown,
            hits.len() - kept,
            hits.len(),
            search.mode.unit(),
            search.mode.hint(),
        ),
        false => crate::output_limit::clamp(&shown),
    };
    (text, kept)
}

/// `hits` under their files: the path, then `line:text` for a match and `line-text` for context,
/// `  [Scope › name]` before a match whose scope differs from the last one's, and `--` where
/// context leaves a gap.
fn grouped(hits: &[Hit], context: bool) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut previous: Option<&Hit> = None;
    let mut scope: Option<&str> = None;
    for hit in hits {
        match previous {
            Some(last) if last.file == hit.file => {
                if context && hit.line > last.line + 1 {
                    out.push("--".into());
                }
            }
            _ => {
                out.push(hit.file.to_string());
                scope = None;
            }
        }
        if hit.kind == HitKind::Match && hit.scope.as_deref() != scope {
            out.push(scope_row(hit.scope.as_deref()));
            scope = hit.scope.as_deref();
        }
        let separator = if hit.kind == HitKind::Match { ':' } else { '-' };
        out.push(format!("{}{separator}{}", hit.line, hit.text));
        previous = Some(hit);
    }
    out.join("\n")
}

/// `  [QuerySet › delete]`, or `  [top level]` for a match that has left the scopes before it.
fn scope_row(scope: Option<&str>) -> String {
    format!("  [{}]", scope.unwrap_or("top level"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(file: &str, kind: HitKind, line: usize, text: &str, scope: Option<&str>) -> Hit {
        let (file, text, scope) = (file.into(), text.to_string(), scope.map(String::from));
        Hit { file, kind, line, text, scope }
    }

    /// Each file is named once, its lines under it, and a scope row comes only where it changes.
    #[test]
    fn matches_group_under_their_file_with_a_row_per_change_of_scope() {
        let hits = [
            hit("a.py", HitKind::Match, 3, "    def delete(self):", Some("QuerySet")),
            hit("a.py", HitKind::Match, 9, "    def delete_all(self):", Some("QuerySet")),
            hit("a.py", HitKind::Match, 20, "def delete(): pass", None),
            hit("b.py", HitKind::Match, 5, "        delete()", Some("Model › save")),
        ];
        let expected = "a.py\n  [QuerySet]\n3:    def delete(self):\n9:    def delete_all(self):\n  \
                        [top level]\n20:def delete(): pass\nb.py\n  [Model › save]\n5:        delete()";
        assert_eq!(grouped(&hits, false), expected);
    }

    /// A file no outline can read gets no scope rows at all, rather than a `[top level]` that
    /// would claim to know.
    #[test]
    fn an_unlabeled_file_gets_no_scope_rows() {
        let hits = [hit("notes.md", HitKind::Match, 1, "delete it", None)];
        assert_eq!(grouped(&hits, false), "notes.md\n1:delete it");
    }

    /// Context lines take `-`, and a gap between runs of context is marked `--`.
    #[test]
    fn context_rows_take_a_dash_and_a_gap_is_marked() {
        let hits = [
            hit("a.rs", HitKind::Match, 2, "needle", None),
            hit("a.rs", HitKind::Context, 3, "after", None),
            hit("a.rs", HitKind::Match, 9, "needle", None),
        ];
        assert_eq!(grouped(&hits, true), "a.rs\n2:needle\n3-after\n--\n9:needle");
        assert_eq!(grouped(&hits, false), "a.rs\n2:needle\n3-after\n9:needle", "no context asked");
    }
}
