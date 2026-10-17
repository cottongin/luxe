//! `todo` tool — the agent's live checklist, persisted per session.
//!
//! Delegates to [`TodoStore`] for durability: one file per session, so the plan survives
//! compaction, restart and resume.
//!
//! Tool arguments use the surface's camelCase convention (`activeForm`/`blockedBy`).
//! [`Todo`] still serializes `active_form`/`blocked_by`: renaming those would orphan existing
//! checklists under `.luxe/todos/`.

use async_trait::async_trait;
use luxe_core::{Todo, TodoStatus, TodoStore, Tool, ToolContext, ToolResultData, ToolSchema};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// JSON-schema for the todo tool arguments.
fn todo_schema() -> ToolSchema {
    ToolSchema {
        name: "todo".to_string(),
        description:
            "Your live checklist for the current effort, and the only view the user has into \
             your plan. Scoped to this session and persisted to disk, so it survives compaction \
             and restart and is restored when the session resumes.\n\
             Actions: add{subjects[] | subject, blockedBy?}; update{id, status?, activeForm?, \
             blockedBy?}; complete{id}; list{}.\n\
             Use it when the work takes three or more distinct steps, touches more than one file, \
             or arrives as a list — open the whole plan in ONE `add` with `subjects`, never one \
             item at a time. The list moves itself along: `add` starts the first item when \
             nothing is in progress, and `complete` starts the next open one, so working down the \
             plan in order takes one `complete` per item and nothing else. Use `update` only to \
             start a different item or to put one back. Exactly one item is in_progress. Never \
             mark something \
             completed while its tests fail or the work is partial — leave it in_progress and add \
             a todo for the blocker. `blockedBy` (todo ids) records a dependency; blocked items \
             are flagged in `list` until their blockers finish, and clear themselves once those \
             blockers do.\n\
             Use it for: 'rename items_list to lines across the repo' (one grep, fifteen call \
             sites); 'add a dark mode toggle and run the tests'; any numbered request. Skip it \
             for: a single edit, one command, a question. When in doubt, use it.\n\
             It is a checklist, not a notebook: durable prose — a plan, a finding, a hand-off \
             note — goes in the repository as a real file."
                .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "update", "list", "complete"],
                    "description": "Action to perform."
                },
                "subjects": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Whole plan in one call: every todo subject, in order. Prefer \
                                    this over repeated single 'add' calls. Each subject is a short \
                                    imperative label, ideally under 50 characters."
                },
                "subject": {
                    "type": "string",
                    "description": "Short imperative label for the todo, ideally under 50 \
                                    characters (required for 'add' action)."
                },
                "id": {
                    "type": "integer",
                    "description": "Todo ID (required for 'update', 'complete' actions)."
                },
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed", "deleted"],
                    "description": "New status for 'update' action."
                },
                "activeForm": {
                    "type": "string",
                    "description": "Present-continuous label (optional, for 'update' action)."
                },
                "blockedBy": {
                    "type": "array",
                    "items": { "type": "integer" },
                    "description": "Todo ids this item depends on (optional, for 'add'/'update'). \
                                    On 'update' a non-empty list replaces the existing \
                                    dependencies; an empty one leaves them alone. Blocks clear \
                                    themselves when the blocker completes."
                }
            },
            "required": ["action"]
        }),
    }
}

/// The `todo` tool implementation.
pub struct TodoTool {
    store: Arc<RwLock<TodoStore>>,
}

impl TodoTool {
    /// Create a new todo tool with the given store root.
    pub fn new(store_root: PathBuf) -> Self {
        Self { store: Arc::new(RwLock::new(TodoStore::new(store_root))) }
    }
}

#[async_trait]
impl Tool for TodoTool {
    fn name(&self) -> &str {
        "todo"
    }

    fn schema(&self) -> ToolSchema {
        todo_schema()
    }

    async fn run(&self, args: serde_json::Value, cx: &ToolContext) -> ToolResultData {
        let Some(action) = args.get("action").and_then(|v| v.as_str()) else {
            return ToolResultData::error("Error: 'action' argument is required");
        };

        let todos = {
            let store = self.store.read().await;
            match store.read(&cx.session_id) {
                Ok(todos) => todos,
                Err(error) => {
                    warn!(%error, "failed to read todos");
                    return ToolResultData::error(format!("Failed to read todos: {error}"));
                }
            }
        };

        match action {
            "add" => self.add(&args, cx, todos).await,
            "update" => self.update(&args, cx, todos).await,
            "complete" => self.complete(&args, cx, todos).await,
            "list" => {
                debug!("listing todos");
                ToolResultData::text(render_list(&todos))
            }
            _ => {
                ToolResultData::error("Unknown action. Use 'add', 'update', 'list', or 'complete'.")
            }
        }
    }
}

