use super::{failed, file};
use crate::render::{escape, Renderer, StatusFacts, StatusSegment, Voice};
use luxe_core::file_diff::FileDiff;
use luxe_core::file_diff::FileDiffFailure;

/// Code that shows the language it was highlighted for, escaped.
struct Bracketed;

impl Renderer for Bracketed {
    fn markdown(&self, text: &str, _voice: Voice) -> String {
        escape(text)
    }
    fn tool_glyph(&self, _tool: &str) -> String {
        String::new()
    }
    fn status(&self, _facts: &StatusFacts) -> Vec<StatusSegment> {
        Vec::new()
    }
    fn code_line(&self, text: &str, language: Option<&str>) -> String {
        format!("[{}:{}]", language.unwrap_or("-"), escape(text))
    }
}

/// A `file_diff_loaded` frame's `diff`, copied from a live session.
const ADDED: &str = r#"{"path": "notes.txt", "file": {"id": "file:bebf0714d086bce43fab95f77103f6ec03bf140a484e7bc0eabaad1e0fc6c2fa", "old_path": null, "new_path": "notes.txt", "status": "added", "language": "txt", "additions": 1, "deletions": 0, "hunks": [{"id": "hunk:a6cc64dc21d9131afb5ce63a2d7f2eec2a826f3660d3f0d4b1d43295fa97babd", "old_range": {"start": 0, "count": 0}, "new_range": {"start": 1, "count": 1}, "heading": null, "symbol": null, "lines": [{"old_line": null, "new_line": 1, "kind": "addition", "text": "hello"}], "noise": "semantic"}], "binary": false, "generated": false, "non_text": null}, "untracked": false}"#;

/// One line of each kind, for a hunk built on [`ADDED`].
const EVERY_KIND: &str = r#"[{"old_line": 4, "new_line": 4, "kind": "context", "text": "fn a() {"}, {"old_line": 5, "new_line": null, "kind": "deletion", "text": "  x < y"}, {"old_line": null, "new_line": 5, "kind": "addition", "text": "  x > y"}, {"old_line": null, "new_line": null, "kind": "no_newline_marker", "text": "\\ No newline at end of file"}]"#;

fn diff(json: &str) -> FileDiff {
    serde_json::from_str(json).expect("a file diff as core sends it")
}

/// The break this catches: the hunk's range, a line's numbers, sign or kind wrong, its code not
/// highlighted for the file's language, or the block and the line not saying what a pick of it
/// points at.
#[test]
fn an_added_file_shows_its_line() {
    let expected = "<div class=\"diff\" data-path=\"notes.txt\"><div class=\"hunk\">\
        <p class=\"hunk-head\">@@ -0,0 +1,1 @@</p><p class=\"dl dl-addition\" data-line=\"1\">\
        <span class=\"ln\"></span><span class=\"ln\">1</span><span class=\"sign\">+</span>\
        <code>[txt:hello]</code></p></div></div>";
    assert_eq!(file(&Bracketed, &diff(ADDED)), expected);
}

/// The break this catches: a deletion signed or numbered as an addition or as context, a hunk's
/// heading dropped or unescaped, or the no-newline note drawn as a line of code.
#[test]
fn each_kind_of_line_keeps_its_sign_and_numbers() {
    let modified = ADDED.replace(r#""heading": null"#, r#""heading": "impl <A>""#).replace(
        r#"[{"old_line": null, "new_line": 1, "kind": "addition", "text": "hello"}]"#,
        EVERY_KIND,
    );
    let html = file(&Bracketed, &diff(&modified));
    // a deleted line names no line to pick: the file doesn't have it any more
    let row = |kind: &str, old: &str, new: &str, sign: &str, code: &str| {
        let picks = if new.is_empty() { String::new() } else { format!(" data-line=\"{new}\"") };
        format!(
            "<p class=\"dl dl-{kind}\"{picks}><span class=\"ln\">{old}</span><span \
             class=\"ln\">{new}</span><span class=\"sign\">{sign}</span><code>[txt:{code}]</code></p>"
        )
    };
    let expected = format!(
        "<p class=\"hunk-head\">@@ -0,0 +1,1 @@ impl &lt;A&gt;</p>{}{}{}\
         <p class=\"dl dl-note\">\\ No newline at end of file</p></div>",
        row("context", "4", "4", " ", "fn a() {"),
        row("deletion", "5", "", "-", "  x &lt; y"),
        row("addition", "", "5", "+", "  x &gt; y"),
    );
    assert!(html.contains(&expected), "{html}");
}

/// A deleted file's block names no file to pick in, and a path is text in its attribute.
///
/// The break this catches: Ask offered on a file that's gone (the old path points at nothing), or a
/// path with a quote closing the attribute early.
#[test]
fn only_a_file_that_is_still_there_can_be_picked_in() {
    let deleted = ADDED.replace(
        r#""old_path": null, "new_path": "notes.txt""#,
        r#""old_path": "notes.txt", "new_path": null"#,
    );
    assert!(file(&Bracketed, &diff(&deleted)).starts_with("<div class=\"diff\"><div"));
    let quoted = ADDED.replace(r#""new_path": "notes.txt""#, r#""new_path": "a\"b&c.txt""#);
    let html = file(&Bracketed, &diff(&quoted));
    assert!(html.starts_with("<div class=\"diff\" data-path=\"a&quot;b&amp;c.txt\">"), "{html}");
}

/// The break this catches: a failure shown as nothing, or Git's words written into the page
/// unescaped.
#[test]
fn a_failure_says_why_in_cores_words() {
    let git: FileDiffFailure =
        serde_json::from_str(r#"{"git": "bad <ref>"}"#).expect("core's shape");
    let expected = "<p class=\"diff-note\">git could not describe this file: bad &lt;ref&gt;</p>";
    assert_eq!(failed(&git), expected);
}

/// The break this catches: a file that matches its base, or a binary one, drawn as an empty diff
/// with nothing saying why.
#[test]
fn a_diff_with_no_lines_says_why() {
    let unchanged = r#"{"path": "notes.txt", "file": null, "untracked": false}"#;
    assert!(file(&Bracketed, &diff(unchanged)).contains("matches the last commit"));
    let binary = ADDED.replace(r#""binary": false"#, r#""binary": true"#);
    assert!(file(&Bracketed, &diff(&binary)).contains("isn't text"));
}
