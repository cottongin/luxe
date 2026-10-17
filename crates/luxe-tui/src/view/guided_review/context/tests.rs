use super::summary::{collapsed_banner, feedback_summary, fixes_returned_label};

/// Fixes asked for from the revision on screen, once their turn has ended, say the diff is from
/// before them. A question's answer changes no code, a turn still working has not come back, and
/// a refreshed revision is one no submission names.
#[test]
fn fixes_that_came_back_say_the_diff_is_from_before_them() {
    use crate::fixture::{seeded_app, FixtureScenario};
    use luxe_core::guided_review::{
        ReviewFeedbackMode, ReviewRevisionId, SubmissionDispatch, SubmissionId,
    };
    let app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture");
    let mut session = app.review.session.clone().expect("session");
    let revision = session.source.content_digest.clone();
    let ids: Vec<_> = session.findings.iter().take(2).map(|finding| finding.id.clone()).collect();
    let files = app.guided_review.as_ref().expect("review").files.clone();
    session
        .queue_submission(SubmissionId::new(), ReviewFeedbackMode::RequestFixes, &ids, &files, 7)
        .expect("queued");
    let mut with = |dispatch, mode| {
        let submission = session.submissions.last_mut().expect("submission");
        submission.dispatch = dispatch;
        submission.mode = mode;
        fixes_returned_label(&session, &revision)
    };

    assert_eq!(with(SubmissionDispatch::Dispatched, ReviewFeedbackMode::RequestFixes), None);
    assert_eq!(with(SubmissionDispatch::Completed, ReviewFeedbackMode::Question), None);
    let label = with(SubmissionDispatch::Completed, ReviewFeedbackMode::RequestFixes);
    assert_eq!(label.as_deref(), Some("Fixes for 2 comments are in · ctrl+r refresh"));
    let refreshed = ReviewRevisionId::from_canonical_bytes(b"after the fixes");
    assert_eq!(fixes_returned_label(&session, &refreshed), None);
}

/// Once a pull request's review has gone to GitHub the foot says so, with the verdict, in the
/// place that offered to post it. The toast saying so is gone in seconds, and a foot still
/// offering to post left the reader unsure whether anything had gone at all.
#[test]
fn the_guide_foot_says_a_posted_review_went_out() {
    use crate::fixture::{seeded_app, FixtureScenario};
    use luxe_core::guided_review::{FindingState, PostedComment, ReviewOutcome};
    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewPullRequestPost).expect("fixture");
    app.guided_review_workspace.github_post = None;
    let review = app.guided_review.clone().expect("review");
    app.guided_review_workspace.walked_chapters =
        review.chapters.iter().map(|chapter| chapter.id.clone()).collect();
    let foot = |app: &crate::app::App| {
        super::foot::guide_foot_lines(app, &review, 60)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let before = foot(&app);
    assert!(before.contains("to post your review"), "{before}");

    let session = app.review.session_mut().expect("session");
    session.outcome = Some(ReviewOutcome::Accepted);
    for finding in &mut session.findings {
        finding.state = FindingState::Draft;
        finding.posted =
            Some(PostedComment { review_id: 9, url: "https://github.com/a/b/pull/1".into() });
    }
    let after = foot(&app);
    assert!(after.contains("Posted to GitHub · approved"), "{after}");
    assert!(after.contains("opens it"), "and where to read it: {after}");
    assert!(!after.contains("to post your review"), "no longer offered: {after}");
    assert!(!after.contains("Outcome:"), "the posted line says the verdict: {after}");

    // A comment written after the post is what the key offers next
    let session = app.review.session_mut().expect("session");
    session.findings[0].posted = None;
    session.findings[0].severity = luxe_core::guided_review::FindingSeverity::Suggestion;
    let more = foot(&app);
    assert!(more.contains("to post your new comment"), "{more}");
}

