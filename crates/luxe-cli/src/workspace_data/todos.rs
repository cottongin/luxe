//! Todos — the session's checklist, as the summary a front-end shows.

use luxe_core::{TodoStatus, TodoStore, TodoSummary, TodoSummaryItem, TodoSummaryStatus};
use std::path::{Path, PathBuf};

use super::bounded_text;

pub(super) async fn collect_todo_summary(root: PathBuf, session_id: Option<String>) -> TodoSummary {
    match tokio::task::spawn_blocking(move || todo_summary(&root, session_id.as_deref())).await {
        Ok(Ok(summary)) => summary,
        Ok(Err(error)) => {
            tracing::warn!(error = %error, "workspace: failed to collect todo summary");
            TodoSummary::default()
        }
        Err(error) => {
            tracing::warn!(error = %error, "workspace: todo summary worker failed");
            TodoSummary::default()
        }
    }
}

/// The dock reflects this session's checklist — there is no repo-global one.
fn todo_summary(
    root: &Path,
    session_id: Option<&str>,
) -> Result<TodoSummary, luxe_core::TodoError> {
    let Some(session_id) = session_id else {
        return Ok(TodoSummary::default());
    };
    let todos = TodoStore::new(root.join(".luxe").join("todos")).read(session_id)?;
    let visible = todos.iter().filter(|todo| todo.status.is_live()).collect::<Vec<_>>();
    Ok(TodoSummary {
        current: visible
            .iter()
            .find(|todo| todo.status == TodoStatus::InProgress)
            .and_then(|todo| todo_summary_item(todo, &todos)),
        items: visible
            .iter()
            .take(200)
            .filter_map(|todo| todo_summary_item(todo, &todos))
            .collect(),
        pending: visible
            .iter()
            .filter(|todo| {
                todo.status == TodoStatus::Pending && todo.open_blockers(&todos).is_empty()
            })
            .count(),
        in_progress: visible.iter().filter(|todo| todo.status == TodoStatus::InProgress).count(),
        completed: visible.iter().filter(|todo| todo.status == TodoStatus::Completed).count(),
        blocked: visible.iter().filter(|todo| !todo.open_blockers(&todos).is_empty()).count(),
        total: visible.len(),
        truncated: visible.len() > 200,
    })
}

/// One row of the summary. Blocked only by work still open: a task whose blocker is done is
/// ready, the way the `todo` tool lists it.
fn todo_summary_item(todo: &luxe_core::Todo, all: &[luxe_core::Todo]) -> Option<TodoSummaryItem> {
    let blockers = todo.open_blockers(all);
    todo.status.is_live().then(|| TodoSummaryItem {
        id: todo.id.to_string(),
        subject: bounded_text(&todo.subject, 160),
        status: if !blockers.is_empty() && todo.status != TodoStatus::Completed {
            TodoSummaryStatus::Blocked
        } else {
            match todo.status {
                TodoStatus::Pending => TodoSummaryStatus::Pending,
                TodoStatus::InProgress => TodoSummaryStatus::InProgress,
                TodoStatus::Completed => TodoSummaryStatus::Completed,
                TodoStatus::Deleted => unreachable!("deleted todos are excluded above"),
            }
        },
        blocked_by: blockers.iter().map(ToString::to_string).collect(),
    })
}
