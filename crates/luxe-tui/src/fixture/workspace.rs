//! Fixture workspace — the snapshot the dock reads, its files in and outside the project, and the
//! session's usage.

use crate::app::App;
use luxe_core::guided_review::{
    FileId, HunkId, LineRange, NoiseKind, ReviewFile, ReviewFileStatus, ReviewHunk, ReviewLine,
    ReviewLineKind,
};
use luxe_core::{
    FileChangeStatus, FileChangeSummary, ProjectIdentity, ReviewSummary, TodoSummary,
    TodoSummaryItem, TodoSummaryStatus, WorkspaceChangesSummary, WorkspaceScopeId,
    WorkspaceSnapshot,
};
use std::path::Path;

/// One file's change for the Changes pane: two hunks, a replaced line, an addition, a deletion.
///
/// Built through the review's own content types, which is the point of the pane sharing them —
/// a fixture that assembled its own shape would prove nothing about what a real capture renders as.
pub(super) fn changes_fixture_file(path: &str) -> ReviewFile {
    let source = |old: Option<u32>, new: Option<u32>, kind: ReviewLineKind, text: &str| {
        ReviewLine { old_line: old, new_line: new, kind, text: text.into() }
    };
    let id = FileId::from_path_lineage(Some(Path::new(path)), Some(Path::new(path)));
    let hunk =
        |label: &str, old: LineRange, new: LineRange, heading: &str, lines: Vec<ReviewLine>| {
            ReviewHunk {
                id: HunkId::from_content(&id, label, label),
                old_range: old,
                new_range: new,
                heading: Some(heading.into()),
                symbol: None,
                lines,
                noise: NoiseKind::Semantic,
            }
        };
    ReviewFile {
        id: id.clone(),
        old_path: Some(path.into()),
        new_path: Some(path.into()),
        status: ReviewFileStatus::Modified,
        language: Some("rs".into()),
        additions: Some(48),
        deletions: Some(9),
        hunks: vec![
            hunk(
                "changes-a",
                LineRange::new(140, 6),
                LineRange::new(140, 8),
                "fn change_lines",
                vec![
                    source(Some(140), Some(140), ReviewLineKind::Context, "    let seen = app.review.seen.as_ref();"),
                    source(Some(141), None, ReviewLineKind::Deletion, "    let rows = changes.rows(seen, row_cap);"),
                    source(None, Some(141), ReviewLineKind::Addition, "    let all = changes.rows(seen, usize::MAX);"),
                    source(None, Some(142), ReviewLineKind::Addition, "    let offset = app.dock.changes_scroll.min(all.len());"),
                    source(Some(142), Some(143), ReviewLineKind::Context, "    let shared = shared_row_prefix(&all);"),
                    source(Some(143), Some(144), ReviewLineKind::Context, "    if !shared.is_empty() {"),
                    source(Some(144), Some(145), ReviewLineKind::Context, "        lines.push(muted_line(format!(\"under {shared}\"), app));"),
                    source(Some(145), Some(146), ReviewLineKind::Context, "    }"),
                ],
            ),
            hunk(
                "changes-b",
                LineRange::new(212, 5),
                LineRange::new(214, 6),
                "fn change_file_line",
                vec![
                    source(Some(212), Some(214), ReviewLineKind::Context, "    let styles = &app.theme_styles;"),
                    source(Some(213), None, ReviewLineKind::Deletion, "    let ink = if reviewed { styles.dim } else { styles.text };"),
                    source(None, Some(215), ReviewLineKind::Addition, "    let open = app.changes.as_ref().is_some_and(|view| view.path == path);"),
                    source(None, Some(216), ReviewLineKind::Addition, "    let ink = if open { styles.selected_bg } else { styles.text };"),
                    source(Some(214), Some(217), ReviewLineKind::Context, "    Line::from(spans)"),
                    source(Some(215), Some(218), ReviewLineKind::Context, "}"),
                ],
            ),
        ],
        binary: false,
        generated: false,
        non_text: None,
    }
}

