use super::*;
use crate::guided_review::ReviewBase;
use std::path::Path;

mod disk;

fn source() -> ReviewSource {
    let revision_id = ReviewRevisionId::from_canonical_bytes(b"diff --git a/a b/a\n");
    ReviewSource {
        base: ReviewBase::EmptyTree,
        content_digest: revision_id,
        includes_staged: false,
        includes_unstaged: true,
        included_untracked_files: 0,
        captured_at_ms: 42,
        uncovered_paths: Vec::new(),
    }
}

fn finding(anchor: FindingAnchor) -> ReviewFinding {
    ReviewFinding {
        posted: None,
        github_thread: None,
        id: FindingId::new(),
        label: String::new(),
        anchor,
        severity: FindingSeverity::Issue,
        title: "Preserve ordering".into(),
        body: "This branch changes the observable notification order.".into(),
        proposed_fix: Some("Keep the existing ordering before mapping values.".into()),
        state: FindingState::Draft,
        thread: vec![FindingMessage {
            author: FindingMessageAuthor::Reviewer,
            body: "Please keep this behavior stable.".into(),
            created_at_ms: 42,
        }],
    }
}

#[test]
fn session_round_trips_with_every_anchor_kind() {
    let file_id = FileId::from_path_lineage(None, Some(Path::new("src/lib.rs")));
    let hunk_id = HunkId::from_content(&file_id, "fn run", "+    run();");
    let chapter_id = ChapterId::from_items("core", std::slice::from_ref(&hunk_id));
    let mut session = ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source(), 42);
    session.findings = vec![
        finding(FindingAnchor::Line {
            file_id: file_id.clone(),
            hunk_id: hunk_id.clone(),
            side: DiffSide::New,
            line: 12,
            context_hash: "sha256:abc".into(),
        }),
        finding(FindingAnchor::Hunk { file_id, hunk_id }),
        finding(FindingAnchor::Chapter { chapter_id }),
        finding(FindingAnchor::Review),
    ];
    let encoded = serde_json::to_vec(&session).unwrap_or_default();
    let decoded: Result<ReviewSession, _> = serde_json::from_slice(&encoded);

    assert_eq!(decoded.ok(), Some(session.clone()));
    assert_eq!(session.validate(), Ok(()));
}

#[test]
fn session_records_explicit_local_review_outcomes() {
    let mut session = ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source(), 42);

    session.set_outcome(ReviewOutcome::Accepted);
    assert_eq!(session.outcome, Some(ReviewOutcome::Accepted));
    session.set_outcome(ReviewOutcome::Commented);
    assert_eq!(session.outcome, Some(ReviewOutcome::Commented));
}

fn from_thread(id: &str) -> ReviewFinding {
    ReviewFinding {
        github_thread: Some(GitHubThread {
            id: id.into(),
            url: String::new(),
            author: "mara".into(),
            can_reply: true,
            can_resolve: true,
            reply_url: None,
            resolved: false,
            conversation: false,
        }),
        ..finding(FindingAnchor::Review)
    }
}

