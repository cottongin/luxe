//! The dock's sections, as the app shows them beside the transcript: the plan and the changes.
//!
//! The TUI's words and marks, laid out for a touch screen: every row of the plan wraps rather than
//! scrolling in a window of its own, and each changed file is a button that opens its diff.

use super::{escape, Renderer};
use luxe_core::workspace::{
    FileChangeStatus, FileChangeSummary, TodoSummary, TodoSummaryItem, WorkspaceChangesSummary,
};
use luxe_core::TodoSummaryStatus;

/// `PLAN 2/5`, the counts, and the checklist in the order it was written.
pub(super) fn plan(renderer: &dyn Renderer, todos: &TodoSummary) -> String {
    let mut out = heading("PLAN", &format!("{}/{}", todos.completed, todos.total));
    out.push_str(&format!(
        "<p class=\"dock-sub\">{} pending \u{b7} {} active \u{b7} {} done</p><ol class=\"todos\">",
        todos.pending, todos.in_progress, todos.completed
    ));
    for item in &todos.items {
        out.push_str(&todo_row(renderer, item));
    }
    out.push_str("</ol>");
    out + &more(todos.total, todos.items.len())
}

fn todo_row(renderer: &dyn Renderer, item: &TodoSummaryItem) -> String {
    let (status, words) = todo_status(item.status);
    format!(
        "<li class=\"todo todo-{status}\"><span class=\"todo-mark\" role=\"img\" \
         aria-label=\"{words}\">{}</span> <span class=\"todo-subject\">{}</span></li>",
        escape(&renderer.todo_mark(item.status)),
        escape(&item.subject)
    )
}

/// The status as the class the stylesheet colors by, and as words for a screen reader.
fn todo_status(status: TodoSummaryStatus) -> (&'static str, &'static str) {
    match status {
        TodoSummaryStatus::Pending => ("pending", "to do"),
        TodoSummaryStatus::InProgress => ("in-progress", "in progress"),
        TodoSummaryStatus::Completed => ("completed", "done"),
        TodoSummaryStatus::Blocked => ("blocked", "blocked"),
    }
}

/// `FILES 3 changed +7 −0`, and a row for each file.
pub(super) fn changes(renderer: &dyn Renderer, changes: &WorkspaceChangesSummary) -> String {
    let counts = counts(changes.additions, changes.deletions);
    let mut out = heading("FILES", &format!("{} changed{counts}", changes.total_files));
    if changes.files.is_empty() {
        return out + "<p class=\"dock-sub\">nothing has changed</p>";
    }
    out.push_str("<ul class=\"changes\">");
    for file in &changes.files {
        out.push_str(&change_row(renderer, file));
    }
    out.push_str("</ul>");
    out + &more(changes.total_files, changes.files.len())
}

fn change_row(renderer: &dyn Renderer, file: &FileChangeSummary) -> String {
    let path = escape(&luxe_core::paths::display(&file.path));
    format!(
        "<li><button type=\"button\" class=\"change change-{}\" data-path=\"{path}\">\
         <span class=\"change-path\">{path}</span><span class=\"change-counts\">{}</span> \
         <span class=\"change-mark\">{}</span></button></li>",
        change_status(file.status),
        counts(file.additions, file.deletions),
        escape(&renderer.change_mark(file.status)),
    )
}

pub(super) fn change_status(status: FileChangeStatus) -> &'static str {
    match status {
        FileChangeStatus::Added => "added",
        FileChangeStatus::Modified => "modified",
        FileChangeStatus::Deleted => "deleted",
        FileChangeStatus::Renamed => "renamed",
        FileChangeStatus::Untracked => "untracked",
    }
}

/// ` +6 −0`, or nothing when Git couldn't count, since `+0 −0` would claim nothing changed.
pub(super) fn counts(additions: Option<u64>, deletions: Option<u64>) -> String {
    let (Some(additions), Some(deletions)) = (additions, deletions) else {
        return String::new();
    };
    format!(
        " <span class=\"add\">+{additions}</span> <span class=\"del\">\u{2212}{deletions}</span>"
    )
}

/// The TUI's heading: the word cut into a badge at the depth both of these sections sit at, with
/// what it sums up riding bare beside it.
fn heading(title: &str, rest: &str) -> String {
    format!("<p class=\"dock-head\"><span class=\"badge raised\">{title}</span> {rest}</p>")
}

/// `and 3 more`, for rows the snapshot left out.
fn more(total: usize, shown: usize) -> String {
    match total.saturating_sub(shown) {
        0 => String::new(),
        left => format!("<p class=\"dock-sub\">and {left} more</p>"),
    }
}

#[cfg(test)]
mod tests;