/// The guide foot says what the feedback wants next: unsent first, then posted, then with the
/// agent, then back with the reader to verify or to read. A posted comment is neither unsent
/// nor waiting, a question on a pull request stays with the agent rather than waiting to be
/// posted, and a comment is with the agent only while its send is queued or running.
#[test]
fn the_guide_foot_counts_what_the_feedback_wants_next() {
    use crate::fixture::{seeded_app, FixtureScenario};
    use luxe_core::guided_review::{
        FindingMessage, FindingMessageAuthor, FindingSeverity, FindingState, PostedComment,
        ReviewFeedbackMode, SubmissionDispatch, SubmissionId,
    };
    let app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture");
    // R1 open · R2 to verify · R3 draft · R4 verified · R5 stale
    let base = app.review.session.clone().expect("session");
    let summary = |edit: &dyn Fn(&mut luxe_core::guided_review::ReviewSession), pr: bool| {
        let mut session = base.clone();
        edit(&mut session);
        feedback_summary(Some(&session), pr)
    };
    let posted =
        || Some(PostedComment { review_id: 1, url: "https://github.com/a/b/pull/1".into() });
    let settle = |session: &mut luxe_core::guided_review::ReviewSession| {
        for finding in &mut session.findings {
            finding.state = FindingState::Resolved;
        }
    };

    assert_eq!(summary(&|_| {}, false), "Feedback 5 · 1 unsent");
    assert_eq!(summary(&|_| {}, true), "Feedback 5 · 1 unposted");
    assert_eq!(
        summary(&|session| session.findings[2].severity = FindingSeverity::Question, true),
        "Feedback 5 · 1 to verify",
        "a question on a pull request is not waiting to be posted"
    );
    assert_eq!(
        summary(&|session| session.findings[2].posted = posted(), false),
        "Feedback 5 · 1 on GitHub",
        "a posted draft is not unsent"
    );
    let drafted = |session: &mut luxe_core::guided_review::ReviewSession| {
        session.findings[2].state = FindingState::Resolved;
    };
    assert_eq!(summary(&drafted, false), "Feedback 5 · 1 to verify", "sent is not with the agent");
    let sending = |session: &mut luxe_core::guided_review::ReviewSession,
                   dispatch: SubmissionDispatch| {
        drafted(session);
        let ids = [session.findings[0].id.clone(), session.findings[1].id.clone()];
        let submission = SubmissionId::new();
        session
            .queue_submission(submission.clone(), ReviewFeedbackMode::RequestFixes, &ids, &[], 0)
            .expect("queued");
        session.submissions.last_mut().expect("the send").dispatch = dispatch;
    };
    for dispatch in [SubmissionDispatch::Queued, SubmissionDispatch::Dispatched] {
        assert_eq!(
            summary(&|session| sending(session, dispatch), false),
            "Feedback 5 · 2 with the agent",
            "{dispatch:?}"
        );
    }
    assert_eq!(
        summary(&|session| sending(session, SubmissionDispatch::Completed), false),
        "Feedback 5 · 1 to verify",
        "a finished send is back with the reader"
    );
    let replied = |session: &mut luxe_core::guided_review::ReviewSession| {
        drafted(session);
        session.findings[1].state = FindingState::Resolved;
        session.findings[0].thread.push(FindingMessage {
            author: FindingMessageAuthor::System,
            body: "No word from the agent on this comment: it wrote no report.".into(),
            created_at_ms: 1,
        });
    };
    assert_eq!(summary(&replied, false), "Feedback 5 · 1 to read");
    assert_eq!(
        summary(
            &|session| {
                settle(session);
                session.findings[0].state = FindingState::Open;
                session.findings[0].posted = posted();
            },
            false
        ),
        "Feedback 5 · 1 on GitHub",
        "a posted comment is not with the agent"
    );
    assert_eq!(summary(&settle, true), "Feedback 5");
    assert_eq!(feedback_summary(None, false), "No feedback yet");
}

#[test]
fn collapsed_banner_pluralizes_changes_and_lines() {
    assert_eq!(collapsed_banner(1, 1), "1 mechanical change collapsed · 1 line");
    assert_eq!(collapsed_banner(3, 12), "3 mechanical changes collapsed · 12 lines");
    assert_eq!(collapsed_banner(1, 4), "1 mechanical change collapsed · 4 lines");
}
