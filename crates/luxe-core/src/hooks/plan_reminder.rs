//! Plan reminder — tells the agent when its checklist has stopped keeping up with its work.
//!
//! The `todo` tool is the only view a user has of the plan, and it drifts in one direction: a model
//! opens a checklist, works, finishes, and never marks anything. The PLAN panel then shows three
//! items in progress over a task the agent has already reported as done, which is worse than no
//! plan at all — the one surface that promised to say where things stand is the one lying about it.
//!
//! # Why this is a tool result and not a prompt
//!
//! A reminder has to reach the model *mid-turn*, and the transcript a request carries is an
//! **append** to the last one's: every provider that caches matches an exact prefix, so anything
//! that edits history — or the system prompt above it — re-bills the whole conversation at the
//! write rate. A tool result is the one place a fact can be added mid-turn that costs nothing,
//! because it was going to be appended anyway. So this hook returns
//! [`HookOutcome::InjectContext`] at [`HookPoint::AfterToolUse`], which dispatch appends to the
//! result of the call that just ran, and it is durable history from that moment on.
//!
//! # Why it backs off
//!
//! The note is a fact, not an instruction — the agent decides what it is worth. Which means it may
//! decide it is worth nothing, and a reminder that repeats on the same cadence forever after that
//! is noise the model has already learned to skip. So an ignored reminder doubles the interval and
//! touching the checklist resets it: the first one arrives promptly, and a run that genuinely has
//! nothing to update is left alone.

use crate::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint};
use crate::todo::{TodoStatus, TodoStore};
use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::debug;

/// Tool calls a plan may go unchanged for before the first reminder.
///
/// Ten is roughly a step's worth of work: a search, a few reads, an edit and its test. Below that
/// the reminder fires inside the step it is asking about; well above it, a whole task closes out
/// unmarked, which is the thing this exists to catch.
const BASE_INTERVAL: u32 = 10;

/// The longest the interval may grow to after repeated reminders go unheeded.
const MAX_INTERVAL: u32 = 80;

/// Open items named in the note before the rest become a count.
const MAX_LISTED: usize = 4;

/// Whether the reminder is wired in at all. Set `LUXE_PLAN_NUDGE=0` to turn it off.
fn enabled() -> bool {
    std::env::var("LUXE_PLAN_NUDGE").as_deref() != Ok("0")
}

/// How far the checklist has drifted from the work, counted in tool calls.
#[derive(Debug)]
struct Drift {
    /// Calls since the checklist was last touched.
    since: u32,
    /// Calls the next reminder waits for.
    interval: u32,
}

impl Default for Drift {
    fn default() -> Self {
        Self { since: 0, interval: BASE_INTERVAL }
    }
}

/// Reminds the agent that its checklist has not moved while its work has.
#[derive(Debug)]
pub struct PlanReminderHook {
    store: TodoStore,
    drift: Mutex<Drift>,
}

impl PlanReminderHook {
    /// Reminds against the checklists in `store`.
    pub fn new(store: TodoStore) -> Self {
        Self { store, drift: Mutex::new(Drift::default()) }
    }

    /// The note for a session whose plan has fallen behind, or `None` when it has not.
    ///
    /// Reads the durable checklist rather than any in-memory tally, so it describes the same list
    /// the panel draws — including one written by an earlier session and resumed into this one.
    fn note(&self, session_id: &str, since: u32) -> Option<String> {
        let todos = self.store.read(session_id).ok()?;
        let open: Vec<_> = todos.iter().filter(|todo| todo.status.is_open()).collect();
        if open.is_empty() {
            return None;
        }
        let mut out = format!(
            "Plan check: {since} tool calls since the checklist last changed, and it still reads:"
        );
        for todo in open.iter().take(MAX_LISTED) {
            let status = match todo.status {
                TodoStatus::InProgress => "in progress",
                _ => "pending",
            };
            out.push_str(&format!("\n- #{} [{}] {}", todo.id, status, todo.subject));
        }
        if open.len() > MAX_LISTED {
            out.push_str(&format!("\n- … +{} more open", open.len() - MAX_LISTED));
        }
        // The consequence, not the instruction, and both readings of it: a model that has been
        // working one item the whole time is told so in as many words, rather than left to guess
        // whether the note is about it
        out.push_str(
            "\nThat list is the only view the user has of your plan. If you are still working \
             through these, nothing needs to change; a step you have finished but not marked reads \
             to the user as work still outstanding.",
        );
        Some(out)
    }
}