impl TodoTool {
    /// Adds one todo, or a whole plan of them, and starts the first that is unblocked.
    async fn add(
        &self,
        args: &serde_json::Value,
        cx: &ToolContext,
        mut todos: Vec<Todo>,
    ) -> ToolResultData {
        let subjects = collect_subjects(args);
        if subjects.is_empty() {
            return ToolResultData::error(
                "Error: 'add' needs `subject` (one item) or `subjects` (a whole plan in \
                     one call).",
            );
        }

        debug!(count = subjects.len(), "adding todos");
        let first_id = todos.iter().map(|todo| todo.id).max().unwrap_or(0) + 1;

        // Per-item metadata only makes sense for a single add; a batch is the plan's
        // shape, and its items get their labels and dependencies as they are worked.
        let single = subjects.len() == 1;
        let active_form = single.then(|| active_form_arg(args)).flatten();
        let blocked_by =
            if single { collect_blockers(args).unwrap_or_default() } else { Vec::new() };

        for (offset, subject) in subjects.iter().enumerate() {
            todos.push(Todo {
                id: first_id + offset as u32,
                subject: subject.clone(),
                status: TodoStatus::Pending,
                active_form: active_form.clone(),
                blocked_by: blocked_by.clone(),
            });
        }

        let started = start_next(&mut todos);
        if let Err(error) = self.commit(cx, &todos).await {
            return error;
        }

        let added = if single {
            format!("Added todo #{first_id}: {}", subjects[0])
        } else {
            let listing = subjects
                .iter()
                .enumerate()
                .map(|(i, s)| format!("  #{} {s}", first_id + i as u32))
                .collect::<Vec<_>>()
                .join("\n");
            format!("Added {} todos:\n{listing}", subjects.len())
        };
        ToolResultData::text(with_started(added, started))
    }

    /// Changes one todo: its status, its label, its dependencies.
    async fn update(
        &self,
        args: &serde_json::Value,
        cx: &ToolContext,
        mut todos: Vec<Todo>,
    ) -> ToolResultData {
        let Some(id) = args.get("id").and_then(|v| v.as_u64()).map(|n| n as u32) else {
            return ToolResultData::error("Error: 'id' argument is required for 'update' action");
        };
        debug!(id, "updating todo");

        let Some(todo) = todos.iter_mut().find(|todo| todo.id == id) else {
            return ToolResultData::error(format!("Todo #{id} not found"));
        };
        let subject = todo.subject.clone();
        let mut moved_to = None;
        if let Some(raw) = args.get("status").and_then(|v| v.as_str()) {
            let Some(status) = parse_status(raw) else {
                return ToolResultData::error(format!("Invalid status: {raw}"));
            };
            moved_to = Some(raw.to_string());
            todo.status = status;
        }
        if let Some(active_form) = active_form_arg(args) {
            todo.active_form = Some(active_form);
        }
        // A supplied `blockedBy` replaces the dependency list wholesale.
        if let Some(blocked_by) = collect_blockers(args) {
            todo.blocked_by = blocked_by;
        }

        if let Err(error) = self.commit(cx, &todos).await {
            return error;
        }

        // Naming the item is not decoration: the card's header only has the id to go on,
        // so this is where "which todo did it just start?" is answered. The subject comes
        // last and whole, so a reader of this line — model or card — can take everything
        // past the colon without having to know what else was said.
        ToolResultData::text(match moved_to {
            Some(status) => format!("Updated todo #{id} → {status}: {subject}"),
            None => format!("Updated todo #{id}: {subject}"),
        })
    }

    /// Marks one done, and starts the next that is unblocked.
    async fn complete(
        &self,
        args: &serde_json::Value,
        cx: &ToolContext,
        mut todos: Vec<Todo>,
    ) -> ToolResultData {
        let Some(id) = args.get("id").and_then(|v| v.as_u64()).map(|n| n as u32) else {
            return ToolResultData::error("Error: 'id' argument is required for 'complete' action");
        };
        debug!(id, "completing todo");

        let Some(todo) = todos.iter_mut().find(|todo| todo.id == id) else {
            return ToolResultData::error(format!("Todo #{id} not found"));
        };
        todo.status = TodoStatus::Completed;
        let subject = todo.subject.clone();
        let started = start_next(&mut todos);

        if let Err(error) = self.commit(cx, &todos).await {
            return error;
        }
        ToolResultData::text(with_started(format!("Completed todo #{id}: {subject}"), started))
    }

    /// Persists the list, returning the failure as a ready tool result.
    async fn commit(&self, cx: &ToolContext, todos: &[Todo]) -> Result<(), ToolResultData> {
        let store = self.store.write().await;
        store.write(&cx.session_id, todos).map_err(|error| {
            warn!(%error, "failed to write todos");
            ToolResultData::error(format!("Failed to save todos: {error}"))
        })
    }
}