/// Comments carried from reading a pull request join the review of its checkout: kept where
/// their code still is, moved with it, kept as stale when it is gone, each once, and relabeled
/// only where a label is taken.
#[test]
fn carried_comments_join_the_review_each_once_where_their_code_is() {
    let mut session = ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source(), 42);
    session.adopt_github_threads(vec![from_thread("t1")]);
    let moved_to = FindingAnchor::Hunk {
        file_id: FileId::from_path_lineage(None, Some(Path::new("src/lib.rs"))),
        hunk_id: HunkId::from_content(
            &FileId::from_path_lineage(None, Some(Path::new("src/lib.rs"))),
            "fn run",
            "+    run();",
        ),
    };
    let carried = |label: &str, title: &str| ReviewFinding {
        label: label.into(),
        title: title.into(),
        ..finding(FindingAnchor::Review)
    };
    let (kept, moved, gone) =
        (carried("R1", "kept"), carried("R2", "moved"), carried("R3", "gone"));
    let first = vec![kept.clone(), moved.clone(), gone.clone()];
    let adopted = session.adopt_carried_findings(first, |_| FindingAnchorRefresh::Exact);
    assert_eq!(adopted, 3);
    let by_title = |session: &ReviewSession, title: &str| {
        session.findings.iter().find(|f| f.title == title).cloned().unwrap()
    };
    assert_eq!(by_title(&session, "kept").label, "R2", "R1 is the thread's");
    assert_eq!(by_title(&session, "moved").label, "R3", "R2 was just given away");
    assert_eq!(by_title(&session, "gone").label, "R4");
    assert_eq!(session.adopt_carried_findings(vec![kept], |_| FindingAnchorRefresh::Stale), 0);

    let mut session = ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source(), 42);
    let mut remaps = vec![
        FindingAnchorRefresh::Exact,
        FindingAnchorRefresh::NeedsVerification(moved_to.clone()),
        FindingAnchorRefresh::Stale,
    ]
    .into_iter();
    session.adopt_carried_findings(vec![carried("R1", "kept"), moved, gone], |_| {
        remaps.next().unwrap()
    });
    let kept = by_title(&session, "kept");
    assert_eq!(
        (kept.anchor, kept.state, kept.label.as_str()),
        (FindingAnchor::Review, FindingState::Draft, "R1")
    );
    let moved = by_title(&session, "moved");
    assert_eq!((moved.anchor, moved.state), (moved_to, FindingState::Draft), "still unsent");
    assert_eq!(by_title(&session, "gone").state, FindingState::Stale, "kept, not dropped");
}

/// Loading the same pull request again adds only threads it has not seen, keeps what was done
/// to the ones it has, and passes over a finding with no thread behind it.
#[test]
fn adopting_threads_adds_each_once_and_keeps_the_work_on_it() {
    let mut session = ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source(), 42);
    assert_eq!(session.adopt_github_threads(vec![from_thread("t1"), from_thread("t2")]), 2);
    session.findings[0].state = FindingState::Resolved;
    assert_eq!(
        session.adopt_github_threads(vec![
            from_thread("t1"),
            from_thread("t3"),
            finding(FindingAnchor::Review),
        ]),
        1
    );
    assert_eq!(session.findings.len(), 3);
    assert_eq!(session.findings[0].state, FindingState::Resolved);
    let mut empty = from_thread("t4");
    empty.body = "  ".into();
    assert_eq!(session.adopt_github_threads(vec![empty]), 0, "an invalid one is passed over");
}

/// A thread's finding goes to the agent as the reviewer's words, so the snapshot keeps who
/// wrote them.
#[test]
fn a_submitted_thread_keeps_its_author() {
    let mut session = ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source(), 42);
    session.adopt_github_threads(vec![from_thread("t1")]);
    session.add_finding(finding(FindingAnchor::Review)).unwrap();
    let ids: Vec<FindingId> = session.findings.iter().map(|finding| finding.id.clone()).collect();
    session
        .queue_submission(SubmissionId::new(), ReviewFeedbackMode::RequestFixes, &ids, &[], 1)
        .unwrap();
    let submitted = &session.submissions[0].findings;
    assert_eq!(submitted[0].github_author.as_deref(), Some("mara"));
    assert_eq!(submitted[1].github_author, None);
}

#[test]
fn answering_a_thread_records_the_reply_and_the_resolve() {
    let mut session = ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source(), 42);
    session.adopt_github_threads(vec![from_thread("t1"), from_thread("t2")]);
    let (first, second) = (session.findings[0].id.clone(), session.findings[1].id.clone());

    session.mark_thread_answered(&first, Some("https://example.invalid/r1".into()), false);
    session.mark_thread_answered(&first, None, true);
    session.mark_thread_answered(&FindingId::new(), None, true);

    let answered = session.findings[0].github_thread.as_ref().unwrap();
    assert_eq!(answered.reply_url.as_deref(), Some("https://example.invalid/r1"));
    assert!(answered.resolved, "a later resolve keeps the earlier reply");
    let untouched = session.findings[1].github_thread.as_ref().unwrap();
    assert!(!untouched.resolved && untouched.reply_url.is_none());
    assert_eq!(session.findings[1].id, second);
}

