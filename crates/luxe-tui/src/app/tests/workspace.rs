//! Workspace — snapshots from the engine, and what they say about the working tree.

use super::*;

#[tokio::test]
async fn workspace_snapshot_rejects_stale_same_scope_revision() {
    use luxe_core::workspace::{ProjectIdentity, WorkspaceScopeId};

    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    let mut current = WorkspaceSnapshot::empty(
        WorkspaceScopeId::new("scope-a"),
        ProjectIdentity::non_git("/project".into(), "project", "/project"),
    );
    current.revision = 2;
    let mut stale = current.clone();
    stale.revision = 1;

    app.update(Msg::Engine(Event::WorkspaceUpdated(Box::new(current))));
    app.update(Msg::Engine(Event::WorkspaceUpdated(Box::new(stale))));

    assert_eq!(app.workspace.as_ref().map(|snapshot| snapshot.revision), Some(2));
}

/// An empty diff against `HEAD` has two causes that a snapshot spells identically, and only one
/// of them means there is nothing to read. The reducer has to tell them apart, because everything
/// downstream that keeps a review reachable after a commit reads this one flag.
#[tokio::test]
async fn an_emptied_working_tree_is_distinguished_from_one_that_was_never_dirty() {
    use luxe_core::workspace::{ProjectIdentity, WorkspaceScopeId};
    use luxe_core::{FileChangeStatus, FileChangeSummary, WorkspaceChangesSummary};

    fn at(revision: u64, changed: usize, head: &str) -> WorkspaceSnapshot {
        let mut identity = ProjectIdentity::non_git("/project".into(), "project", "/project");
        identity.git_head = Some(head.to_owned());
        let mut snapshot = WorkspaceSnapshot::empty(WorkspaceScopeId::new("scope-a"), identity);
        snapshot.revision = revision;
        snapshot.changes = WorkspaceChangesSummary {
            files: (0..changed)
                .map(|nth| FileChangeSummary {
                    path: format!("src/{nth}.rs").into(),
                    status: FileChangeStatus::Modified,
                    additions: Some(1),
                    deletions: Some(0),
                    binary: false,
                })
                .collect(),
            total_files: changed,
            additions: Some(changed as u64),
            deletions: Some(0),
            truncated: false,
        };
        snapshot
    }

    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());

    app.update(Msg::Engine(Event::WorkspaceUpdated(Box::new(at(1, 0, "aaa")))));
    assert!(!app.worktree_emptied, "a clean project has not committed anything away");

    app.update(Msg::Engine(Event::WorkspaceUpdated(Box::new(at(2, 3, "aaa")))));
    assert!(!app.worktree_emptied, "work in progress is still in the working tree");

    // The tree empties and `HEAD` moves in the same breath: that is a commit.
    app.update(Msg::Engine(Event::WorkspaceUpdated(Box::new(at(3, 0, "bbb")))));
    assert!(app.worktree_emptied, "the change went somewhere, and the reader can still read it");

    // It survives the further clean snapshots that arrive while nothing is happening …
    app.update(Msg::Engine(Event::WorkspaceUpdated(Box::new(at(4, 0, "bbb")))));
    assert!(app.worktree_emptied);
    // … and clears the moment there is real work to review again.
    app.update(Msg::Engine(Event::WorkspaceUpdated(Box::new(at(5, 1, "bbb")))));
    assert!(!app.worktree_emptied);

    // A different session starts over: its own working tree has emptied nothing.
    app.update(Msg::Engine(Event::WorkspaceUpdated(Box::new(at(6, 0, "ccc")))));
    assert!(app.worktree_emptied);
    app.update(Msg::Engine(Event::SessionLoaded {
        session_id: "session-next".into(),
        messages: Vec::new(),
    }));
    assert!(!app.worktree_emptied);
}

