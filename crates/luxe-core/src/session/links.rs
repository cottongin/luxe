//! Links — the pull request, issue and worktree a session recorded working on.

use super::*;

/// The newest of `sessions` — given newest first — that worked on pull request `number`.
///
/// A session unreadable for any reason is passed over: finding the right one is worth more than
/// stopping at a damaged one.
pub fn session_for_pull_request(
    store: &dyn SessionStore,
    sessions: &[SessionId],
    number: u64,
) -> Option<SessionId> {
    sessions
        .iter()
        .find(|session| {
            store.load(session).is_ok_and(|entries| {
                entries.iter().any(|entry| {
                    matches!(&entry.kind, EntryKind::PullRequestLink { number: linked, .. } if *linked == number)
                })
            })
        })
        .cloned()
}

/// The issue a session took up as its brief, the last one it recorded.
pub fn session_issue(entries: &[Entry]) -> Option<u64> {
    entries.iter().rev().find_map(|entry| match &entry.kind {
        EntryKind::IssueLink { number, .. } => Some(*number),
        _ => None,
    })
}

/// The worktree branch a session works on, the last one it recorded.
pub fn session_worktree(entries: &[Entry]) -> Option<String> {
    entries.iter().rev().find_map(|entry| match &entry.kind {
        EntryKind::Worktree { branch, .. } => Some(branch.clone()),
        _ => None,
    })
}

/// The pull requests a session worked on, in the order it first did.
pub fn linked_pull_requests(entries: &[Entry]) -> Vec<u64> {
    let mut linked = Vec::new();
    for entry in entries {
        if let EntryKind::PullRequestLink { number, .. } = &entry.kind {
            if !linked.contains(number) {
                linked.push(*number);
            }
        }
    }
    linked
}
