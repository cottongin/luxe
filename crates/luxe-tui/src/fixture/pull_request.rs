//! The pull request review — someone else's change, with its context and the threads on it.

use super::demo::fixture_demo_guided_review;
use luxe_core::guided_review::{GuidedReview, ReviewTarget};

/// The review the demos walk. See [`DEMO_CHANGE`] for why it is not the small one.
/// The demos' change as a pull request: opened by a bot, one check failing, changes requested, and
/// an issue it closes. No session provenance, because a pull request was not this session's work.
pub fn fixture_pull_request_review() -> GuidedReview {
    use luxe_core::guided_review::{
        LinkedIssue, PullRequestChecks, PullRequestContext, PullRequestStatus, PullRequestVerdict,
    };
    let mut review = fixture_demo_guided_review();
    review.target = ReviewTarget::PullRequest { number: 42, repository: None };
    review.source.base = luxe_core::guided_review::ReviewBase::PullRequest {
        number: 42,
        repository: "acme/notify".into(),
        base_ref: "main".into(),
        head_ref: "copilot/category-enum".into(),
        merge_base: "4f1c2d9a7e6b5c4d3e2f1a0b9c8d7e6f5a4b3c2d".into(),
        head_oid: "a9d9d84ee3e848f0e1ea97eaef837126d3a7d983".into(),
    };
    review.provenance = None;
    review.context.goal = None;
    review.context.title = "Parse notification categories into an enum".into();
    review.context.pull_request = Some(PullRequestContext {
        number: 42,
        repository: "acme/notify".into(),
        url: "https://github.com/acme/notify/pull/42".into(),
        title: "Parse notification categories into an enum".into(),
        body: "Categories arrived as free-form strings, so a typo in a webhook became a new \
               category nobody routed. This parses them once at the edge and makes the router \
               match exhaustively.\n\n- `Category` replaces `String` in the model\n- unknown \
               categories are rejected with a 422"
            .into(),
        author: "app/copilot-swe-agent".into(),
        author_is_bot: true,
        draft: false,
        status: PullRequestStatus::Open,
        verdict: Some(PullRequestVerdict::ChangesRequested),
        checks: PullRequestChecks { passed: 11, failed: 1, pending: 0, skipped: 2 },
        base_ref: "main".into(),
        head_ref: "copilot/category-enum".into(),
        closing_issues: vec![LinkedIssue {
            repository: "acme/notify".into(),
            number: 318,
            title: Some("A typo in a webhook creates a phantom category".into()),
            excerpt: String::new(),
        }],
        more_closing_issues: 0,
    });
    // Through the same placement the engine uses, so the fixture draws what a real one would
    let comment = |author: &str, body: &str| luxe_core::github::ThreadComment {
        author: author.into(),
        body: body.into(),
        created_at: String::new(),
        url: String::new(),
    };
    let thread = |path: &str, line, outdated, resolved, file_level, comments: Vec<_>| {
        luxe_core::github::ReviewThread {
            path: path.into(),
            line,
            start_line: None,
            side: luxe_core::github::ThreadSide::Right,
            outdated,
            resolved,
            file_level,
            id: format!("thread-{path}-{line:?}"),
            anchor_text: None,
            can_reply: true,
            can_resolve: true,
            total_comments: comments.len(),
            comments,
        }
    };
    review.conversation = Some(luxe_core::guided_review::earlier_conversation(
        &review.files,
        luxe_core::github::ReviewThreads {
            threads: vec![
                thread(
                    "src/notification.rs",
                    Some(13),
                    false,
                    false,
                    false,
                    vec![
                        comment(
                            "mara-okafor",
                            "Does anything still construct this from a raw string?",
                        ),
                        comment("app/copilot-swe-agent", "Only the webhook, and it parses first."),
                    ],
                ),
                thread(
                    "src/dispatch/router.rs",
                    None,
                    true,
                    false,
                    false,
                    vec![comment("mara-okafor", "This match used to fall through to the digest.")],
                ),
                thread(
                    "src/api/webhook.rs",
                    None,
                    false,
                    true,
                    true,
                    vec![comment("jon-avery", "Worth a 422 test for an empty category.")],
                ),
            ],
            total: 3,
            conversation: vec![
                luxe_core::github::ConversationComment {
                    id: "IC_kwDOA1".into(),
                    author: "codecov".into(),
                    bot: true,
                    mine: false,
                    body: "Coverage 91.2% (+0.4%)".into(),
                    created_at: "2026-05-02T09:00:00Z".into(),
                    url: "https://github.com/acme/notify/pull/42#issuecomment-1".into(),
                },
                luxe_core::github::ConversationComment {
                    id: "IC_kwDOA2".into(),
                    author: "jon-avery".into(),
                    bot: false,
                    mine: false,
                    body: "Could the changelog mention the new 422?".into(),
                    created_at: "2026-05-02T10:00:00Z".into(),
                    url: "https://github.com/acme/notify/pull/42#issuecomment-2".into(),
                },
            ],
        },
    ));
    review
}