#[test]
fn marking_posted_touches_only_the_comments_that_went() {
    let mut session = ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source(), 42);
    session.findings = vec![finding(FindingAnchor::Review), finding(FindingAnchor::Review)];
    let went = session.findings[0].id.clone();
    let posted = PostedComment { review_id: 7, url: "https://github.com/a/b/pull/1".into() };

    session.mark_posted(&[went, FindingId::new()], &posted);

    assert_eq!(session.findings[0].posted, Some(posted));
    assert_eq!(session.findings[1].posted, None);
}

#[test]
fn session_rejects_missing_line_context_or_duplicate_findings() {
    let file_id = FileId::from_path_lineage(None, Some(Path::new("src/lib.rs")));
    let hunk_id = HunkId::from_content(&file_id, "fn run", "+    run();");
    let mut session = ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source(), 42);
    let invalid = finding(FindingAnchor::Line {
        file_id,
        hunk_id,
        side: DiffSide::Old,
        line: 0,
        context_hash: String::new(),
    });
    assert_eq!(invalid.validate(), Err(ReviewSessionValidationError::InvalidLineAnchor));

    session.findings = vec![finding(FindingAnchor::Review)];
    session.findings.push(session.findings[0].clone());
    assert_eq!(
        session.validate(),
        Err(ReviewSessionValidationError::DuplicateId { kind: "finding" })
    );
}

#[test]
fn resolved_findings_can_be_reopened_without_losing_evidence() {
    let mut session = ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source(), 42);
    let finding = finding(FindingAnchor::Review);
    let finding_id = finding.id.clone();
    assert_eq!(session.add_finding(finding), Ok(()));

    assert_eq!(session.resolve_finding(&finding_id), Ok(()));
    assert_eq!(session.findings[0].state, FindingState::Resolved);
    assert_eq!(session.reopen_finding(&finding_id), Ok(()));
    assert_eq!(session.findings[0].state, FindingState::Open);
    assert_eq!(session.findings[0].body, "This branch changes the observable notification order.");
    assert_eq!(
        session.resolve_finding(&FindingId::new()),
        Err(ReviewSessionMutationError::FindingNotFound)
    );
}

#[test]
fn stale_finding_reanchor_requires_reviewer_confirmation() {
    let mut session = ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source(), 42);
    let mut stale = finding(FindingAnchor::Review);
    let finding_id = stale.id.clone();
    stale.state = FindingState::Stale;
    assert_eq!(session.add_finding(stale), Ok(()));

    let anchor =
        FindingAnchor::Chapter { chapter_id: ChapterId::from_item_keys("semantic", &["new home"]) };
    assert_eq!(session.reanchor_stale_finding(&finding_id, anchor.clone()), Ok(()));
    assert_eq!(session.findings[0].anchor, anchor);
    assert_eq!(session.findings[0].state, FindingState::NeedsVerification);
    assert_eq!(
        session.reanchor_stale_finding(&finding_id, FindingAnchor::Review),
        Err(ReviewSessionMutationError::FindingNotStale { state: FindingState::NeedsVerification })
    );
}

#[test]
fn refreshed_source_preserves_exact_findings_and_marks_unmapped_open_findings_stale() {
    let mut session = ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source(), 42);
    let file_id = FileId::from_path_lineage(None, Some(Path::new("src/lib.rs")));
    let exact_anchor = FindingAnchor::Hunk {
        hunk_id: HunkId::from_content(&file_id, "context", "exact"),
        file_id,
    };
    let exact = finding(exact_anchor.clone());
    let exact_id = exact.id.clone();
    let mut stale = finding(FindingAnchor::Review);
    stale.state = FindingState::Open;
    let stale_id = stale.id.clone();
    assert_eq!(session.add_finding(exact), Ok(()));
    assert_eq!(session.add_finding(stale), Ok(()));

    let mut refreshed_source = source();
    refreshed_source.content_digest = ReviewRevisionId::from_canonical_bytes(b"refreshed");
    session.reconcile_refreshed_source(refreshed_source.clone(), |anchor| {
        if anchor == &exact_anchor {
            FindingAnchorRefresh::Exact
        } else {
            FindingAnchorRefresh::Stale
        }
    });

    assert_eq!(session.source, refreshed_source);
    assert_eq!(
        session.findings.iter().find(|finding| finding.id == exact_id).map(|finding| finding.state),
        Some(FindingState::Draft)
    );
    assert_eq!(
        session.findings.iter().find(|finding| finding.id == stale_id).map(|finding| finding.state),
        Some(FindingState::Stale)
    );
}