/// Starts the first open item nothing blocks, unless something is already in progress.
///
/// Marking each item in progress was a call of its own, which made a three-step plan seven calls:
/// GPT models spent more than a quarter of their tool calls on this list that way. Returns the
/// started item's `(id, subject)`.
fn start_next(todos: &mut [Todo]) -> Option<(u32, String)> {
    if todos.iter().any(|todo| todo.status == TodoStatus::InProgress) {
        return None;
    }
    let next = todos.iter().position(|todo| {
        todo.status == TodoStatus::Pending && todo.open_blockers(todos).is_empty()
    })?;
    let todo = &mut todos[next];
    todo.status = TodoStatus::InProgress;
    Some((todo.id, todo.subject.clone()))
}

/// `text`, with a line saying which item [`start_next`] started. A line of its own, because a card
/// reads its item's name off the first.
fn with_started(text: String, started: Option<(u32, String)>) -> String {
    match started {
        Some((id, subject)) => format!("{text}\nStarted todo #{id}: {subject}"),
        None => text,
    }
}

/// The model-facing rendering of the list.
///
/// Markdown-checkbox markers rather than glyphs, because the model reads this output.
fn render_list(todos: &[Todo]) -> String {
    let live: Vec<&Todo> = todos.iter().filter(|todo| todo.status.is_live()).collect();
    if live.is_empty() {
        return "No todos".to_string();
    }
    // An item is "blocked" only while a dependency is still incomplete.
    let completed: std::collections::HashSet<u32> = todos
        .iter()
        .filter(|todo| todo.status == TodoStatus::Completed)
        .map(|todo| todo.id)
        .collect();
    live.iter()
        .map(|todo| {
            let marker = match todo.status {
                TodoStatus::Pending => "[ ]",
                TodoStatus::InProgress => "[~]",
                TodoStatus::Completed => "[x]",
                TodoStatus::Deleted => "[-]",
            };
            let unresolved: Vec<String> = todo
                .blocked_by
                .iter()
                .filter(|id| !completed.contains(id))
                .map(|id| format!("#{id}"))
                .collect();
            if todo.status != TodoStatus::Completed && !unresolved.is_empty() {
                format!(
                    "{marker} #{}: {} (blocked by {})",
                    todo.id,
                    todo.subject,
                    unresolved.join(", ")
                )
            } else {
                format!("{marker} #{}: {}", todo.id, todo.subject)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_status(raw: &str) -> Option<TodoStatus> {
    match raw {
        "pending" => Some(TodoStatus::Pending),
        "in_progress" => Some(TodoStatus::InProgress),
        "completed" => Some(TodoStatus::Completed),
        "deleted" => Some(TodoStatus::Deleted),
        _ => None,
    }
}

/// The subjects an `add` is creating: one via `subject`, or a whole plan via `subjects`.
///
/// Batch creation is the point. With one item per call a six-step plan costs six round-trips, and a
/// model weighing cost against benefit correctly skips planning altogether.
///
/// `subjects` wins only when it holds something. A model under a strict JSON schema sends **every**
/// property, so `{subject: "real text", subjects: []}` is an ordinary single add, and treating the
/// empty array as the answer would refuse it.
fn collect_subjects(args: &serde_json::Value) -> Vec<String> {
    let batch: Vec<String> = args
        .get("subjects")
        .and_then(|v| v.as_array())
        .map(|list| list.iter().filter_map(|v| v.as_str()).filter_map(non_blank).collect())
        .unwrap_or_default();
    if !batch.is_empty() {
        return batch;
    }
    args.get("subject").and_then(|v| v.as_str()).and_then(non_blank).into_iter().collect()
}

/// `text` with its surrounding space gone, or `None` if that leaves nothing.
///
/// The blank is the common case, not the odd one: a strict schema makes the model fill in every
/// optional property, and what it fills them with is `""`.
fn non_blank(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The spinner label a call is setting, under either spelling, or `None` if it set none.
fn active_form_arg(args: &serde_json::Value) -> Option<String> {
    luxe_core::tool::arg(args, &["activeForm", "active_form"])
        .and_then(|value| value.as_str())
        .and_then(non_blank)
}

/// The dependencies a call is declaring, or `None` for "leave them alone".
///
/// An empty list means the latter. Clearing is the reading the schema invites — but a strict-schema
/// model sends `blockedBy: []` on *every* call it makes, which would wipe a real dependency the
/// first time the item was touched for any other reason. Nothing is lost by dropping it: a block
/// clears itself the moment its blocker completes.
fn collect_blockers(args: &serde_json::Value) -> Option<Vec<u32>> {
    let ids: Vec<u32> = luxe_core::tool::arg(args, &["blockedBy", "blocked_by"])?
        .as_array()?
        .iter()
        .filter_map(|value| value.as_u64())
        .filter(|id| *id > 0)
        .map(|id| id as u32)
        .collect();
    (!ids.is_empty()).then_some(ids)
}

#[cfg(test)]
mod tests;