#[async_trait]
impl Hook for PlanReminderHook {
    fn name(&self) -> &'static str {
        "plan-reminder"
    }

    fn interest(&self) -> &[HookPoint] {
        &[HookPoint::AfterToolUse]
    }

    async fn on(&self, _point: HookPoint, ev: &HookEvent, cx: &HookCtx) -> HookOutcome {
        let HookEvent::ToolResult { tool_name, .. } = ev else {
            return HookOutcome::Continue;
        };
        if !enabled() {
            return HookOutcome::Continue;
        }

        let mut drift = self.drift.lock().await;
        // Any `todo` call at all, including a plain `list`. Reading the plan is engagement with
        // it, and a reminder landing on the call after the agent just looked would be answering a
        // question nobody asked.
        if tool_name == "todo" {
            *drift = Drift::default();
            return HookOutcome::Continue;
        }
        // Our own request-budget bookkeeping is not a step in anyone's task, so it does not count
        // as work the plan has fallen behind.
        if crate::activity::is_bookkeeping(tool_name) {
            return HookOutcome::Continue;
        }

        drift.since += 1;
        if drift.since < drift.interval {
            return HookOutcome::Continue;
        }

        let Some(note) = self.note(&cx.session_id, drift.since) else {
            // Nothing open to fall behind. Start the count again rather than firing the moment a
            // checklist appears — a plan opened on call eleven has not drifted from anything.
            drift.since = 0;
            return HookOutcome::Continue;
        };
        debug!(calls = drift.since, "the checklist has not kept up with the work");
        drift.since = 0;
        drift.interval = (drift.interval * 2).min(MAX_INTERVAL);
        HookOutcome::InjectContext(note)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::todo::Todo;

    fn context(session_id: &str) -> HookCtx {
        HookCtx {
            session_id: session_id.to_string(),
            event_tx: tokio::sync::mpsc::channel(1).0,
            hook_registry: crate::hook::HookRegistry::new(),
        }
    }

    fn todo(id: u32, subject: &str, status: TodoStatus) -> Todo {
        Todo { id, subject: subject.to_string(), status, active_form: None, blocked_by: Vec::new() }
    }

    fn result(tool_name: &str) -> HookEvent {
        HookEvent::ToolResult {
            tool_name: tool_name.to_string(),
            result: String::new(),
            is_error: false,
        }
    }

    /// Runs `tool` `count` times and returns every note the hook produced.
    async fn run(hook: &PlanReminderHook, tool: &str, count: u32) -> Vec<String> {
        let mut notes = Vec::new();
        for _ in 0..count {
            if let HookOutcome::InjectContext(note) =
                hook.on(HookPoint::AfterToolUse, &result(tool), &context("session-a")).await
            {
                notes.push(note);
            }
        }
        notes
    }

    fn hook_over(todos: &[Todo]) -> (tempfile::TempDir, PlanReminderHook) {
        let dir = tempfile::tempdir().expect("a temp dir");
        let store = TodoStore::new(dir.path().to_path_buf());
        store.write("session-a", todos).expect("a checklist");
        let hook = PlanReminderHook::new(TodoStore::new(dir.path().to_path_buf()));
        (dir, hook)
    }

    /// The plan drifts in one direction: a model opens a checklist, works, finishes, and never
    /// marks anything. The note has to name what the panel is still showing.
    #[tokio::test]
    async fn a_plan_that_stops_keeping_up_is_reported_once_the_work_piles_up() {
        let (_dir, hook) = hook_over(&[
            todo(1, "Wire the composing event", TodoStatus::InProgress),
            todo(2, "Sort the changes tree", TodoStatus::Pending),
            todo(3, "Run the gate", TodoStatus::Completed),
        ]);

        assert!(run(&hook, "read", BASE_INTERVAL - 1).await.is_empty(), "not before its time");
        let notes = run(&hook, "read", 1).await;
        let [note] = notes.as_slice() else { panic!("one reminder, on the tenth call: {notes:?}") };
        assert!(note.contains("10 tool calls since"), "{note}");
        assert!(note.contains("#1 [in progress] Wire the composing event"), "{note}");
        assert!(note.contains("#2 [pending] Sort the changes tree"), "{note}");
        assert!(!note.contains("Run the gate"), "a finished item is not outstanding work: {note}");
        assert!(note.contains("still working through these, nothing needs to change"), "{note}");
        assert!(note.contains("finished but not marked"), "{note}");
    }

    /// Touching the checklist is the whole point, so it resets everything — the count *and* the
    /// backoff a previous reminder had earned. Any `todo` call at all counts, including a plain
    /// `list`: reading the plan is engagement with it.
    #[tokio::test]
    async fn touching_the_checklist_starts_the_count_again() {
        let (_dir, hook) = hook_over(&[todo(1, "Ship it", TodoStatus::InProgress)]);

        assert_eq!(run(&hook, "read", BASE_INTERVAL * 2).await.len(), 1, "one, then it backs off");
        run(&hook, "todo", 1).await;

        // A clean slate, so the count starts from this call rather than carrying ten with it — and
        // the next reminder is a full base interval away, not the doubled one it had earned.
        assert!(run(&hook, "read", BASE_INTERVAL - 1).await.is_empty(), "the count restarted");
        let notes = run(&hook, "read", 1).await;
        let [note] = notes.as_slice() else { panic!("the base interval is back: {notes:?}") };
        assert!(note.contains("10 tool calls since"), "{note}");
    }

    /// A reminder is a fact the agent may decide is worth nothing. Repeating it on the same cadence
    /// after that is noise it has already learned to skip.
    #[tokio::test]
    async fn a_reminder_nobody_acts_on_gets_rarer_rather_than_louder() {
        let (_dir, hook) = hook_over(&[todo(1, "Ship it", TodoStatus::Pending)]);

        // Four intervals' worth of work: at a fixed cadence that is four reminders.
        let notes = run(&hook, "read", BASE_INTERVAL * 4).await;
        assert_eq!(notes.len(), 2, "10 then 20, not 10-20-30-40: {notes:?}");
        assert!(notes[1].contains("20 tool calls since"), "and it says how long it has been");
    }

    /// Nothing to fall behind, nothing to say. A checklist that opens on the eleventh call has not
    /// drifted from anything, so the count starts again rather than firing at it.
    #[tokio::test]
    async fn a_session_with_no_open_plan_is_left_alone() {
        let (dir, hook) = hook_over(&[todo(1, "Done", TodoStatus::Completed)]);
        assert!(run(&hook, "read", BASE_INTERVAL * 3).await.is_empty());

        TodoStore::new(dir.path().to_path_buf())
            .write("session-a", &[todo(2, "New work", TodoStatus::InProgress)])
            .expect("a checklist");
        assert!(run(&hook, "read", BASE_INTERVAL - 1).await.is_empty(), "a full interval first");
        assert_eq!(run(&hook, "read", 1).await.len(), 1);
    }

    /// Loading a deferred tool's schema is Luxe's own request-budget bookkeeping, not a step in the
    /// task — so it cannot be the work a plan has fallen behind.
    #[tokio::test]
    async fn bookkeeping_calls_are_not_work_the_plan_fell_behind() {
        let (_dir, hook) = hook_over(&[todo(1, "Ship it", TodoStatus::Pending)]);
        assert!(run(&hook, crate::tool::ACTIVATE_TOOLS_NAME, BASE_INTERVAL * 2).await.is_empty());
        assert_eq!(run(&hook, "read", BASE_INTERVAL).await.len(), 1, "real work still counts");
    }

    /// A long plan is named up to the cap and counted past it; one exactly at the cap is whole.
    #[test]
    fn a_long_plan_is_named_then_counted() {
        let open = |count: u32| -> Vec<Todo> {
            (1..=count).map(|id| todo(id, &format!("Step {id}"), TodoStatus::Pending)).collect()
        };
        let (_dir, hook) = hook_over(&open(MAX_LISTED as u32 + 2));
        let note = hook.note("session-a", 10).expect("a note");
        assert!(note.contains("Step 4") && !note.contains("Step 5"), "{note}");
        assert!(note.contains("… +2 more open"), "{note}");

        let (_dir, hook) = hook_over(&open(MAX_LISTED as u32));
        let note = hook.note("session-a", 10).expect("a note");
        assert!(!note.contains("more open"), "{note}");
    }

    /// A session that never wrote a checklist has no plan to be reminded about, and reading one
    /// that is not there is not an error.
    #[tokio::test]
    async fn a_session_with_no_checklist_at_all_says_nothing() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let hook = PlanReminderHook::new(TodoStore::new(dir.path().to_path_buf()));
        assert!(run(&hook, "read", BASE_INTERVAL * 2).await.is_empty());
    }
}