/// One file this session changed outside the project, as a peek at it renders.
///
/// A configuration file in no repository, so its change is measured against the copy retained
/// before the first write rather than against any `HEAD` — which is what a peek at one of these
/// actually shows.
pub(super) fn external_fixture_file(path: &str) -> ReviewFile {
    let source = |old: Option<u32>, new: Option<u32>, kind: ReviewLineKind, text: &str| {
        ReviewLine { old_line: old, new_line: new, kind, text: text.into() }
    };
    let name = Path::new(path).file_name().map(Path::new).unwrap_or(Path::new(path));
    let id = FileId::from_path_lineage(Some(name), Some(name));
    ReviewFile {
        id: id.clone(),
        old_path: Some(name.into()),
        new_path: Some(name.into()),
        status: ReviewFileStatus::Modified,
        language: Some("json".into()),
        additions: Some(4),
        deletions: Some(1),
        hunks: vec![ReviewHunk {
            id: HunkId::from_content(&id, "external-a", "external-a"),
            old_range: LineRange::new(12, 5),
            new_range: LineRange::new(12, 8),
            heading: None,
            symbol: None,
            lines: vec![
                source(Some(12), Some(12), ReviewLineKind::Context, "  \"models\": {"),
                source(
                    Some(13),
                    None,
                    ReviewLineKind::Deletion,
                    "    \"default\": \"qwen3.8-14b\",",
                ),
                source(
                    None,
                    Some(13),
                    ReviewLineKind::Addition,
                    "    \"default\": \"qwen3.8-27b\",",
                ),
                source(
                    None,
                    Some(14),
                    ReviewLineKind::Addition,
                    "    \"summarizer\": \"qwen3.8-4b\",",
                ),
                source(Some(14), Some(15), ReviewLineKind::Context, "    \"thinking\": \"medium\""),
                source(Some(15), Some(16), ReviewLineKind::Context, "  }"),
            ],
            noise: NoiseKind::Semantic,
        }],
        binary: false,
        generated: false,
        non_text: None,
    }
}

/// A session that has spent on the turn *and* on a review, so the dock's per-source breakdown has
/// something to show.
pub(super) fn dock_session_usage() -> luxe_core::usage::SessionUsage {
    use luxe_core::usage::{SessionUsage, SourceUsage, UsageSource, UsageTotals};
    let agent = UsageTotals {
        calls: 9,
        prompt_tokens: 12_400,
        completion_tokens: 3_800,
        cache_read_tokens: 21_000,
        cost_usd: 0.42,
        ..Default::default()
    };
    let review = UsageTotals {
        calls: 2,
        prompt_tokens: 9_100,
        completion_tokens: 1_400,
        cost_usd: 0.031,
        ..Default::default()
    };
    let mut total = agent;
    total.merge(&review);
    SessionUsage {
        total,
        by_source: vec![
            SourceUsage { source: UsageSource::Agent, totals: agent },
            SourceUsage { source: UsageSource::Review, totals: review },
        ],
    }
}