#[test]
fn refreshing_source_does_not_rewrite_submitted_snapshot_revisions() {
    let mut session = ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source(), 42);
    let original_revision = session.source.content_digest.clone();
    let finding = finding(FindingAnchor::Review);
    let finding_id = finding.id.clone();
    assert_eq!(session.add_finding(finding), Ok(()));
    session
        .queue_submission(
            SubmissionId::new(),
            ReviewFeedbackMode::RequestFixes,
            &[finding_id],
            &[],
            43,
        )
        .expect("queue immutable submission");

    let mut refreshed_source = source();
    refreshed_source.content_digest = ReviewRevisionId::from_canonical_bytes(b"refreshed");
    session.reconcile_refreshed_source(refreshed_source.clone(), |_| FindingAnchorRefresh::Exact);

    assert_eq!(session.source, refreshed_source);
    assert_eq!(session.submissions[0].revision_id, original_revision);
}

#[test]
fn line_anchor_remap_requires_an_exact_or_unique_context_match() {
    let parsed = super::super::parse_unified_diff(include_str!(
        "../../../tests/fixtures/guided_review/multi_file_semantic.diff"
    ))
    .expect("fixture diff");
    let file = &parsed.files[0];
    let hunk = &file.hunks[0];
    let line =
        hunk.lines.iter().find(|line| line.new_line.is_some()).expect("new-side fixture line");
    let exact = FindingAnchor::Line {
        file_id: file.id.clone(),
        hunk_id: hunk.id.clone(),
        side: DiffSide::New,
        line: line.new_line.expect("new line number"),
        context_hash: line.context_hash(),
    };

    assert_eq!(remap_finding_anchor(&exact, &parsed.files, &[]), FindingAnchorRefresh::Exact);

    let moved = FindingAnchor::Line {
        file_id: file.id.clone(),
        hunk_id: HunkId::from_content(&file.id, "different hunk", "different content"),
        side: DiffSide::New,
        line: line.new_line.expect("new line number"),
        context_hash: line.context_hash(),
    };
    assert!(matches!(
        remap_finding_anchor(&moved, &parsed.files, &[]),
        FindingAnchorRefresh::NeedsVerification(anchor) if anchor == exact
    ));
}

#[test]
fn queued_submission_snapshots_selected_findings_at_the_pinned_revision() {
    let mut session = ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source(), 42);
    let finding = finding(FindingAnchor::Review);
    let finding_id = finding.id.clone();
    assert_eq!(session.add_finding(finding), Ok(()));

    let submission = session
        .queue_submission(
            SubmissionId::new(),
            ReviewFeedbackMode::RequestFixes,
            std::slice::from_ref(&finding_id),
            &[],
            43,
        )
        .expect("queued submission");

    assert_eq!(submission.dispatch, SubmissionDispatch::Queued);
    assert_eq!(submission.revision_id, session.source.content_digest);
    assert_eq!(submission.findings.len(), 1);
    assert_eq!(submission.findings[0].finding_id, finding_id);
    assert_eq!(session.findings[0].state, FindingState::Open);
    assert_eq!(session.submissions, vec![submission]);
    assert_eq!(
        session.queue_submission(
            SubmissionId::new(),
            ReviewFeedbackMode::RequestFixes,
            &[],
            &[],
            44,
        ),
        Err(ReviewSessionMutationError::EmptySubmission)
    );
}

#[test]
fn queued_submission_keeps_previously_open_findings_open() {
    let mut session = ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source(), 42);
    let mut finding = finding(FindingAnchor::Review);
    finding.state = FindingState::Open;
    let finding_id = finding.id.clone();
    assert_eq!(session.add_finding(finding), Ok(()));

    session
        .queue_submission(
            SubmissionId::new(),
            ReviewFeedbackMode::RequestFixes,
            &[finding_id],
            &[],
            43,
        )
        .expect("queued submission");

    assert_eq!(session.findings[0].state, FindingState::Open);
}

