use super::{changes, plan};
use crate::render::{escape, Renderer, StatusFacts, StatusSegment, Voice};
use luxe_core::workspace::{FileChangeStatus, TodoSummary, WorkspaceChangesSummary};
use luxe_core::TodoSummaryStatus;

/// Marks that name their status, with markup in them so escaping shows.
struct Named;

impl Renderer for Named {
    fn markdown(&self, text: &str, _voice: Voice) -> String {
        escape(text)
    }
    fn tool_glyph(&self, _tool: &str) -> String {
        String::new()
    }
    fn status(&self, _facts: &StatusFacts) -> Vec<StatusSegment> {
        Vec::new()
    }
    fn todo_mark(&self, status: TodoSummaryStatus) -> String {
        format!("<{status:?}>")
    }
    fn change_mark(&self, status: FileChangeStatus) -> String {
        format!("{status:?}")
    }
}

/// A `workspace_updated` frame's `todos`, copied from a live session.
const TODOS: &str = r#"{"current": {"id": "2", "subject": "Write the summary", "status": "in_progress", "blocked_by": []}, "items": [{"id": "1", "subject": "Read the notes", "status": "completed", "blocked_by": []}, {"id": "2", "subject": "Write the summary", "status": "in_progress", "blocked_by": []}, {"id": "3", "subject": "Check the summary", "status": "pending", "blocked_by": []}], "pending": 1, "in_progress": 1, "completed": 1, "blocked": 0, "total": 3, "truncated": false}"#;

/// A `workspace_updated` frame's `changes`, copied from a live session.
const CHANGES: &str = r#"{"files": [{"path": "notes.txt", "status": "added", "additions": 1, "deletions": 0, "binary": false}, {"path": "README.md", "status": "untracked", "additions": 3, "deletions": 0, "binary": false}], "total_files": 2, "additions": 4, "deletions": 0, "truncated": false}"#;

fn todos(json: &str) -> TodoSummary {
    serde_json::from_str(json).expect("a todos summary as core sends it")
}

fn changed(json: &str) -> WorkspaceChangesSummary {
    serde_json::from_str(json).expect("a changes summary as core sends it")
}

/// The break this catches: the dock's heading, counts or marks crossed or dropped, rows reordered
/// (the TUI keeps the order they were written in), a status class misspelled so the stylesheet
/// can't color it, or a mark written into the page unescaped.
#[test]
fn the_plan_reads_as_the_docks_checklist() {
    let row = |status: &str, words: &str, mark: &str, subject: &str| {
        format!(
            "<li class=\"todo todo-{status}\"><span class=\"todo-mark\" role=\"img\" \
             aria-label=\"{words}\">&lt;{mark}&gt;</span> <span class=\"todo-subject\">{subject}\
             </span></li>"
        )
    };
    let expected = format!(
        "<p class=\"dock-head\"><span class=\"badge raised\">PLAN</span> 1/3</p>\
         <p class=\"dock-sub\">1 pending · 1 active · 1 done</p><ol class=\"todos\">{}{}{}</ol>",
        row("completed", "done", "Completed", "Read the notes"),
        row("in-progress", "in progress", "InProgress", "Write the summary"),
        row("pending", "to do", "Pending", "Check the summary"),
    );
    assert_eq!(plan(&Named, &todos(TODOS)), expected);
}

/// The break this catches: a trimmed plan shown as if it were all of it, or its counts crossed
/// (the live plan above has as many pending as active, so it can't tell them apart).
#[test]
fn a_trimmed_plan_says_how_much_more_there_is() {
    let trimmed = TODOS
        .replace("\"pending\": 1,", "\"pending\": 3,")
        .replace("\"total\": 3, \"truncated\": false", "\"total\": 5, \"truncated\": true");
    let html = plan(&Named, &todos(&trimmed));
    assert!(html.contains(">3 pending · 1 active · 1 done<"), "{html}");
    assert!(html.ends_with("</ol><p class=\"dock-sub\">and 2 more</p>"), "{html}");
}

/// The break this catches: a file's path, counts or mark crossed or dropped, the button without the
/// path the app opens the diff by, or the heading's totals wrong.
#[test]
fn each_change_is_a_way_into_its_diff() {
    let expected = "<p class=\"dock-head\"><span class=\"badge raised\">FILES</span> 2 changed \
        <span class=\"add\">+4</span> <span class=\"del\">−0</span></p><ul class=\"changes\">\
        <li><button type=\"button\" class=\"change change-added\" data-path=\"notes.txt\">\
        <span class=\"change-path\">notes.txt</span><span class=\"change-counts\"> \
        <span class=\"add\">+1</span> <span class=\"del\">−0</span></span> \
        <span class=\"change-mark\">Added</span></button></li>\
        <li><button type=\"button\" class=\"change change-untracked\" data-path=\"README.md\">\
        <span class=\"change-path\">README.md</span><span class=\"change-counts\"> \
        <span class=\"add\">+3</span> <span class=\"del\">−0</span></span> \
        <span class=\"change-mark\">Untracked</span></button></li></ul>";
    assert_eq!(changes(&Named, &changed(CHANGES)), expected);
}

/// The break this catches: counts Git couldn't make printed as `+0 −0`, which says the file
/// didn't change, when a binary file plainly did.
#[test]
fn counts_git_couldnt_make_are_left_out() {
    let binary = r#"{"files": [{"path": "logo.png", "status": "modified", "additions": null, "deletions": null, "binary": true}], "total_files": 1, "additions": null, "deletions": null, "truncated": false}"#;
    let html = changes(&Named, &changed(binary));
    assert!(html.starts_with(
        "<p class=\"dock-head\"><span class=\"badge raised\">FILES</span> 1 changed</p>"
    ));
    assert!(html.contains("<span class=\"change-counts\"></span>"), "{html}");
}

/// The break this catches: a path written into the page, or into the attribute the app reads,
/// unescaped, so a file named with a quote breaks out of it.
#[test]
fn a_path_is_escaped_everywhere_it_goes() {
    let odd = CHANGES.replace("notes.txt", "a\\\"<b>.txt");
    let html = changes(&Named, &changed(&odd));
    assert!(html.contains("data-path=\"a&quot;&lt;b&gt;.txt\""), "{html}");
    assert!(html.contains("<span class=\"change-path\">a&quot;&lt;b&gt;.txt</span>"), "{html}");
}

/// The break this catches: an empty change list drawn as an empty box, or the trimmed tail said
/// when nothing was trimmed.
#[test]
fn nothing_changed_says_so() {
    let none =
        r#"{"files": [], "total_files": 0, "additions": 0, "deletions": 0, "truncated": false}"#;
    assert!(changes(&Named, &changed(none))
        .ends_with("</p><p class=\"dock-sub\">nothing has changed</p>"));
}
