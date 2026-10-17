//! Fixtures the detail's tests share.

use luxe_core::github::overview::{
    ConversationEntry, MergeMethod, MergeStatus, PullRequestOverview, Reviewer, ReviewerKind,
    Verdict, Viewer,
};
use luxe_core::github::{
    Author, CheckRollup, ConversationComment, PullRequestState, ReviewDecision,
};
use ratatui::text::Line;

use crate::theme::styles::ThemeStyles;

pub(super) fn styles() -> ThemeStyles {
    let registry = luxe_core::theme::ThemeRegistry::new();
    let resolved = registry.resolve("luxe").expect("luxe resolves");
    ThemeStyles::from_resolved(&resolved, "luxe")
}

/// Your own pull request: one check failing, one reviewer asking for changes, two threads open.
pub(super) fn pr() -> PullRequestOverview {
    PullRequestOverview {
        node_id: String::new(),
        number: 142,
        title: "Route digests by category".into(),
        body: "Routes each digest by its category.".into(),
        url: "https://github.com/acme/notify/pull/142".into(),
        repository: "acme/notify".into(),
        author: Author { login: "nora".into(), bot: false },
        state: PullRequestState::Open,
        draft: false,
        base_ref: "main".into(),
        head_ref: "digest-routing".into(),
        head_oid: "4c1e2a0f9b".into(),
        cross_repository: false,
        additions: 50,
        deletions: 12,
        changed_files: 6,
        commits: 6,
        updated_at: 1_000,
        labels: Vec::new(),
        merge_status: MergeStatus::Blocked,
        review_decision: Some(ReviewDecision::ChangesRequested),
        reviewers: vec![Reviewer {
            login: "jon".into(),
            kind: ReviewerKind::User,
            verdict: Verdict::ChangesRequested,
            at: Some(1_000),
        }],
        checks: CheckRollup { passed: 11, failed: 1, ..CheckRollup::default() },
        failing: Vec::new(),
        unresolved_threads: 2,
        stack: None,
        viewer: Viewer { author: true, can_push: true, can_merge: true },
        merge_methods: vec![MergeMethod::Squash],
        merge_settings: Default::default(),
        conversation_count: 3,
    }
}

pub(super) fn comment(author: &str, bot: bool, body: &str) -> ConversationEntry {
    ConversationEntry::Comment(ConversationComment {
        id: author.into(),
        author: author.into(),
        bot,
        mine: false,
        body: body.into(),
        created_at: "1970-01-01T00:16:40Z".into(),
        url: String::new(),
    })
}

/// A person's short comment, then a bot's long one.
pub(super) fn conversation() -> Vec<ConversationEntry> {
    let long: String = (1..=20).map(|n| format!("Line {n}.\n\n")).collect();
    vec![comment("jon", false, "Short."), comment("coderabbitai[bot]", true, &long)]
}

pub(super) fn text(lines: &[Line]) -> Vec<String> {
    lines.iter().map(|line| line.to_string().trim_end().to_owned()).collect()
}
