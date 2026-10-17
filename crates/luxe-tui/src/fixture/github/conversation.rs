//! GitHub conversation — a pull request's comments and threads, and the agent's draft reply.

use super::overview::DESCRIPTION;
use super::rows::{stamp, DAY, HOUR, NOW};
use crate::github_detail::Draft;
use luxe_core::github::overview::{ConversationEntry, Verdict};
use luxe_core::github::{Author, ConversationComment, ReviewThread, ThreadComment, ThreadSide};

const RABBIT: &str = "## Walkthrough

The change introduces category-based routing for digest delivery. A new `DigestRouter` maps \
each `Category` to a send schedule, and the scheduler consults it before enqueueing.

## Changes

| File | Summary |
| --- | --- |
| `src/digest/router.rs` | New router mapping categories to schedules |
| `src/digest/scheduler.rs` | Consults the router before enqueueing |
| `src/dispatch/router.rs` | Passes the parsed category through |

## Sequence

1. Webhook parses the category.
2. Dispatcher hands it to the digest scheduler.
3. Scheduler asks the router for a schedule.
4. The digest is enqueued on that schedule.

## Estimated review effort

Medium — touches scheduling on a hot path.";

const JON_REVIEW: &str = "Two things before this goes in. The fallback for tenants without \
rules is right, but it is decided in two places; and on-call-only for incidents needs a test, \
since that is the one a mistake here would page the wrong people over.";

pub(super) const JON_THREAD: &str =
    "This decides the fallback again — the scheduler already does it. If \
the two ever disagree, a tenant without rules gets incidents on the daily send. Can this read the \
schedule the scheduler picked instead?";

fn comment(author: &str, bot: bool, body: &str, ago: u64) -> ConversationComment {
    ConversationComment {
        id: format!("{author}-{ago}"),
        author: author.into(),
        bot,
        mine: author == "nora-quinn",
        body: body.into(),
        created_at: stamp(NOW - ago),
        url: String::new(),
    }
}

fn thread_comment(author: &str, body: &str, ago: u64) -> ThreadComment {
    ThreadComment {
        author: author.into(),
        body: body.into(),
        created_at: stamp(NOW - ago),
        url: String::new(),
    }
}

pub(super) fn conversation() -> Vec<ConversationEntry> {
    vec![
        ConversationEntry::Opening {
            author: Author { login: "nora-quinn".into(), bot: false },
            body: DESCRIPTION.into(),
            at: NOW - 2 * DAY,
        },
        ConversationEntry::Review {
            comment: comment("coderabbitai[bot]", true, RABBIT, DAY + 6 * HOUR),
            verdict: Verdict::Commented,
            line_comments: 4,
        },
        ConversationEntry::Review {
            comment: comment(
                "mara-okafor",
                false,
                "Reads well. The tenant fallback is the part I'd have got wrong.",
                DAY,
            ),
            verdict: Verdict::Approved,
            line_comments: 0,
        },
        ConversationEntry::Review {
            comment: comment("jon-avery", false, JON_REVIEW, 3 * HOUR),
            verdict: Verdict::ChangesRequested,
            line_comments: 2,
        },
        ConversationEntry::Thread(thread(
            "jon-router",
            "src/dispatch/router.rs",
            Some(88),
            false,
            vec![thread_comment("jon-avery", JON_THREAD, 3 * HOUR)],
        )),
        ConversationEntry::Thread(thread(
            "jon-test",
            "src/digest/scheduler.rs",
            Some(141),
            false,
            vec![thread_comment(
                "jon-avery",
                "Worth a test that an incident digest reaches on-call and nobody else.",
                3 * HOUR,
            )],
        )),
        ConversationEntry::Thread(thread(
            "mara-webhook",
            "src/api/webhook.rs",
            Some(42),
            true,
            vec![
                thread_comment("mara-okafor", "Should an unknown category 422 here?", DAY),
                thread_comment("nora-quinn", "It does now — see the second commit.", 20 * HOUR),
            ],
        )),
    ]
}

pub(super) fn thread(
    id: &str,
    path: &str,
    line: Option<u32>,
    resolved: bool,
    comments: Vec<ThreadComment>,
) -> ReviewThread {
    ReviewThread {
        id: id.into(),
        path: path.into(),
        line,
        start_line: None,
        side: ThreadSide::Right,
        outdated: false,
        resolved,
        file_level: false,
        total_comments: comments.len(),
        comments,
        anchor_text: None,
        can_reply: true,
        can_resolve: true,
    }
}

pub(super) fn agent_draft() -> Draft {
    Draft {
        target: "Reply to @jon-avery on `src/dispatch/router.rs:88`".into(),
        thread: Some("jon-router".into()),
        body: "Good catch — they could drift, and the failure mode is the one you describe: a \
tenant without routing rules would get incidents on the daily send if the router and the \
scheduler ever disagreed about the fallback.

The router no longer decides it. It reads the schedule the scheduler already picked \
(`Schedule::for_tenant`), so there is one place that knows a tenant without rules gets the daily \
send, and the router only maps categories onto whatever that returns.

What changed:

- `DigestRouter::route` takes the tenant's `Schedule` instead of building its own default
- the duplicate fallback in `src/dispatch/router.rs` is gone
- a new test, `tenant_without_rules_still_pages_on_call`, sends an incident for a tenant with no \
rules and checks it goes out at once, to on-call only

Fixed in `4c1e2a0`. The other thread — the on-call test you asked for on the scheduler — is \
covered by the same test; I'll point to it there too."
            .into(),
        by_agent: true,
        sources: vec![
            "this session".into(),
            "the thread".into(),
            "the review comment, verified".into(),
            "4c1e2a0".into(),
        ],
        note: None,
    }
}