#[test]
fn submission_dispatch_lifecycle_allows_same_id_retry_and_queued_cancellation() {
    let mut session = ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source(), 42);
    let finding = finding(FindingAnchor::Review);
    let finding_id = finding.id.clone();
    assert_eq!(session.add_finding(finding), Ok(()));
    let submission_id = SubmissionId::new();
    session
        .queue_submission(
            submission_id.clone(),
            ReviewFeedbackMode::RequestFixes,
            std::slice::from_ref(&finding_id),
            &[],
            43,
        )
        .expect("queue submission");

    assert_eq!(session.dispatch_submission(&submission_id), Ok(()));
    assert_eq!(session.fail_submission(&submission_id), Ok(()));
    assert_eq!(session.dispatch_submission(&submission_id), Ok(()));
    assert_eq!(session.complete_submission(&submission_id), Ok(()));
    assert_eq!(session.submissions[0].dispatch, SubmissionDispatch::Completed);
    assert_eq!(
        session.dispatch_submission(&submission_id),
        Err(ReviewSessionMutationError::SubmissionNotDispatchable {
            state: SubmissionDispatch::Completed
        })
    );

    let cancelled_id = SubmissionId::new();
    session
        .queue_submission(
            cancelled_id.clone(),
            ReviewFeedbackMode::CommentOnly,
            &[finding_id],
            &[],
            44,
        )
        .expect("queue cancellable submission");
    assert_eq!(
        session.cancel_queued_submission(&cancelled_id).map(|submission| submission.id),
        Ok(cancelled_id)
    );
    assert_eq!(session.findings[0].state, FindingState::Open);
}

#[test]
fn agent_fixed_report_requires_reviewer_verification_and_retains_evidence() {
    let mut session = ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source(), 42);
    let fixed = finding(FindingAnchor::Review);
    let fixed_id = fixed.id.clone();
    let not_fixed = finding(FindingAnchor::Review);
    let not_fixed_id = not_fixed.id.clone();
    assert_eq!(session.add_finding(fixed), Ok(()));
    assert_eq!(session.add_finding(not_fixed), Ok(()));
    session
        .queue_submission(
            SubmissionId::new(),
            ReviewFeedbackMode::RequestFixes,
            &[fixed_id.clone(), not_fixed_id.clone()],
            &[],
            43,
        )
        .expect("queued submission");
    let report = RemediationReport {
        review_id: session.id.clone(),
        revision_id: session.source.content_digest.clone(),
        findings: vec![
            super::super::remediation::RemediationResult {
                finding_id: fixed_id.clone(),
                status: RemediationStatus::Fixed,
                note: Some("Added the retry cap.".into()),
            },
            super::super::remediation::RemediationResult {
                finding_id: not_fixed_id.clone(),
                status: RemediationStatus::NotFixed,
                note: None,
            },
        ],
    };

    assert_eq!(session.record_remediation_report(&report, 44), Ok(()));
    let fixed = session.findings.iter().find(|finding| finding.id == fixed_id).unwrap();
    let not_fixed = session.findings.iter().find(|finding| finding.id == not_fixed_id).unwrap();
    assert_eq!(fixed.state, FindingState::NeedsVerification);
    assert_eq!(not_fixed.state, FindingState::Open);
    assert_eq!(
        fixed.thread.last().map(|message| message.author),
        Some(FindingMessageAuthor::Agent)
    );
    assert_eq!(
        fixed.thread.last().map(|message| message.body.as_str()),
        Some("Agent reported: fixed. Added the retry cap.")
    );
}

#[test]
fn draft_updates_preserve_immutable_anchor_and_identity() {
    let mut session = ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source(), 42);
    let finding = finding(FindingAnchor::Review);
    let finding_id = finding.id.clone();
    let anchor = finding.anchor.clone();
    let thread = finding.thread.clone();
    assert_eq!(session.add_finding(finding), Ok(()));

    assert_eq!(
        session.update_draft_finding(
            &finding_id,
            FindingSeverity::Blocking,
            "Bound retry time".into(),
            "Avoid an unbounded client wait.".into(),
            Some("Return the timeout error.".into()),
        ),
        Ok(())
    );

    let updated = &session.findings[0];
    assert_eq!(updated.id, finding_id);
    assert_eq!(updated.anchor, anchor);
    assert_eq!(updated.severity, FindingSeverity::Blocking);
    assert_eq!(updated.title, "Bound retry time");
    assert_eq!(updated.thread, thread);
}

