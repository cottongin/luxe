//! Two repositories' pull requests with one number: a detail acts on the one it shows.

use super::*;
use luxe_core::github::overview::{
    ConversationEntry, MergeStatus, PullRequestOverview, Reviewer, ReviewerKind, Verdict, Viewer,
};
use luxe_core::github::{CheckRollup, ConversationComment, ReviewDecision};

/// `acme/app#142` (the checkout's) above `upstream/app#142`, both read.
fn colliding(draft: bool) -> GithubPanel {
    let rows = vec![item(true, 142, "Ours", "acme/app"), item(true, 142, "Theirs", "upstream/app")];
    let mut panel = GithubPanel {
        repository: Some("acme/app".into()),
        sections: Some(vec![ListedSection {
            name: "Mine".into(),
            items: rows.clone(),
            error: None,
            total: None,
            next: None,
        }]),
        clock: Some(0),
        ..GithubPanel::with_searches(&searches(), searches())
    };
    for row in &rows {
        let overview = PullRequestOverview {
            number: 142,
            url: row.url.clone(),
            repository: row.repository.clone(),
            draft,
            merge_status: MergeStatus::Clean,
            review_decision: Some(ReviewDecision::Approved),
            reviewers: vec![Reviewer {
                login: "jon".into(),
                kind: ReviewerKind::User,
                verdict: Verdict::Approved,
                at: Some(0),
            }],
            checks: CheckRollup { passed: 3, ..CheckRollup::default() },
            failing: Vec::new(),
            unresolved_threads: 0,
            viewer: Viewer { author: true, can_push: true, can_merge: true },
            ..crate::fixture::pull_request_overview()
        };
        let mut detail = GithubDetail::new(Subject::PullRequest(Box::new(overview)));
        detail.conversation = Some(vec![ConversationEntry::Comment(ConversationComment {
            id: "c1".into(),
            author: "jon".into(),
            bot: false,
            mine: false,
            body: "Why this way?".into(),
            created_at: "1970-01-01T00:16:40Z".into(),
            url: String::new(),
        })]);
        panel.details.insert(row.url.clone(), detail);
    }
    panel
}

/// The panel with row `index` selected and its detail focused.
fn on_row(draft: bool, index: usize) -> GithubPanel {
    let mut panel = colliding(draft);
    panel.selected = index;
    panel.focus = Focus::Detail;
    panel
}

/// Every write and read a detail sends names the repository of the row it shows, in both
/// directions, whichever row with that number came first.
#[test]
fn detail_actions_keep_the_selected_repository_when_numbers_collide() {
    for (index, repository) in [(0, None), (1, Some("upstream/app".to_string()))] {
        let mut merging = on_row(false, index);
        assert_eq!(merging.on_key(&key(KeyCode::Enter)), GithubKey::Held, "opens the confirmation");
        match merging.on_key(&key(KeyCode::Enter)) {
            GithubKey::Merge { repository: chosen, number: 142, .. } => {
                assert_eq!(chosen, repository, "merge")
            }
            other => panic!("{other:?}"),
        }

        let mut acting = on_row(true, index);
        match acting.on_key(&key(KeyCode::Enter)) {
            GithubKey::Act { repository: chosen, .. } => assert_eq!(chosen, repository, "act"),
            other => panic!("{other:?}"),
        }

        let mut reading = on_row(false, index);
        match reading.on_key(&key(KeyCode::Tab)) {
            GithubKey::ReadConversation { repository: chosen, number: 142, .. } => {
                assert_eq!(chosen, repository, "conversation")
            }
            other => panic!("{other:?}"),
        }
        match reading.on_key(&key(KeyCode::Char('d'))) {
            GithubKey::Draft { repository: chosen, number: 142, .. } => {
                assert_eq!(chosen, repository, "draft")
            }
            other => panic!("{other:?}"),
        }

        let mut posting = on_row(false, index);
        posting.on_key(&key(KeyCode::Tab));
        posting.on_key(&key(KeyCode::Char('r')));
        typed(&mut posting, "Because.");
        posting.on_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));
        match posting.on_key(&key(KeyCode::Enter)) {
            GithubKey::Post(post) => assert_eq!(post.repository, repository, "post"),
            other => panic!("{other:?}"),
        }
    }
}

/// Enter on a merge row in `/github` shows the confirmation, with its method and branch choice,
/// before any write: at full width and in the narrow single pane. Esc backs out; Enter again sends
/// the write as shown. The same for turning on auto-merge.
#[test]
fn github_panel_draws_merge_confirmation_before_emitting_write() {
    for (width, auto) in [(180, false), (90, false), (180, true), (90, true)] {
        let mut panel = on_row(false, 0);
        if auto {
            for detail in panel.details.values_mut() {
                if let Subject::PullRequest(pr) = &mut detail.subject {
                    pr.merge_settings.can_auto_merge = true;
                    pr.merge_status = MergeStatus::Blocked;
                }
            }
        }
        let title = if auto { "Enable auto-merge" } else { "Merge pull request" };
        assert_eq!(panel.on_key(&key(KeyCode::Enter)), GithubKey::Held, "no write yet");
        let screen = drawn(&panel, width, 40).join("\n");
        assert!(screen.contains(title) && screen.contains("Method"), "{width}: {screen}");
        assert_eq!(panel.on_key(&key(KeyCode::Esc)), GithubKey::Held);
        assert!(!drawn(&panel, width, 40).join("\n").contains(title), "Esc backed out");
        panel.on_key(&key(KeyCode::Enter));
        let write = match panel.on_key(&key(KeyCode::Enter)) {
            GithubKey::Merge { write, .. } => write,
            other => panic!("{width} {auto}: {other:?}"),
        };
        use luxe_core::engine::PullRequestWrite;
        assert_eq!(matches!(write, PullRequestWrite::EnableAutoMerge { .. }), auto, "{write:?}");
        assert_eq!(matches!(write, PullRequestWrite::Merge(_)), !auto, "{write:?}");
    }
}

/// A detail that is not the pull request of the row it hangs off takes no key: nothing is sent,
/// and nothing in it changes, rather than a confirmation sent and then dropped.
#[test]
fn a_detail_that_disagrees_with_its_row_takes_no_key() {
    let mut panel = on_row(false, 0);
    let url = panel.rows()[0].url.clone();
    if let Some(Subject::PullRequest(pr)) = panel.details.get_mut(&url).map(|d| &mut d.subject) {
        pr.number = 999;
    }
    assert_eq!(panel.on_key(&key(KeyCode::Enter)), GithubKey::Held);
    assert!(panel.details[&url].merging.is_none(), "the detail did not act");
    assert_eq!(panel.focus, Focus::List);
}
