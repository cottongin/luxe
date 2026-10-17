use super::*;

#[test]
fn empty_snapshot_roundtrips() {
    let snapshot = WorkspaceSnapshot::empty(
        WorkspaceScopeId::new("scope-1"),
        ProjectIdentity::non_git("/work/luxe".into(), "luxe", "~/work/luxe"),
    );

    let encoded = serde_json::to_string(&snapshot).unwrap();
    let decoded: WorkspaceSnapshot = serde_json::from_str(&encoded).unwrap();

    assert_eq!(decoded, snapshot);
    assert_eq!(decoded.todos.total, 0);
    assert!(!decoded.changes.truncated);
    assert!(decoded.diagnostics.is_empty());
    assert!(decoded.external.is_empty(), "a session that stayed home reports no elsewhere");
}

/// `external` was added after front-ends shipped, and both enums on this wire degrade an
/// unknown tag rather than erroring — so a snapshot written before it existed has to keep
/// decoding, or every persisted and in-flight frame from an older peer becomes an error.
#[test]
fn a_snapshot_without_the_external_section_still_decodes() {
    let snapshot = WorkspaceSnapshot::empty(
        WorkspaceScopeId::new("scope-1"),
        ProjectIdentity::non_git("/work/luxe".into(), "luxe", "~/work/luxe"),
    );
    let mut encoded = serde_json::to_value(&snapshot).unwrap();
    encoded.as_object_mut().unwrap().remove("external").expect("the field is written");

    let decoded: WorkspaceSnapshot = serde_json::from_value(encoded).unwrap();
    assert!(decoded.external.is_empty());
    assert_eq!(decoded, snapshot);
}

#[test]
fn populated_snapshot_roundtrips_every_summary() {
    let snapshot = WorkspaceSnapshot {
        scope_id: WorkspaceScopeId::new("scope-2"),
        revision: 7,
        project: ProjectIdentity {
            root: "/work/luxe".into(),
            display_name: "luxe".into(),
            display_path: "~/work/luxe".into(),
            git_branch: Some("main".into()),
            git_dirty: true,
            git_head: Some("1f3a9c2".into()),
        },
        session: Some(SessionSummary {
            id: "session-1".into(),
            title: "Workspace contracts".into(),
            updated_at_ms: 10,
        }),
        todos: TodoSummary {
            current: Some(TodoSummaryItem {
                id: "2".into(),
                subject: "Add contracts".into(),
                status: TodoSummaryStatus::InProgress,
                blocked_by: Vec::new(),
            }),
            items: vec![TodoSummaryItem {
                id: "1".into(),
                subject: "Plan".into(),
                status: TodoSummaryStatus::Completed,
                blocked_by: Vec::new(),
            }],
            pending: 1,
            in_progress: 1,
            completed: 1,
            blocked: 1,
            total: 5,
            truncated: true,
        },
        changes: WorkspaceChangesSummary {
            files: vec![FileChangeSummary {
                path: "crates/luxe-core/src/workspace.rs".into(),
                status: FileChangeStatus::Added,
                additions: Some(1),
                deletions: None,
                binary: false,
            }],
            total_files: 2,
            additions: Some(1),
            deletions: None,
            truncated: true,
        },
        external: ExternalChangesSummary {
            roots: vec![ExternalRootSummary {
                root: "/work/sibling".into(),
                display_path: "~/work/sibling".into(),
                git_branch: Some("main".into()),
                git: true,
                files: vec![FileChangeSummary {
                    path: "src/lib.rs".into(),
                    status: FileChangeStatus::Modified,
                    additions: Some(3),
                    deletions: Some(1),
                    binary: false,
                }],
                total_files: 1,
                truncated: false,
            }],
            total_files: 1,
            additions: Some(3),
            deletions: Some(1),
            truncated: false,
        },
        review: Some(ReviewSummary {
            id: "review-1".into(),
            title: "Refactor workspace snapshots review".into(),
            source: "working tree".into(),
            unresolved_findings: 1,
            completed_at_ms: 12,
        }),
        diagnostics: DiagnosticsSummary {
            errors: 2,
            warnings: 1,
            infos: 0,
            hints: 0,
            files: vec![FileDiagnostics {
                path: "crates/luxe-core/src/workspace.rs".into(),
                errors: 2,
                warnings: 1,
                infos: 0,
                hints: 0,
            }],
            total_files: 1,
            truncated: false,
        },
        tooling: ToolingSummary {
            servers: vec![LanguageServerStatus {
                language: "rust".into(),
                command: "rust-analyzer".into(),
                extensions: vec!["rs".into()],
                from_config: false,
                install_hint: Some("rustup component add rust-analyzer".into()),
                installed: true,
                running: true,
            }],
            adapters: vec![DebugAdapterStatus {
                id: "debugpy".into(),
                command: "python3 -m debugpy.adapter".into(),
                extensions: vec!["py".into()],
                from_config: false,
                installed: true,
            }],
            debug_sessions: vec![DebugSessionStatus {
                id: "dbg-1".into(),
                adapter: "debugpy".into(),
                program: "/tmp/app.py".into(),
                terminated: false,
                stopped: false,
                stop_reason: None,
            }],
            mcp_servers: vec![McpServerStatus {
                id: "demo".into(),
                transport: "stdio".into(),
                auth: "none".into(),
                tool_count: Some(2),
                enabled: true,
                tools: vec!["echo".into(), "add".into()],
                signed_in: None,
            }],
            lsp_config_path: Some("/home/x/.config/luxe/lsp-servers.json".into()),
            adapters_config_path: Some("/home/x/.config/luxe/debug-adapters.json".into()),
            tools: Vec::new(),
        },
        pull_request: PullRequestPresence::None,
        worktree: Some(".luxe/worktrees/topic".into()),
    };

    let encoded = serde_json::to_string(&snapshot).unwrap();
    let decoded: WorkspaceSnapshot = serde_json::from_str(&encoded).unwrap();

    assert_eq!(decoded, snapshot);
    assert_eq!(decoded.todos.total, 5);
    assert!(decoded.changes.truncated);
    assert_eq!(decoded.diagnostics.total(), 3);
    assert_eq!(decoded.diagnostics.files[0].errors, 2);
    assert_eq!(decoded.tooling.servers[0].language, "rust");
    assert!(decoded.tooling.servers[0].running);
    assert_eq!(decoded.tooling.debug_sessions[0].id, "dbg-1");
    assert!(!decoded.external.is_empty());
    assert_eq!(decoded.external.roots[0].root, PathBuf::from("/work/sibling"));
    assert!(decoded.external.roots[0].git, "a checkout is reported by Git, not by the ledger");
    assert_eq!(decoded.external.roots[0].files[0].path, PathBuf::from("src/lib.rs"));
}