#[test]
fn finding_mutations_preserve_submitted_history() {
    let mut session = ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source(), 42);
    let draft = finding(FindingAnchor::Review);
    let draft_id = draft.id.clone();
    assert_eq!(session.add_finding(draft), Ok(()));
    assert_eq!(
        session.add_finding(session.findings[0].clone()),
        Err(ReviewSessionMutationError::DuplicateFindingId)
    );
    assert_eq!(session.delete_draft_finding(&draft_id).map(|finding| finding.id), Ok(draft_id));

    let mut open = finding(FindingAnchor::Review);
    open.state = FindingState::Open;
    let open_id = open.id.clone();
    assert_eq!(session.add_finding(open), Ok(()));
    assert_eq!(
        session.delete_draft_finding(&open_id),
        Err(ReviewSessionMutationError::FindingNotDraft { state: FindingState::Open })
    );

    let preserved = finding(FindingAnchor::Review);
    let preserved_id = preserved.id.clone();
    let submitted = SubmittedFinding {
        finding_id: preserved.id.clone(),
        label: "R1".into(),
        evidence: Default::default(),
        anchor: preserved.anchor.clone(),
        severity: preserved.severity,
        title: preserved.title.clone(),
        body: preserved.body.clone(),
        proposed_fix: preserved.proposed_fix.clone(),
        thread: Vec::new(),
        github_author: None,
    };
    assert_eq!(session.add_finding(preserved), Ok(()));
    session.submissions.push(ReviewSubmission {
        id: SubmissionId::new(),
        revision_id: session.source.content_digest.clone(),
        mode: ReviewFeedbackMode::RequestFixes,
        findings: vec![submitted],
        dispatch: SubmissionDispatch::Queued,
        created_at_ms: 43,
    });
    assert_eq!(
        session.delete_draft_finding(&preserved_id),
        Err(ReviewSessionMutationError::FindingAlreadySubmitted)
    );
}

/// A submission asks for the strongest thing any of its comments asks for.
///
/// The breaks this catches: a lone question promoted to a fix request (its turn would edit
/// code it was told to only read about), and an issue diluted to a comment by a question or a
/// note sent beside it, which leaves the reviewer's fix unasked.
#[test]
fn a_submission_asks_for_the_strongest_thing_in_it() {
    use FindingSeverity::{Blocking, Issue, Note, Question, Suggestion};
    use ReviewFeedbackMode::{CommentOnly, Question as Asks, RequestFixes};
    let cases: [(&[FindingSeverity], ReviewFeedbackMode); 8] = [
        (&[], CommentOnly),
        (&[Note], CommentOnly),
        (&[Question], Asks),
        (&[Note, Question], Asks),
        (&[Question, Issue], RequestFixes),
        (&[Issue, Question], RequestFixes),
        (&[Note, Suggestion], RequestFixes),
        (&[Blocking], RequestFixes),
    ];
    for (severities, mode) in cases {
        assert_eq!(
            ReviewFeedbackMode::asked_by(severities.iter().copied()),
            mode,
            "{severities:?}"
        );
    }
}

/// Unsent means written or open and never posted: resolved, stale, needing verification, or
/// already on GitHub are all accounted for elsewhere.
#[test]
fn only_written_and_open_comments_never_posted_are_unsent() {
    let mut session = ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source(), 42);
    let state = |state: FindingState| ReviewFinding { state, ..finding(FindingAnchor::Review) };
    let posted = ReviewFinding {
        posted: Some(PostedComment { review_id: 7, url: "https://example.test/7".into() }),
        ..state(FindingState::Open)
    };
    session.findings = vec![
        state(FindingState::Draft),
        state(FindingState::Open),
        state(FindingState::Resolved),
        state(FindingState::Stale),
        state(FindingState::NeedsVerification),
        posted,
    ];
    let expected: Vec<FindingId> =
        session.findings[..2].iter().map(|finding| finding.id.clone()).collect();
    assert_eq!(session.unsent_finding_ids(), expected);
}
