use super::*;
use luxe_core::github::CheckRollup;

fn pr() -> PullRequestStatus {
    PullRequestStatus {
        number: 138,
        title: "Groundwork".into(),
        url: "https://github.com/acme/app/pull/138".into(),
        state: PullRequestState::Open,
        draft: false,
        review_decision: None,
        checks: CheckRollup::default(),
        unresolved_threads: 0,
        head_oid: "abc".into(),
        stack: None,
    }
}

#[test]
fn the_label_says_what_is_waiting_in_words() {
    assert_eq!(label(&pr()), "#138");
    let checks = |passed, failed, pending| CheckRollup { passed, failed, pending, skipped: 0 };
    assert_eq!(label(&PullRequestStatus { checks: checks(5, 0, 0), ..pr() }), "#138 · checks pass");
    assert_eq!(
        label(&PullRequestStatus { checks: checks(4, 0, 1), ..pr() }),
        "#138 · checks running"
    );
    assert_eq!(
        label(&PullRequestStatus {
            checks: checks(3, 2, 1),
            draft: true,
            review_decision: Some(ReviewDecision::ChangesRequested),
            unresolved_threads: 2,
            ..pr()
        }),
        "#138 · draft · 2 failing · changes requested · 2 threads"
    );
    assert_eq!(
        label(&PullRequestStatus {
            review_decision: Some(ReviewDecision::Approved),
            unresolved_threads: 1,
            ..pr()
        }),
        "#138 · approved · 1 thread"
    );
    assert_eq!(
        label(&PullRequestStatus { review_decision: Some(ReviewDecision::ReviewRequired), ..pr() }),
        "#138"
    );
}

/// A finished pull request has nothing left to act on, so its checks and threads are not news.
#[test]
fn a_finished_pull_request_says_only_that() {
    let failing = CheckRollup { failed: 1, ..CheckRollup::default() };
    let merged = PullRequestStatus { state: PullRequestState::Merged, checks: failing, ..pr() };
    assert_eq!(label(&merged), "#138 · merged");
    assert!(!wants_attention(&merged));
    let closed =
        PullRequestStatus { state: PullRequestState::Closed, unresolved_threads: 3, ..pr() };
    assert_eq!(label(&closed), "#138 · closed");
}

#[test]
fn attention_is_a_failing_check_or_a_request_for_changes() {
    assert!(!wants_attention(&pr()));
    let failing = CheckRollup { failed: 1, ..CheckRollup::default() };
    assert!(wants_attention(&PullRequestStatus { checks: failing, ..pr() }));
    assert!(wants_attention(&PullRequestStatus {
        review_decision: Some(ReviewDecision::ChangesRequested),
        ..pr()
    }));
    assert!(!wants_attention(&PullRequestStatus {
        review_decision: Some(ReviewDecision::Approved),
        ..pr()
    }));
}

#[test]
fn the_report_says_everything_or_why_there_is_nothing() {
    let found = PullRequestPresence::Found(PullRequestStatus {
        checks: CheckRollup { passed: 4, failed: 1, pending: 2, skipped: 1 },
        review_decision: Some(ReviewDecision::ChangesRequested),
        unresolved_threads: 120,
        draft: true,
        ..pr()
    });
    assert_eq!(
        report(&found),
        "#138 Groundwork — open, draft\n\
             checks: 1 failing, 2 running, 4 passed, 1 skipped\n\
             review: changes requested · 100+ threads unresolved\n\
             https://github.com/acme/app/pull/138"
    );
    let quiet = PullRequestPresence::Found(pr());
    assert!(report(&quiet).contains("checks: none reported"), "{}", report(&quiet));
    assert!(report(&quiet).contains("review: no review required"));
    let merged = PullRequestPresence::Found(PullRequestStatus {
        state: PullRequestState::Merged,
        review_decision: Some(ReviewDecision::Approved),
        ..pr()
    });
    assert!(report(&merged).starts_with("#138 Groundwork — merged"));
    assert!(report(&merged).contains("review: approved"));
    let closed = PullRequestPresence::Found(PullRequestStatus {
        state: PullRequestState::Closed,
        review_decision: Some(ReviewDecision::ReviewRequired),
        ..pr()
    });
    assert!(report(&closed).starts_with("#138 Groundwork — closed"));
    assert!(report(&closed).contains("review: review required"));

    assert_eq!(report(&PullRequestPresence::None), "This branch has no pull request on GitHub.");
    assert!(report(&PullRequestPresence::Unknown).contains("asking now"));
    let reason = "The GitHub CLI is not signed in.".to_string();
    assert_eq!(report(&PullRequestPresence::Unavailable { reason: reason.clone() }), reason);
}

fn stack(trunk_is_default: bool, bottom_checked: bool) -> luxe_core::github::Stack {
    let entry =
        |number: u64, title: &str, draft: bool, checked: bool| luxe_core::github::StackEntry {
            number,
            title: title.into(),
            head_ref: format!("b{number}"),
            draft,
            checked,
        };
    luxe_core::github::Stack {
        trunk: "master".into(),
        trunk_is_default,
        below: vec![entry(136, "Bottom", false, bottom_checked), entry(137, "Middle", true, false)],
        above: vec![entry(139, "Top", false, false)],
    }
}

/// The bar says where it sits; `/pr` lists the chain bottom first with this one marked, and
/// says why no checks ran when the bottom's did.
#[test]
fn a_stacked_pull_request_says_where_it_sits_and_why_checks_wait() {
    let stacked = PullRequestStatus { stack: Some(stack(true, true)), draft: true, ..pr() };
    assert_eq!(label(&stacked), "#138 · stacked 3 of 4 · draft");
    let merged = PullRequestStatus { state: PullRequestState::Merged, ..stacked.clone() };
    assert_eq!(label(&merged), "#138 · merged", "a finished one sits nowhere");

    let text = report(&PullRequestPresence::Found(stacked.clone()));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[1], "stack: 3 of 4, into `master`");
    assert_eq!(
        &lines[2..6],
        ["  #136 Bottom", "  #137 Middle (draft)", "› #138 Groundwork — this one", "  #139 Top"]
    );
    assert!(
        lines[6].starts_with(
            "checks: none yet — this repository checks pull requests into `master`, as #136's were"
        ),
        "{text}"
    );

    // No evidence, no claim: the bottom's had none either, or the stack ends off trunk
    for (default, checked) in [(true, false), (false, true)] {
        let quiet = PullRequestStatus { stack: Some(stack(default, checked)), ..pr() };
        let text = report(&PullRequestPresence::Found(quiet));
        assert!(text.contains("checks: none reported") && !text.contains("none yet"), "{text}");
    }
    let off_trunk = PullRequestStatus { stack: Some(stack(false, true)), ..pr() };
    let text = report(&PullRequestPresence::Found(off_trunk));
    assert!(text.contains("into `master`, a branch with no pull request of its own"), "{text}");
    // Only the counts there are: no "0 failing" beside what passed
    let passing =
        PullRequestStatus { checks: CheckRollup { passed: 3, ..CheckRollup::default() }, ..pr() };
    let text = report(&PullRequestPresence::Found(passing));
    assert!(text.lines().any(|line| line == "checks: 3 passed"), "{text}");
}
