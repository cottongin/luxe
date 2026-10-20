use super::*;
use luxe_core::{
    FileChangeStatus, FileChangeSummary, ProjectIdentity, SessionSummary, TodoSummary,
    TodoSummaryItem, TodoSummaryStatus, WorkspaceChangesSummary, WorkspaceScopeId,
};

fn snapshot() -> WorkspaceSnapshot {
    WorkspaceSnapshot::empty(
        WorkspaceScopeId::new("dock-sections"),
        ProjectIdentity {
            root: "/work/luxe".into(),
            display_name: "luxe".into(),
            display_path: "~/work/luxe".into(),
            git_branch: Some("feature/context-dock".into()),
            git_dirty: true,
            git_head: None,
        },
    )
}

#[test]
fn identity_is_project_and_git_state_only() {
    let mut snapshot = snapshot();
    // The session is present in the snapshot but deliberately absent from identity: an
    // auto-derived title isn't worth a row.
    snapshot.session = Some(SessionSummary {
        id: "session-1".into(),
        title: "Implement the dock".into(),
        updated_at_ms: 1,
    });

    assert_eq!(
        DockIdentity::from_snapshot(&snapshot),
        DockIdentity {
            project_name: "luxe".into(),
            branch: Some("feature/context-dock".into()),
            dirty: true,
            pull_request: None,
        }
    );
}

/// The branch's pull request joins the identity in the statusline's words, and only once
/// there is one: not asked yet, none, or unavailable all draw no row.
#[test]
fn identity_carries_the_pull_request_only_when_there_is_one() {
    use luxe_core::github::{CheckRollup, PullRequestState, PullRequestStatus};
    use luxe_core::workspace::PullRequestPresence;
    let mut snapshot = snapshot();
    for absent in [
        PullRequestPresence::Unknown,
        PullRequestPresence::None,
        PullRequestPresence::Unavailable { reason: "no gh".into() },
    ] {
        snapshot.pull_request = absent;
        assert_eq!(DockIdentity::from_snapshot(&snapshot).pull_request, None);
    }
    snapshot.pull_request = PullRequestPresence::Found(PullRequestStatus {
        number: 12,
        title: "t".into(),
        url: "u".into(),
        state: PullRequestState::Open,
        draft: false,
        review_decision: None,
        checks: CheckRollup { failed: 1, ..CheckRollup::default() },
        unresolved_threads: 0,
        head_oid: "a".into(),
        stack: None,
    });
    assert_eq!(
        DockIdentity::from_snapshot(&snapshot).pull_request.as_deref(),
        Some("#12 · 1 failing")
    );
}

#[test]
fn identity_supports_non_git_projects_without_placeholder_rows() {
    let snapshot = WorkspaceSnapshot::empty(
        WorkspaceScopeId::new("non-git"),
        ProjectIdentity::non_git("/work/notes".into(), "notes", "~/work/notes"),
    );

    let identity = DockIdentity::from_snapshot(&snapshot);
    assert_eq!(identity.project_name, "notes");
    assert_eq!(identity.branch, None);
    assert!(!identity.dirty);
}

#[test]
fn plan_keeps_blocked_rows_and_an_exact_truncation_count() {
    let mut snapshot = snapshot();
    snapshot.todos = TodoSummary {
        current: Some(TodoSummaryItem {
            id: "2".into(),
            subject: "Implement the Context Dock".into(),
            status: TodoSummaryStatus::InProgress,
            blocked_by: Vec::new(),
        }),
        items: vec![
            TodoSummaryItem {
                id: "2".into(),
                subject: "Implement the Context Dock".into(),
                status: TodoSummaryStatus::InProgress,
                blocked_by: Vec::new(),
            },
            TodoSummaryItem {
                id: "3".into(),
                subject: "Review the new layout".into(),
                status: TodoSummaryStatus::Blocked,
                blocked_by: vec!["2".into()],
            },
        ],
        pending: 1,
        in_progress: 1,
        completed: 8,
        blocked: 1,
        total: 11,
        truncated: true,
    };

    let plan = DockPlan::from_snapshot(&snapshot).expect("non-empty todos show Plan");
    assert_eq!(plan.items[0].subject, "Implement the Context Dock");
    assert_eq!(plan.items[1].status, TodoSummaryStatus::Blocked);
    assert_eq!((plan.pending, plan.in_progress, plan.completed), (1, 1, 8));
    assert_eq!(plan.remaining, Some(9));
}

