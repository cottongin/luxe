//! Dashboard data — bounded startup metadata collection.
//!
//! Disk access stays at the binary boundary so `luxe-tui` remains a pure renderer and
//! never depends on concrete tools or storage backends.

use crate::git_probe::run as run_git;
use luxe_core::message::{ContentBlock, Role};
use luxe_core::session::{Entry, EntryKind, SessionStore};
use luxe_tui::dashboard::{
    DashBriefing, DashboardState, PreviewRole, PreviewTurn, ResumeTarget, SessionRecord,
    SessionSummary,
};
use std::collections::HashSet;
use std::path::Path;
use std::time::SystemTime;

const SESSION_SCAN_LIMIT: usize = 40;
const SESSION_SEARCH_TEXT_LIMIT: usize = 16 * 1024;
/// Preview budget: enough role-tagged turns to fill a tall pane without loading a whole
/// megabyte-scale transcript into memory for every catalog row.
const PREVIEW_MAX_TURNS: usize = 40;
const PREVIEW_MAX_CHARS: usize = 6_000;
const PREVIEW_MAX_TURN_CHARS: usize = 1_600;
/// A single tool-call summary line stays short so it reads as a one-liner in the preview.
const PREVIEW_TOOL_SUMMARY_CHARS: usize = 120;
/// Rows the recent list shows, after duplicates are folded out.
const RECENT_ROWS: usize = 5;

/// Metadata prepared before the dashboard enters the TUI loop.
pub(crate) struct DashboardData {
    /// Recent sessions the empty prompt lists
    pub(crate) recent: Vec<SessionSummary>,
    /// Searchable session catalog the prompt and the session pickers search
    pub(crate) sessions: Vec<SessionSummary>,
    /// The session the Resume row offers, when the project has one worth resuming
    pub(crate) resume: Option<ResumeTarget>,
    /// The working tree's changes, for the review row
    pub(crate) briefing: DashBriefing,
}

/// Collects dashboard metadata without blocking the TUI thread.
pub(crate) async fn collect<S: SessionStore + 'static>(
    store: S,
    repo_root: &Path,
) -> DashboardData {
    let session_task = tokio::task::spawn_blocking(move || collect_session_catalog(&store));
    let (catalog, changes) = tokio::join!(session_task, changed_files(repo_root));
    let (catalog, resume) = match catalog {
        Ok(catalog) => catalog,
        Err(error) => {
            tracing::warn!(error = %error, "dashboard: session scan task failed");
            (Vec::new(), None)
        }
    };
    let (changed_files, added, removed) = changes;
    let briefing = DashBriefing { changed_files, added, removed };
    let recent = dedupe_recent(&catalog);
    DashboardData { recent, sessions: catalog, resume, briefing }
}

/// The searchable session catalog alone, for `/sessions` in a running session.
pub(crate) async fn session_catalog<S: SessionStore + 'static>(store: S) -> Vec<SessionSummary> {
    match tokio::task::spawn_blocking(move || collect_session_catalog(&store)).await {
        Ok((catalog, _)) => catalog,
        Err(error) => {
            tracing::warn!(error = %error, "session scan task failed");
            Vec::new()
        }
    }
}

/// Uncommitted work: how many files, and the lines added and removed across them.
async fn changed_files(repo_root: &Path) -> (usize, u64, u64) {
    let files = run_git(repo_root, &["status", "--porcelain", "--untracked-files=normal"])
        .await
        .map(|status| status.lines().filter(|line| !line.trim().is_empty()).count())
        .unwrap_or(0);
    if files == 0 {
        return (0, 0, 0);
    }
    // Tracked files only: an untracked file has no diff to count, and `--numstat` says nothing
    // about it. The file count above already includes it.
    let (added, removed) = run_git(repo_root, &["diff", "HEAD", "--numstat"])
        .await
        .map(|numstat| {
            numstat.lines().fold((0u64, 0u64), |(added, removed), line| {
                let mut fields = line.split_whitespace();
                let parse = |field: Option<&str>| field.and_then(|f| f.parse::<u64>().ok());
                match (parse(fields.next()), parse(fields.next())) {
                    (Some(plus), Some(minus)) => (added + plus, removed + minus),
                    // A binary file's counts are `-`, which is not a line count in either column.
                    _ => (added, removed),
                }
            })
        })
        .unwrap_or((0, 0));
    (files, added, removed)
}

/// The recent list, one row per distinct title.
///
/// The same prompt run three times must not fill the list with three identical rows, the worst
/// possible use of five lines: it hides three other sessions *and* tells you nothing about the one
/// it repeats.
fn dedupe_recent(catalog: &[SessionSummary]) -> Vec<SessionSummary> {
    let mut seen = HashSet::new();
    catalog
        .iter()
        .filter(|session| seen.insert(session.name.to_lowercase()))
        .take(RECENT_ROWS)
        .cloned()
        .collect()
}