/// `--rpc` clients read this, so its shape is pinned against literals rather than round-tripped:
/// a round trip sails straight through a rename.
#[test]
fn the_pull_request_presence_wire_shape_is_pinned() {
    use crate::github::{CheckRollup, PullRequestState, PullRequestStatus, ReviewDecision};
    let found = PullRequestPresence::Found(PullRequestStatus {
        number: 138,
        title: "Groundwork".into(),
        url: "https://github.com/acme/app/pull/138".into(),
        state: PullRequestState::Open,
        draft: false,
        review_decision: Some(ReviewDecision::ChangesRequested),
        checks: CheckRollup { passed: 4, failed: 1, pending: 0, skipped: 0 },
        unresolved_threads: 2,
        head_oid: "3992868".into(),
        stack: None,
    });
    let pinned = [
        (PullRequestPresence::Unknown, r#"{"kind":"unknown"}"#),
        (
            PullRequestPresence::Unavailable { reason: "no gh".into() },
            r#"{"kind":"unavailable","reason":"no gh"}"#,
        ),
        (PullRequestPresence::None, r#"{"kind":"none"}"#),
        (
            found,
            r#"{"kind":"found","number":138,"title":"Groundwork","url":"https://github.com/acme/app/pull/138","state":"open","draft":false,"review_decision":"changes_requested","checks":{"passed":4,"failed":1,"pending":0,"skipped":0},"unresolved_threads":2,"head_oid":"3992868"}"#,
        ),
    ];
    for (presence, wire) in pinned {
        assert_eq!(serde_json::to_string(&presence).unwrap(), wire);
        assert_eq!(serde_json::from_str::<PullRequestPresence>(wire).unwrap(), presence);
    }
}

/// `--rpc` clients read the `/mcp` rows too, so they are pinned the same way; and a row from a
/// Luxe that predates `tools` and `signed_in` still reads, as nothing discovered and no sign-in.
#[test]
fn the_mcp_server_status_wire_shape_is_pinned() {
    let status = McpServerStatus {
        id: "asana".into(),
        transport: "http".into(),
        auth: "oauth".into(),
        tool_count: Some(2),
        enabled: true,
        tools: vec!["get_tasks".into(), "create_task".into()],
        signed_in: Some(true),
    };
    let wire = r#"{"id":"asana","transport":"http","auth":"oauth","tool_count":2,"enabled":true,"tools":["get_tasks","create_task"],"signed_in":true}"#;
    assert_eq!(serde_json::to_string(&status).unwrap(), wire);
    assert_eq!(serde_json::from_str::<McpServerStatus>(wire).unwrap(), status);

    let older =
        r#"{"id":"demo","transport":"stdio","auth":"none","tool_count":null,"enabled":true}"#;
    let read = serde_json::from_str::<McpServerStatus>(older).unwrap();
    assert_eq!((read.tools.len(), read.signed_in), (0, None));
}

/// A snapshot from a Luxe that predates the field still reads, as not asked yet.
#[test]
fn a_snapshot_without_a_pull_request_reads_as_unknown() {
    let snapshot = WorkspaceSnapshot::empty(
        WorkspaceScopeId::new("scope"),
        ProjectIdentity {
            root: PathBuf::from("/work/app"),
            display_name: "app".into(),
            display_path: "~/work/app".into(),
            git_branch: None,
            git_dirty: false,
            git_head: None,
        },
    );
    let mut value = serde_json::to_value(&snapshot).unwrap();
    value.as_object_mut().unwrap().remove("pull_request");
    let decoded: WorkspaceSnapshot = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.pull_request, PullRequestPresence::Unknown);
    assert_eq!(decoded.worktree, None);
}

#[test]
fn all_workspace_enums_roundtrip() {
    for value in [
        FileChangeStatus::Added,
        FileChangeStatus::Modified,
        FileChangeStatus::Deleted,
        FileChangeStatus::Renamed,
        FileChangeStatus::Untracked,
    ] {
        let encoded = serde_json::to_string(&value).unwrap();
        assert_eq!(serde_json::from_str::<FileChangeStatus>(&encoded).unwrap(), value);
    }
    for value in [
        TodoSummaryStatus::Pending,
        TodoSummaryStatus::InProgress,
        TodoSummaryStatus::Completed,
        TodoSummaryStatus::Blocked,
    ] {
        let encoded = serde_json::to_string(&value).unwrap();
        assert_eq!(serde_json::from_str::<TodoSummaryStatus>(&encoded).unwrap(), value);
    }
}