#[test]
fn plan_is_omitted_when_the_snapshot_has_no_todos() {
    assert_eq!(DockPlan::from_snapshot(&snapshot()), None);
}

#[test]
fn changes_preserve_order_status_unknown_totals_and_remaining_files() {
    let mut snapshot = snapshot();
    snapshot.changes = WorkspaceChangesSummary {
        files: vec![
            FileChangeSummary {
                path: "src/lib.rs".into(),
                status: FileChangeStatus::Modified,
                additions: Some(12),
                deletions: Some(3),
                binary: false,
            },
            FileChangeSummary {
                path: "assets/logo.png".into(),
                status: FileChangeStatus::Added,
                additions: None,
                deletions: None,
                binary: true,
            },
            FileChangeSummary {
                path: "old-name.rs".into(),
                status: FileChangeStatus::Deleted,
                additions: Some(0),
                deletions: Some(8),
                binary: false,
            },
        ],
        total_files: 5,
        additions: None,
        deletions: None,
        truncated: true,
    };

    let changes = DockChanges::from_snapshot(&snapshot).expect("non-empty changes show");
    assert_eq!(
        changes.files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>(),
        vec!["src/lib.rs", "assets/logo.png", "old-name.rs"]
    );
    assert_eq!(
        changes.files.iter().map(|file| file.status).collect::<Vec<_>>(),
        vec![FileChangeStatus::Modified, FileChangeStatus::Added, FileChangeStatus::Deleted]
    );
    // Git says nothing about a blob, and the row renders that silence by printing no counts —
    // `+0 −0` would claim the file did not change.
    assert_eq!((changes.files[1].additions, changes.files[1].deletions), (None, None));
    assert_eq!((changes.additions, changes.deletions, changes.remaining), (None, None, Some(2)));
}

#[test]
fn changes_are_omitted_when_the_snapshot_has_no_files() {
    assert_eq!(DockChanges::from_snapshot(&snapshot()), None);
}

fn changes_of(paths: &[(&str, u64, u64)]) -> DockChanges {
    DockChanges {
        files: paths
            .iter()
            .map(|(path, additions, deletions)| DockChangeItem {
                path: (*path).to_string(),
                status: FileChangeStatus::Modified,
                additions: Some(*additions),
                deletions: Some(*deletions),
            })
            .collect(),
        total_files: paths.len(),
        additions: Some(paths.iter().map(|(_, a, _)| a).sum()),
        deletions: Some(paths.iter().map(|(_, _, d)| d).sum()),
        remaining: None,
    }
}

fn shape(rows: &[DockChangeRow]) -> Vec<String> {
    rows.iter()
        .map(|row| match row {
            DockChangeRow::Directory(group) => format!("dir {}", group.directory),
            DockChangeRow::File { name, .. } => format!("file {name}"),
        })
        .collect()
}

/// A directory heading, then its files. This is what the section withheld: the names.
#[test]
fn rows_list_files_under_their_directory_alphabetically() {
    let changes = changes_of(&[
        ("src/app/tests.rs", 15, 0),
        ("src/main.rs", 14, 0),
        ("src/app/mod.rs", 7, 0),
    ]);
    assert_eq!(
        shape(&changes.rows(None, 20)),
        vec![
            "dir src".to_string(),
            "file main.rs".into(),
            "dir src/app".into(),
            "file mod.rs".into(),
            "file tests.rs".into(),
        ]
    );
}