/// The catalog, plus the newest session worth offering to resume.
fn collect_session_catalog(
    store: &impl SessionStore,
) -> (Vec<SessionSummary>, Option<ResumeTarget>) {
    let mut resume: Option<(SystemTime, ResumeTarget)> = None;
    let sessions: Vec<_> = store
        .recent_sessions(SESSION_SCAN_LIMIT)
        .into_iter()
        .filter_map(|(id, modified)| {
            let entries = match store.load(&id) {
                Ok(entries) => entries,
                Err(error) => {
                    tracing::warn!(session = %id, error = %error, "dashboard: failed to load session");
                    return None;
                }
            };
            let title = first_user_text(&entries)?;
            // The newest session that actually holds a conversation. An empty one is a resume
            // target that opens onto nothing.
            if resume.as_ref().is_none_or(|(seen, _)| modified > *seen) {
                resume = Some((
                    modified,
                    ResumeTarget {
                        id: id.0.clone(),
                        title: normalize(&title),
                        age: String::new(),
                    },
                ));
            }
            // The pull requests it worked on: on the row, and findable by `#146`
            let linked = luxe_core::session::linked_pull_requests(&entries);
            let search = format!("{}{}", pull_request_tags(&linked), searchable_text(&entries));
            let title = if linked.is_empty() {
                title
            } else {
                format!("{} · {}", normalize(&title), pull_request_tags(&linked).trim_end())
            };
            Some(SessionRecord {
                id: id.0,
                modified,
                title,
                search_text: search,
                preview: session_preview(&entries),
                worktree: luxe_core::session::session_worktree(&entries),
            })
        })
        .collect();

    let now = SystemTime::now();
    let resume = resume.map(|(modified, mut target)| {
        target.age = luxe_tui::dashboard::relative_age(modified, now);
        target
    });
    (DashboardState::populate_session_catalog(sessions, now), resume)
}

/// Collapses whitespace so a multi-line first message reads as one title.
fn normalize(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first non-empty user message text in a session, used to derive a human title.
pub(crate) fn first_user_text(entries: &[Entry]) -> Option<String> {
    entries.iter().find_map(|entry| match &entry.kind {
        EntryKind::Message(message) if message.role == Role::User => {
            message.content.iter().find_map(|block| match block {
                ContentBlock::Text(text) if !text.trim().is_empty() => Some(text.clone()),
                _ => None,
            })
        }
        _ => None,
    })
}

/// Builds a bounded, role-tagged preview transcript for the session picker's preview pane.
///
/// Walks messages in order, emitting a turn per contiguous text block (user/assistant) and per
/// assistant tool call, preserving conversation order. System prompts and tool *results* are
/// skipped — they add noise the loaded conversation also de-emphasizes. Bounded by turn count,
/// total characters, and per-turn length so a giant transcript can't blow up the catalog.
fn session_preview(entries: &[Entry]) -> Vec<PreviewTurn> {
    let mut turns: Vec<PreviewTurn> = Vec::new();
    let mut total = 0usize;
    for entry in entries {
        let EntryKind::Message(message) = &entry.kind else { continue };
        let text_role = match message.role {
            Role::User => PreviewRole::User,
            Role::Assistant => PreviewRole::Assistant,
            // System prompt / tool-result messages are noise in a resume preview.
            Role::System | Role::Tool => continue,
        };
        for block in &message.content {
            if turns.len() >= PREVIEW_MAX_TURNS || total >= PREVIEW_MAX_CHARS {
                return turns;
            }
            match block {
                ContentBlock::Text(text) if !text.trim().is_empty() => {
                    push_preview_turn(&mut turns, &mut total, text_role, text);
                }
                ContentBlock::ToolCall { name, arguments, .. } => {
                    let summary = tool_call_summary(name, arguments);
                    push_preview_turn(&mut turns, &mut total, PreviewRole::Tool, &summary);
                }
                _ => {}
            }
        }
    }
    turns
}

/// Pushes one clamped turn, trimming surrounding whitespace and capping its length so a single
/// long message can't dominate the preview. Advances the running character budget.
fn push_preview_turn(
    turns: &mut Vec<PreviewTurn>,
    total: &mut usize,
    role: PreviewRole,
    text: &str,
) {
    let clamped = clamp_chars(text.trim(), PREVIEW_MAX_TURN_CHARS);
    *total += clamped.chars().count();
    turns.push(PreviewTurn { role, text: clamped });
}

/// A compact `name(key: value, …)` one-liner for a tool call, capped in length. Object arguments
/// list their fields; other JSON shapes render inline. Keeps the preview readable without dumping
/// whole tool payloads.
fn tool_call_summary(name: &str, arguments: &serde_json::Value) -> String {
    let rendered = match arguments {
        serde_json::Value::Object(map) if !map.is_empty() => {
            let fields = map
                .iter()
                .map(|(key, value)| format!("{key}: {}", compact_json(value)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{name}({fields})")
        }
        serde_json::Value::Null => name.to_string(),
        other => format!("{name}({})", compact_json(other)),
    };
    clamp_chars(&rendered, PREVIEW_TOOL_SUMMARY_CHARS)
}

/// Renders a JSON value as a short scalar for a tool-call summary: strings unquoted, containers
/// abbreviated to `{…}` / `[…]` so a nested payload stays a one-liner.
fn compact_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.replace('\n', " "),
        serde_json::Value::Object(_) => "{…}".to_string(),
        serde_json::Value::Array(_) => "[…]".to_string(),
        other => other.to_string(),
    }
}

/// Truncates to `max` characters (by `char`, so multi-byte text is safe), appending `…` on cut.
fn clamp_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// `#146 #7 `, one tag per pull request a session worked on.
fn pull_request_tags(linked: &[u64]) -> String {
    linked.iter().map(|number| format!("#{number} ")).collect()
}

fn searchable_text(entries: &[Entry]) -> String {
    entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            EntryKind::Message(message) => Some(&message.content),
            _ => None,
        })
        .flatten()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .flat_map(|text| text.chars().chain(std::iter::once(' ')))
        .take(SESSION_SEARCH_TEXT_LIMIT)
        .collect()
}

#[cfg(test)]
mod tests;
