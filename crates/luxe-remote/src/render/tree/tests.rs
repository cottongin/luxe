use super::{project_name, tree, FileMark, MarkHue, TreeRow};
use crate::render::{escape, Renderer, StatusFacts, StatusSegment, Voice};
use luxe_core::workspace::{FileChangeStatus, WorkspaceChangesSummary};

/// Marks that say what they are: `F` for a file, warm for Rust, `D` for a folder, a status's name.
struct Marked;

impl Renderer for Marked {
    fn markdown(&self, text: &str, _voice: Voice) -> String {
        escape(text)
    }
    fn tool_glyph(&self, _tool: &str) -> String {
        String::new()
    }
    fn status(&self, _facts: &StatusFacts) -> Vec<StatusSegment> {
        Vec::new()
    }
    fn change_mark(&self, status: FileChangeStatus) -> String {
        format!("{status:?}")
    }
    fn file_mark(&self, name: &str) -> FileMark {
        let hue = if name.ends_with(".rs") { MarkHue::Warning } else { MarkHue::Muted };
        FileMark { glyph: "F".into(), hue }
    }
    fn folder_mark(&self) -> String {
        "D".into()
    }
    fn branch_mark(&self) -> String {
        "B".into()
    }
}

/// A `workspace_updated` frame's `changes`, copied from a live session, with its file moved into
/// `src/` so a folder holds it.
const CHANGES: &str = r#"{"files": [{"path": "src/notes.txt", "status": "added", "additions": 1, "deletions": 0, "binary": false}], "total_files": 1, "additions": 1, "deletions": 0, "truncated": false}"#;

/// The panel's bar names the project behind the tree's folder mark, and the name stays text.
///
/// The break this catches: a drawn icon standing in for the TUI's glyph, so the bar and the tree
/// disagree at every tier, or a folder named like markup running as markup.
#[test]
fn the_project_name_wears_the_folder_mark() {
    assert_eq!(
        project_name(&Marked, "luxe", None, true),
        "<span class=\"project-mark\">D</span>luxe",
        "no branch, nothing after the name, dirty or not"
    );
    assert!(project_name(&Marked, "<b>x", None, false).ends_with("&lt;b&gt;x"));
}

/// The branch follows the name behind its own mark, starred only when the tree has changes.
///
/// The break this catches: the dirty star shown on a clean tree or lost on a dirty one, the branch
/// drawn without the TUI's mark, or a branch name running as markup.
#[test]
fn the_branch_follows_the_name_starred_when_dirty() {
    let branch = |dirty| project_name(&Marked, "luxe", Some("remote-access"), dirty);
    let lead = "<span class=\"project-mark\">D</span>luxe<span class=\"project-branch\">\
                <span class=\"project-mark\">B</span>remote-access";
    assert_eq!(branch(true), format!("{lead} *</span>"));
    assert_eq!(branch(false), format!("{lead}</span>"));
    assert!(project_name(&Marked, "x", Some("<i>"), false).contains("&lt;i&gt;"));
}

fn rows(json: serde_json::Value) -> Vec<TreeRow> {
    serde_json::from_value(json).expect("rows as the app sends them")
}

fn changes() -> WorkspaceChangesSummary {
    serde_json::from_str(CHANGES).expect("a changes summary as core sends it")
}

/// The break this catches: a guide too many or too few, an open folder drawn closed, a mark in the
/// wrong hue, or the attribute the app's tap reads missing or crossed between folder and file.
#[test]
fn rows_read_as_the_tuis_tree() {
    let rows = rows(serde_json::json!([
        { "kind": "folder", "path": "src", "name": "src", "depth": 0, "open": true },
        { "kind": "file", "path": "src/main.rs", "name": "main.rs", "depth": 1 },
    ]));
    let expected = "<div class=\"tree\">\
        <button type=\"button\" class=\"tree-row\" data-folder=\"src\" aria-expanded=\"true\">\
        <span class=\"chevron\">▾</span><span class=\"mark hue-secondary\" aria-hidden=\"true\">D\
        </span><span class=\"tree-name folder\">src</span></button>\
        <button type=\"button\" class=\"tree-row\" data-file=\"src/main.rs\">\
        <span class=\"guide\" aria-hidden=\"true\"></span><span class=\"chevron\"></span>\
        <span class=\"mark hue-warning\" aria-hidden=\"true\">F</span>\
        <span class=\"tree-name\">main.rs</span></button></div>";
    assert_eq!(tree(&Marked, &rows, None), expected);
}

/// The break this catches: a changed file's counts or letter dropped, or its name not colored by
/// its status, as the TUI's tree colors it.
#[test]
fn a_changed_file_ends_with_its_counts_and_letter() {
    let rows = rows(serde_json::json!([
        { "kind": "file", "path": "src/notes.txt", "name": "notes.txt", "depth": 1 },
        { "kind": "file", "path": "notes.txt", "name": "notes.txt", "depth": 0 },
    ]));
    let html = tree(&Marked, &rows, Some(&changes()));
    let changed =
        "<span class=\"tree-name change-added\">notes.txt</span><span class=\"tree-right\"> \
        <span class=\"add\">+1</span> <span class=\"del\">−0</span> \
        <span class=\"change-added\">Added</span></span></button>";
    assert_eq!(html.matches(changed).count(), 1, "only the file at that path is marked: {html}");
}

/// The break this catches: the dot on an open folder, whose change is in plain sight, missing from
/// a closed one, or shown for a folder whose name only starts the changed file's (`sr` for `src`).
#[test]
fn a_closed_folder_holding_a_change_wears_a_dot() {
    let folder = |path: &str, open: bool| serde_json::json!({ "kind": "folder", "path": path, "name": path, "depth": 0, "open": open });
    let html = |row| tree(&Marked, &rows(serde_json::json!([row])), Some(&changes()));
    let dot = "<span class=\"tree-right held\">•</span>";
    assert!(html(folder("src", false)).contains(dot));
    assert!(!html(folder("src", true)).contains(dot));
    assert!(!html(folder("sr", false)).contains(dot));
}

/// The break this catches: a note drawn at the wrong depth, or written into the page unescaped.
#[test]
fn a_note_stands_where_entries_would() {
    let rows = rows(serde_json::json!([{ "kind": "note", "depth": 2, "text": "<empty>" }]));
    let expected = "<div class=\"tree\"><p class=\"tree-row tree-note\">\
        <span class=\"guide\" aria-hidden=\"true\"></span><span class=\"guide\" aria-hidden=\"true\">\
        </span>&lt;empty&gt;</p></div>";
    assert_eq!(tree(&Marked, &rows, None), expected);
}

/// The break this catches: a path or name written into the attribute the app reads, or the page,
/// unescaped, so a file named with a quote breaks out of it.
#[test]
fn paths_and_names_are_escaped() {
    let rows = rows(serde_json::json!([
        { "kind": "file", "path": "a\"<b>", "name": "a\"<b>", "depth": 0 },
    ]));
    let html = tree(&Marked, &rows, None);
    assert!(html.contains("data-file=\"a&quot;&lt;b&gt;\""), "{html}");
    assert!(html.contains(">a&quot;&lt;b&gt;</span>"), "{html}");
}