/// The rail redraws on every tool call, so an order derived from the work rearranges itself
/// under a reader following one file. A name does not move: neither the counts nor the review
/// state may touch the order.
#[test]
fn the_order_survives_the_work_changing_underneath_it() {
    use luxe_core::guided_review::SeenReview;
    let paths = ["src/zebra.rs", "README.md", "src/alpha.rs", "docs/guide.md"];
    let small = changes_of(&paths.map(|path| (path, 1, 0)));
    let lopsided = changes_of(&[
        ("src/zebra.rs", 900, 400),
        ("README.md", 1, 0),
        ("src/alpha.rs", 2, 7),
        ("docs/guide.md", 40, 0),
    ]);
    let expected = vec![
        "dir .".to_string(),
        "file README.md".into(),
        "dir docs".into(),
        "file guide.md".into(),
        "dir src".into(),
        "file alpha.rs".into(),
        "file zebra.rs".into(),
    ];

    assert_eq!(shape(&small.rows(None, 20)), expected);
    assert_eq!(shape(&lopsided.rows(None, 20)), expected, "counts do not reorder anything");

    // Nor does reviewing half of it: a row must not walk off down the list the moment it is read.
    let mut seen = SeenReview::default();
    seen.files.insert("src/zebra.rs".into(), (900, 400));
    seen.files.insert("README.md".into(), (1, 0));
    assert_eq!(shape(&lopsided.rows(Some(&seen), 20)), expected);
}

/// `README.md` and `readme.md` belong under the same letter, which a byte comparison denies.
#[test]
fn case_does_not_decide_where_a_name_files() {
    let changes = changes_of(&[("beta.rs", 1, 0), ("Alpha.rs", 1, 0), ("gamma.rs", 1, 0)]);
    assert_eq!(
        shape(&changes.rows(None, 20)),
        vec![
            "dir .".to_string(),
            "file Alpha.rs".into(),
            "file beta.rs".into(),
            "file gamma.rs".into(),
        ]
    );
}

/// A deep path keeps its heading rather than folding into a file row that cannot be read.
///
/// Collapsing saves a row and spends the name: at forty-two cells
/// `crates/luxe-core/src/usage.rs` elides to `crates/luxe-c…/src/usage.rs`, where a heading
/// over a bare `usage.rs` costs one row and stays legible.
#[test]
fn a_single_file_directory_still_gets_its_heading() {
    let changes = changes_of(&[("crates/luxe-core/src/usage.rs", 200, 0)]);
    assert_eq!(
        shape(&changes.rows(None, 20)),
        vec!["dir crates/luxe-core/src".to_string(), "file usage.rs".into()]
    );
}

/// The budget bounds rows, and the ones it cuts are the tail of the list — not an arbitrary
/// slice of it.
#[test]
fn rows_are_bounded_from_the_top_of_the_list() {
    let files: Vec<(&str, u64, u64)> =
        vec![("src/a.rs", 9, 0), ("src/b.rs", 8, 0), ("src/c.rs", 7, 0), ("src/d.rs", 6, 0)];
    let changes = changes_of(&files);
    let rows = changes.rows(None, 3);
    assert_eq!(rows.len(), 3, "the budget is respected");
    assert_eq!(shape(&rows), vec!["dir src".to_string(), "file a.rs".into(), "file b.rs".into()]);
}

/// A truncation that keeps a heading whose files were all cut promises rows that are not there.
#[test]
fn a_dangling_directory_heading_is_never_the_last_row() {
    let changes = changes_of(&[("src/one.rs", 9, 0), ("src/two.rs", 8, 0), ("docs/x.md", 1, 0)]);
    let rows = changes.rows(None, 3);
    assert!(
        !matches!(rows.last(), Some(DockChangeRow::Directory(_))),
        "trailing heading with no files: {:?}",
        shape(&rows)
    );
}

