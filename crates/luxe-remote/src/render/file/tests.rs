use super::{failed, file, size};
use crate::render::{escape, Renderer, StatusFacts, StatusSegment, Voice};
use luxe_core::project_files::{ProjectFile, ProjectFileFailure};

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

/// A `project_file_read` frame's `file`, copied from `luxe --rpc`.
const READ: &str = r#"{"path":"a.rs","text":"fn main() {\n\tprintln!(\"<hi>\");\n}\n","bytes":33,"truncated":false,"binary":false}"#;

fn read(edit: impl FnOnce(&mut ProjectFile)) -> ProjectFile {
    let mut file = serde_json::from_str(READ).expect("a project file as core sends it");
    edit(&mut file);
    file
}

/// The break this catches: lines numbered from zero or out of order, a line dropped, the trailing
/// newline drawn as an empty last line, the code not highlighted for the file's extension, or a
/// line not saying which file and line a pick of it points at.
#[test]
fn a_file_shows_each_line_numbered_and_highlighted() {
    let expected = "<div class=\"file\" data-path=\"a.rs\">\
        <p class=\"fl\" data-line=\"1\"><span class=\"ln\">1</span><code>[rs:fn main() {]</code></p>\
        <p class=\"fl\" data-line=\"2\"><span class=\"ln\">2</span>\
        <code>[rs:\tprintln!(&quot;&lt;hi&gt;&quot;);]</code></p>\
        <p class=\"fl\" data-line=\"3\"><span class=\"ln\">3</span><code>[rs:}]</code></p></div>";
    assert_eq!(file(&Bracketed, &read(|_| ())), expected);
}

/// The break this catches: a file with no extension highlighted as some language anyway.
#[test]
fn a_file_without_an_extension_is_highlighted_as_nothing() {
    let html = file(&Bracketed, &read(|file| file.path = "Makefile".into()));
    assert!(html.contains("<code>[-:fn main() {]</code>"), "{html}");
}

/// The break this catches: a cut-short file shown as if it were whole, or the note's sizes swapped.
#[test]
fn a_cut_short_file_says_how_much_is_shown() {
    let html = file(
        &Bracketed,
        &read(|file| {
            file.truncated = true;
            file.bytes = 3 * 1_048_576;
        }),
    );
    assert!(html.ends_with("<p class=\"diff-note\">First 33 B of 3.0 MB</p>"), "{html}");
}

/// The break this catches: a binary file's empty text drawn as an empty file, or the reverse.
#[test]
fn a_binary_file_and_an_empty_one_each_say_so() {
    let binary = read(|file| {
        file.binary = true;
        file.text.clear();
    });
    let empty = read(|file| file.text.clear());
    assert_eq!(file(&Bracketed, &binary), "<p class=\"diff-note\">Binary file</p>");
    assert_eq!(file(&Bracketed, &empty), "<p class=\"diff-note\">Empty file</p>");
}

/// The break this catches: a failure's reason left out, or a path in it not escaped.
#[test]
fn a_failure_says_why_in_cores_words() {
    let not_found: ProjectFileFailure = serde_json::from_str(r#""not_found""#).expect("a failure");
    let unreadable = ProjectFileFailure::Unreadable("<denied>".into());
    assert_eq!(failed(&not_found), "<p class=\"diff-note\">nothing is there any more</p>");
    assert!(failed(&unreadable).contains("&lt;denied&gt;"));
}

/// The break this catches: a boundary in the wrong unit, so 1024 bytes reads `1024 B`.
#[test]
fn sizes_read_in_the_unit_a_person_expects() {
    assert_eq!(size(1023), "1023 B");
    assert_eq!(size(1024), "1 KB");
    assert_eq!(size(1_048_575), "1023 KB");
    assert_eq!(size(1_048_576), "1.0 MB");
}