/// The other way a working tree empties, and the reason `HEAD` has to be part of the test.
///
/// An agent that writes a scratch file, uses it and deletes it leaves exactly the snapshot a
/// commit leaves — dirty, then clean — while the repository's history has not moved. Reported as a
/// commit, the dock offers to review a turn whose changes no longer exist anywhere.
#[tokio::test]
async fn a_change_that_was_undone_is_not_reported_as_committed() {
    use luxe_core::workspace::{ProjectIdentity, WorkspaceScopeId};
    use luxe_core::{FileChangeStatus, FileChangeSummary, WorkspaceChangesSummary};

    fn at(revision: u64, changed: usize, head: &str) -> WorkspaceSnapshot {
        let mut identity = ProjectIdentity::non_git("/project".into(), "project", "/project");
        identity.git_head = Some(head.to_owned());
        let mut snapshot = WorkspaceSnapshot::empty(WorkspaceScopeId::new("scope-b"), identity);
        snapshot.revision = revision;
        snapshot.changes = WorkspaceChangesSummary {
            files: (0..changed)
                .map(|nth| FileChangeSummary {
                    path: format!("scratch/{nth}.txt").into(),
                    status: FileChangeStatus::Added,
                    additions: Some(1),
                    deletions: Some(0),
                    binary: false,
                })
                .collect(),
            total_files: changed,
            additions: Some(changed as u64),
            deletions: Some(0),
            truncated: false,
        };
        snapshot
    }

    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());

    app.update(Msg::Engine(Event::WorkspaceUpdated(Box::new(at(1, 0, "aaa")))));
    app.update(Msg::Engine(Event::WorkspaceUpdated(Box::new(at(2, 2, "aaa")))));
    assert!(!app.worktree_emptied);

    // Same `HEAD`, empty tree: the work was undone, not recorded.
    app.update(Msg::Engine(Event::WorkspaceUpdated(Box::new(at(3, 0, "aaa")))));
    assert!(
        !app.worktree_emptied,
        "nothing was committed, so there is no turn left to point the reader at",
    );
}

#[tokio::test]
async fn workspace_snapshot_rejects_retired_scope() {
    use luxe_core::workspace::{ProjectIdentity, WorkspaceScopeId};

    fn snapshot(scope: &str, revision: u64) -> WorkspaceSnapshot {
        let mut snapshot = WorkspaceSnapshot::empty(
            WorkspaceScopeId::new(scope),
            ProjectIdentity::non_git("/project".into(), "project", "/project"),
        );
        snapshot.revision = revision;
        snapshot
    }

    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.update(Msg::Engine(Event::WorkspaceUpdated(Box::new(snapshot("scope-a", 1)))));
    app.update(Msg::Engine(Event::WorkspaceUpdated(Box::new(snapshot("scope-b", 1)))));
    app.update(Msg::Engine(Event::WorkspaceUpdated(Box::new(snapshot("scope-a", 2)))));

    assert_eq!(
        app.workspace.as_ref().map(|snapshot| snapshot.scope_id.0.as_str()),
        Some("scope-b")
    );
    assert!(app.retired_workspace_scopes.contains(&WorkspaceScopeId::new("scope-a")));
}

#[tokio::test]
async fn session_load_clears_activity_from_the_previous_session() {
    let (tx, _rx) = mpsc::channel(100);
    let mut app = App::new(tx, KeyMap::defaults(), Config::default());
    app.session_id = Some("session-a".into());
    app.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(ToolActivity::running(
        "session-a",
        "call-a",
        "write",
        "Write file",
        1,
    )))));
    app.update(Msg::Engine(Event::SessionLoaded {
        session_id: "session-b".into(),
        messages: vec![luxe_core::event::SessionMessage {
            entry_id: luxe_core::session::EntryId("entry-1".into()),
            message: luxe_core::message::Message::assistant("resumed"),
        }],
    }));
    app.update(Msg::Engine(Event::ToolActivityUpdated(Box::new(ToolActivity::running(
        "session-a",
        "call-a",
        "write",
        "late update",
        2,
    )))));

    assert_eq!(app.session_id.as_deref(), Some("session-b"));
    assert!(app.tool_activity.is_empty());
    assert!(app.tool_items.is_empty());
    assert_eq!(app.convo.len(), 1);
}