/// Git collapses an untracked directory into one entry with a trailing slash. It is a leaf,
/// not a heading over a child with no name.
#[test]
fn an_untracked_directory_is_one_row_named_for_itself() {
    let changes = DockChanges {
        files: vec![
            DockChangeItem {
                path: "docs/".into(),
                status: FileChangeStatus::Untracked,
                additions: None,
                deletions: None,
            },
            DockChangeItem {
                path: "src/nested/".into(),
                status: FileChangeStatus::Untracked,
                additions: None,
                deletions: None,
            },
        ],
        total_files: 2,
        additions: None,
        deletions: None,
        remaining: None,
    };
    assert_eq!(
        shape(&changes.rows(None, 20)),
        vec!["dir .".to_string(), "file docs/".into(), "dir src".into(), "file nested/".into(),]
    );
}

/// Files already covered by the last review recede rather than disappearing.
#[test]
fn reviewed_files_are_marked_not_hidden() {
    use luxe_core::guided_review::SeenReview;
    let changes = changes_of(&[("src/a.rs", 9, 0), ("src/b.rs", 8, 0)]);
    let mut seen = SeenReview::default();
    seen.files.insert("src/a.rs".into(), (9, 0));
    let rows = changes.rows(Some(&seen), 20);
    let marked: Vec<(String, bool)> = rows
        .iter()
        .filter_map(|row| match row {
            DockChangeRow::File { path, reviewed, .. } => Some((path.clone(), *reviewed)),
            DockChangeRow::Directory(_) => None,
        })
        .collect();
    assert_eq!(marked.len(), 2, "both files still listed");
    assert!(marked.iter().any(|(path, reviewed)| path == "src/a.rs" && *reviewed));
    assert!(marked.iter().any(|(path, reviewed)| path == "src/b.rs" && !*reviewed));
}

#[test]
fn complete_model_is_empty_for_a_fresh_snapshot() {
    let model = DockSectionModel::from_snapshot(&snapshot());

    assert_eq!(model.identity.project_name, "luxe");
    assert_eq!(model.plan, None);
    assert_eq!(model.changes, None);
    assert_eq!(model.external, None);
}

/// A session that never left the project must not grow a section telling it so — an empty
/// "nothing elsewhere" heading is a row spent saying nothing.
#[test]
fn staying_inside_the_project_shows_no_elsewhere_section() {
    assert_eq!(DockExternal::from_snapshot(&snapshot()), None);
}

/// The two kinds of place are not interchangeable: one is backed by Git and one by the record
/// of what luxe itself touched, and the branch is what tells them apart on screen.
#[test]
fn external_groups_keep_the_witness_that_backs_them() {
    let mut snapshot = snapshot();
    snapshot.external = luxe_core::ExternalChangesSummary {
        roots: vec![
            luxe_core::ExternalRootSummary {
                root: "/work/sdk".into(),
                display_path: "~/work/sdk".into(),
                git_branch: Some("main".into()),
                git: true,
                files: vec![FileChangeSummary {
                    path: "src/client.ts".into(),
                    status: FileChangeStatus::Modified,
                    additions: Some(4),
                    deletions: Some(1),
                    binary: false,
                }],
                total_files: 3,
                truncated: true,
            },
            luxe_core::ExternalRootSummary {
                root: "/home/dev/.config/luxe".into(),
                display_path: "~/.config/luxe".into(),
                git_branch: None,
                git: false,
                files: vec![FileChangeSummary {
                    path: "models.json".into(),
                    status: FileChangeStatus::Modified,
                    additions: None,
                    deletions: None,
                    binary: false,
                }],
                total_files: 1,
                truncated: false,
            },
        ],
        total_files: 4,
        additions: None,
        deletions: None,
        truncated: false,
    };

    let external = DockExternal::from_snapshot(&snapshot).expect("work was done elsewhere");
    assert_eq!(external.total_files, 4);
    let checkout = &external.groups[0];
    assert!(checkout.git);
    assert_eq!(checkout.branch.as_deref(), Some("main"));
    assert_eq!(checkout.remaining, Some(2), "the rows it could not show are counted");
    let loose = &external.groups[1];
    assert!(!loose.git);
    assert_eq!(loose.branch, None, "no repository, so no branch to name");
    assert_eq!(loose.remaining, None);
}