pub(crate) fn dock_workspace_snapshot() -> WorkspaceSnapshot {
    let mut snapshot = WorkspaceSnapshot::empty(
        WorkspaceScopeId::new("fixture-dock"),
        ProjectIdentity {
            root: "/work/luxe".into(),
            display_name: "luxe".into(),
            display_path: "~/work/luxe".into(),
            git_branch: Some("feature/context-dock".into()),
            git_dirty: true,
            git_head: None,
        },
    );
    // In the order they were written, which is the order the rail draws them: the finished ones
    // stay where they were rather than sinking to the bottom, so the checklist reads top to bottom
    // and the ticks descend through it. Longer than the rail's budget on purpose, so the capture
    // shows the checklist's own scrollbar parked around the work in hand rather than at the top.
    // The counts add up — 6 pending + 1 active + 4 done + 1 blocked — with the two rows past the
    // snapshot's bound all still pending.
    snapshot.todos = TodoSummary {
        current: Some(TodoSummaryItem {
            id: "30".into(),
            subject: "Render the Context Plan".into(),
            status: TodoSummaryStatus::InProgress,
            blocked_by: Vec::new(),
        }),
        items: vec![
            TodoSummaryItem {
                id: "26".into(),
                subject: "Read the workspace snapshot".into(),
                status: TodoSummaryStatus::Completed,
                blocked_by: Vec::new(),
            },
            TodoSummaryItem {
                id: "27".into(),
                subject: "Name the sections".into(),
                status: TodoSummaryStatus::Completed,
                blocked_by: Vec::new(),
            },
            TodoSummaryItem {
                id: "28".into(),
                subject: "Model the snapshot".into(),
                status: TodoSummaryStatus::Completed,
                blocked_by: Vec::new(),
            },
            TodoSummaryItem {
                id: "29".into(),
                subject: "Reduce it for the rail".into(),
                status: TodoSummaryStatus::Completed,
                blocked_by: Vec::new(),
            },
            TodoSummaryItem {
                id: "30".into(),
                subject: "Render the Context Plan".into(),
                status: TodoSummaryStatus::InProgress,
                blocked_by: Vec::new(),
            },
            TodoSummaryItem {
                id: "31".into(),
                subject: "Capture responsive fixtures".into(),
                status: TodoSummaryStatus::Blocked,
                blocked_by: vec!["30".into()],
            },
            TodoSummaryItem {
                id: "32".into(),
                subject: "Wire the dock hit regions".into(),
                status: TodoSummaryStatus::Pending,
                blocked_by: Vec::new(),
            },
            TodoSummaryItem {
                id: "33".into(),
                subject: "Document the sections".into(),
                status: TodoSummaryStatus::Pending,
                blocked_by: Vec::new(),
            },
            TodoSummaryItem {
                id: "34".into(),
                subject: "Capture the responsive docks".into(),
                status: TodoSummaryStatus::Pending,
                blocked_by: Vec::new(),
            },
            TodoSummaryItem {
                id: "35".into(),
                subject: "Check it under a bare font".into(),
                status: TodoSummaryStatus::Pending,
                blocked_by: Vec::new(),
            },
        ],
        pending: 6,
        in_progress: 1,
        completed: 4,
        blocked: 1,
        total: 12,
        truncated: true,
    };
    let change = |path: &str, status, additions, deletions, binary| FileChangeSummary {
        path: path.into(),
        status,
        additions,
        deletions,
        binary,
    };
    snapshot.changes = WorkspaceChangesSummary {
        files: vec![
            // A root-level file: common in practice, and the only way a screenshot shows the
            // `/ (root)` heading.
            change("PLAN.md", FileChangeStatus::Modified, Some(64), Some(18), false),
            change(
                "crates/luxe-tui/src/view/dock.rs",
                FileChangeStatus::Modified,
                Some(48),
                Some(9),
                false,
            ),
            change(
                "crates/luxe-tui/src/statusline.rs",
                FileChangeStatus::Modified,
                Some(21),
                Some(4),
                false,
            ),
            change(
                "crates/luxe-tui/src/dashboard.rs",
                FileChangeStatus::Modified,
                Some(6),
                Some(2),
                false,
            ),
            change(
                "crates/luxe-core/src/workspace.rs",
                FileChangeStatus::Modified,
                Some(13),
                Some(0),
                false,
            ),
            change(
                "crates/luxe-tui/src/fixture.rs",
                FileChangeStatus::Added,
                Some(31),
                Some(0),
                false,
            ),
            change("assets/context-preview.png", FileChangeStatus::Added, None, None, true),
            change(
                "docs/architecture/context-dock.md",
                FileChangeStatus::Renamed,
                Some(2),
                Some(2),
                false,
            ),
            change(".luxe/agent-loop/state.json", FileChangeStatus::Untracked, None, None, false),
        ],
        total_files: 12,
        additions: None,
        deletions: None,
        truncated: true,
    };
    // Both kinds of place, because they promise different things: a checkout Git reports in full,
    // and loose files measured only against what the session found. Three of them, so the section
    // outgrows its budget and the capture shows the scrollbar that makes the rest reachable.
    snapshot.external = luxe_core::ExternalChangesSummary {
        roots: vec![
            luxe_core::ExternalRootSummary {
                root: "/home/dev/work/luxe-sdk".into(),
                display_path: "~/work/luxe-sdk".into(),
                git_branch: Some("main".into()),
                git: true,
                files: vec![
                    change("src/client.ts", FileChangeStatus::Modified, Some(18), Some(3), false),
                    change("src/events.ts", FileChangeStatus::Added, Some(64), Some(0), false),
                ],
                total_files: 2,
                truncated: false,
            },
            luxe_core::ExternalRootSummary {
                root: "/home/dev/work/luxe-docs".into(),
                display_path: "~/work/luxe-docs".into(),
                git_branch: Some("guide".into()),
                git: true,
                files: vec![
                    change(
                        "guide/interface.md",
                        FileChangeStatus::Modified,
                        Some(31),
                        Some(6),
                        false,
                    ),
                    change("README.md", FileChangeStatus::Modified, Some(2), Some(2), false),
                ],
                total_files: 2,
                truncated: false,
            },
            luxe_core::ExternalRootSummary {
                root: "/home/dev/.config/luxe".into(),
                display_path: "~/.config/luxe".into(),
                git_branch: None,
                git: false,
                files: vec![change(
                    "models.json",
                    FileChangeStatus::Modified,
                    Some(4),
                    Some(1),
                    false,
                )],
                total_files: 1,
                truncated: false,
            },
        ],
        total_files: 5,
        additions: Some(119),
        deletions: Some(12),
        truncated: false,
    };
    snapshot.review = Some(ReviewSummary {
        id: "fixture-review".into(),
        title: "Fix the stale reference".into(),
        source: "working tree".into(),
        unresolved_findings: 1,
        completed_at_ms: 2,
    });
    snapshot
}

pub fn seed_plan_usage(app: &mut App) {
    app.status.plan_usage = Some(luxe_core::provider::PlanUsage {
        plan: Some("plus".into()),
        primary: Some(luxe_core::provider::PlanWindow {
            used_percent: 12.0,
            window_minutes: 10_080,
            resets_in_secs: Some(603_553),
        }),
        secondary: None,
        credits_balance: None,
    });
}
