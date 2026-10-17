//! Edits — what a front end may ask of a review's feedback session, and how each is applied.

use super::*;

/// One change a front end asks of the open review's feedback session.
///
/// Sent as `Action::EditReview` with the review it is for and a `request_id`, and answered once:
/// `review_session_saved` with the stored session, even when the edit changed nothing, or
/// `review_session_save_failed` saying why it was refused.
///
/// Published on the RPC wire as `edit_review`'s `edit`, tagged by `edit`. Every field a client may
/// leave out defaults, and adopting carried comments is not on it: that is the interface handing a
/// review's comments to the session it starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "edit", rename_all = "snake_case")]
pub enum ReviewEdit {
    /// A new comment. With `send`, it goes to the agent in the same save.
    Comment {
        /// What was written, and where
        #[serde(flatten)]
        draft: CommentDraft,
        /// Send it to the agent at once, as `a` and a thread reply do
        #[serde(default)]
        send: bool,
    },
    /// A reply on a comment's thread, which reopens it if it was resolved.
    Reply {
        /// The comment replied to
        finding_id: FindingId,
        /// The reply
        body: String,
        /// Send the comment, reply and all, to the agent at once
        #[serde(default)]
        send: bool,
    },
    /// Direction on a question whose answer says the code should change: the question becomes an
    /// issue, so sending it asks for the fix with the whole thread.
    AskForFix {
        /// The question
        finding_id: FindingId,
        /// What to do
        body: String,
        /// Send it to the agent at once
        #[serde(default)]
        send: bool,
    },
    /// New wording for a comment not sent yet.
    EditComment {
        /// The comment, still a draft
        finding_id: FindingId,
        /// How much it matters
        severity: FindingSeverity,
        /// Its summary
        title: String,
        /// Its text
        body: String,
        /// A fix it suggests
        #[serde(default)]
        proposed_fix: Option<String>,
    },
    /// Throws away a comment not sent yet.
    DeleteComment {
        /// The comment, still a draft
        finding_id: FindingId,
    },
    /// Marks a comment resolved.
    Resolve {
        /// The comment
        finding_id: FindingId,
    },
    /// Opens a resolved comment again.
    Reopen {
        /// The comment
        finding_id: FindingId,
    },
    /// Points a comment whose code a refresh moved out from under it at a line that exists.
    Reanchor {
        /// The stale comment
        finding_id: FindingId,
        /// Where it belongs now, in the review as it stands
        anchor: FindingAnchor,
    },
    /// Records how the review ended. The agent is not told.
    SetOutcome {
        /// The verdict
        outcome: ReviewOutcome,
    },
    /// Sends comments to the agent as one submission, at the next turn boundary.
    Send {
        /// The comments that travel
        finding_ids: Vec<FindingId>,
        /// What it asks for; absent means what the strongest comment asks for
        #[serde(default)]
        mode: Option<ReviewFeedbackMode>,
        /// A verdict recorded in the same save, as finishing a review does
        #[serde(default)]
        outcome: Option<ReviewOutcome>,
    },
    /// Withdraws a submission the agent has not started yet.
    CancelSend {
        /// The queued submission
        submission_id: SubmissionId,
    },
    /// Sends a submission again, under the same id, after its turn failed or was cut off.
    RetrySend {
        /// The submission to retry
        submission_id: SubmissionId,
    },
    /// Adopts comments written while reading a pull request, now that it is checked out here,
    /// moving each onto this review's code. With `send`, the ones still unsent go to the agent.
    #[serde(skip)]
    AdoptCarried {
        /// The comments, as the other review left them
        findings: Vec<ReviewFinding>,
        /// Send them to the agent once adopted
        send: bool,
    },
}

/// A comment as written, before the desk gives it an id and a label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentDraft {
    /// Where it is, in the review as it stands. A line's `context_hash` may be left out.
    pub anchor: FindingAnchor,
    /// How much it matters; a question asks and forbids edits. An issue when left out.
    #[serde(default = "issue")]
    pub severity: FindingSeverity,
    /// Its summary; its opening line when left empty
    #[serde(default)]
    pub title: String,
    /// Its text
    pub body: String,
    /// A fix it suggests
    #[serde(default)]
    pub proposed_fix: Option<String>,
}

fn issue() -> FindingSeverity {
    FindingSeverity::Issue
}

/// Said when a comment's place is not in the review as it stands.
const ANCHOR_NOT_IN_REVIEW: &str =
    "That line is not in the review as it stands now. Refresh it, or comment on what is there.";

/// Applies `edit` to `candidate`, answering what the engine has to do once it is stored, or why it
/// was refused, as a sentence.
pub(super) fn apply(
    edit: ReviewEdit,
    candidate: &mut ReviewSession,
    review: &GuidedReview,
) -> Result<Option<FollowUp>, String> {
    let now = now_ms();
    let send_one = |candidate: &mut ReviewSession, finding_id: FindingId| {
        let severity = candidate
            .findings
            .iter()
            .find(|finding| finding.id == finding_id)
            .map(|finding| finding.severity)
            .ok_or_else(|| mutation_refusal(&ReviewSessionMutationError::FindingNotFound))?;
        let mode = ReviewFeedbackMode::asked_by([severity]);
        queue(candidate, review, mode, &[finding_id]).map(Some)
    };
    match edit {
        ReviewEdit::Comment { draft, send } => {
            let finding_id = apply_comment(draft, candidate, review)?;
            if send {
                return send_one(candidate, finding_id);
            }
            Ok(None)
        }
        ReviewEdit::Reply { finding_id, body, send } => {
            candidate
                .append_reviewer_message(&finding_id, body, now)
                .map_err(|error| mutation_refusal(&error))?;
            if send {
                return send_one(candidate, finding_id);
            }
            Ok(None)
        }
        ReviewEdit::AskForFix { finding_id, body, send } => {
            candidate
                .ask_for_fix(&finding_id, body, now)
                .map_err(|error| mutation_refusal(&error))?;
            if send {
                return send_one(candidate, finding_id);
            }
            Ok(None)
        }
        ReviewEdit::EditComment { finding_id, severity, title, body, proposed_fix } => candidate
            .update_draft_finding(&finding_id, severity, title, body, proposed_fix)
            .map(|()| None)
            .map_err(|error| mutation_refusal(&error)),
        ReviewEdit::DeleteComment { finding_id } => candidate
            .delete_draft_finding(&finding_id)
            .map(|_| None)
            .map_err(|error| mutation_refusal(&error)),
        ReviewEdit::Resolve { finding_id } => {
            candidate.resolve_finding(&finding_id).map(|()| None).map_err(|e| mutation_refusal(&e))
        }
        ReviewEdit::Reopen { finding_id } => {
            candidate.reopen_finding(&finding_id).map(|()| None).map_err(|e| mutation_refusal(&e))
        }
        ReviewEdit::Reanchor { finding_id, anchor } => {
            let anchor = in_review(anchor, review)?;
            candidate
                .reanchor_stale_finding(&finding_id, anchor)
                .map(|()| None)
                .map_err(|error| mutation_refusal(&error))
        }
        ReviewEdit::SetOutcome { outcome } => {
            candidate.set_outcome(outcome);
            Ok(None)
        }
        ReviewEdit::Send { finding_ids, mode, outcome } => {
            apply_send(candidate, review, &finding_ids, mode, outcome).map(Some)
        }
        ReviewEdit::CancelSend { submission_id } => candidate
            .cancel_queued_submission(&submission_id)
            .map(|_| Some(FollowUp::Cancel(submission_id)))
            .map_err(|error| mutation_refusal(&error)),
        ReviewEdit::RetrySend { submission_id } => candidate
            .dispatch_submission(&submission_id)
            .map(|()| Some(FollowUp::Submit(submission_id)))
            .map_err(|error| mutation_refusal(&error)),
        ReviewEdit::AdoptCarried { findings, send } => {
            apply_carried(candidate, review, findings, send)
        }
    }
}

/// Adds a new comment as a draft, its anchor checked against the review, answering its id.
fn apply_comment(
    draft: CommentDraft,
    candidate: &mut ReviewSession,
    review: &GuidedReview,
) -> Result<FindingId, String> {
    let anchor = in_review(draft.anchor, review)?;
    let title = match draft.title.trim() {
        "" => crate::guided_review::title_from_body(&draft.body),
        _ => draft.title,
    };
    let finding = ReviewFinding {
        id: FindingId::new(),
        // The session gives it the next `R<n>`
        label: String::new(),
        anchor,
        severity: draft.severity,
        title,
        body: draft.body,
        proposed_fix: draft.proposed_fix,
        state: FindingState::Draft,
        thread: Vec::new(),
        posted: None,
        github_thread: None,
    };
    let finding_id = finding.id.clone();
    candidate.add_finding(finding).map_err(|error| mutation_refusal(&error))?;
    Ok(finding_id)
}

/// Queues the chosen comments, asking for what the strongest of them asks for unless the edit
/// named a mode, and records the verdict that went with them.
fn apply_send(
    candidate: &mut ReviewSession,
    review: &GuidedReview,
    finding_ids: &[FindingId],
    mode: Option<ReviewFeedbackMode>,
    outcome: Option<ReviewOutcome>,
) -> Result<FollowUp, String> {
    let mode = mode.unwrap_or_else(|| {
        ReviewFeedbackMode::asked_by(
            candidate
                .findings
                .iter()
                .filter(|finding| finding_ids.contains(&finding.id))
                .map(|finding| finding.severity),
        )
    });
    if let Some(outcome) = outcome {
        candidate.set_outcome(outcome);
    }
    queue(candidate, review, mode, finding_ids)
}

/// Adopts comments carried from elsewhere, and with `send` queues the ones still placed.
///
/// A comment whose code is gone came back stale, and waits for the reader to say what it means
/// now.
fn apply_carried(
    candidate: &mut ReviewSession,
    review: &GuidedReview,
    findings: Vec<ReviewFinding>,
    send: bool,
) -> Result<Option<FollowUp>, String> {
    let carried: Vec<FindingId> = findings.iter().map(|finding| finding.id.clone()).collect();
    candidate.adopt_carried_findings(findings, |anchor| {
        remap_finding_anchor(anchor, &review.files, &review.chapters)
    });
    if !send {
        return Ok(None);
    }
    let sendable: Vec<FindingId> = candidate
        .unsent_finding_ids()
        .into_iter()
        .filter(|finding_id| carried.contains(finding_id))
        .collect();
    if sendable.is_empty() {
        return Ok(None);
    }
    let mode = ReviewFeedbackMode::asked_by(
        candidate
            .findings
            .iter()
            .filter(|finding| sendable.contains(&finding.id))
            .map(|finding| finding.severity),
    );
    queue(candidate, review, mode, &sendable).map(Some)
}

/// Queues `finding_ids` as one new submission, its evidence resolved against `review` now: a later
/// refresh makes the ids unresolvable.
fn queue(
    candidate: &mut ReviewSession,
    review: &GuidedReview,
    mode: ReviewFeedbackMode,
    finding_ids: &[FindingId],
) -> Result<FollowUp, String> {
    let submission_id = SubmissionId::new();
    candidate
        .queue_submission(submission_id.clone(), mode, finding_ids, &review.files, now_ms())
        .map(|_| FollowUp::Submit(submission_id))
        .map_err(|error| mutation_refusal(&error))
}

/// The engine action that queues a stored submission's turn, or nothing when it cannot go.
///
/// It must still be dispatchable and belong to the revision it was written against: findings
/// anchored to source the agent would no longer recognize are worse to act on than not at all.
pub(super) fn submit(session: &ReviewSession, submission_id: &SubmissionId) -> Option<Effect> {
    let submission = session
        .submissions
        .iter()
        .find(|submission| &submission.id == submission_id)
        .filter(|submission| submission.revision_id == session.source.content_digest)
        .filter(|submission| {
            matches!(
                submission.dispatch,
                SubmissionDispatch::Queued | SubmissionDispatch::Dispatched
            )
        })?;
    Some(Effect::Engine(Box::new(Action::SubmitReviewFeedback {
        submission_id: submission.id.clone(),
        review_id: session.id.clone(),
        source: session.source.clone(),
        mode: submission.mode,
        findings: submission.findings.clone(),
    })))
}

/// The anchor as it should be stored, or a refusal when it does not resolve exactly against the
/// review as it stands.
///
/// A line named without its context digest, which no client can be expected to compute, gets the
/// one the pinned review has for that line.
fn in_review(mut anchor: FindingAnchor, review: &GuidedReview) -> Result<FindingAnchor, String> {
    if let FindingAnchor::Line { file_id, hunk_id, side, line, context_hash } = &mut anchor {
        if context_hash.trim().is_empty() {
            let found = review
                .files
                .iter()
                .find(|file| &file.id == file_id)
                .and_then(|file| file.hunks.iter().find(|hunk| &hunk.id == hunk_id))
                .and_then(|hunk| {
                    hunk.lines.iter().find(|candidate| {
                        let number = match side {
                            DiffSide::Old => candidate.old_line,
                            DiffSide::New => candidate.new_line,
                        };
                        number == Some(*line)
                    })
                })
                .ok_or_else(|| ANCHOR_NOT_IN_REVIEW.to_string())?;
            *context_hash = found.context_hash();
        }
    }
    match remap_finding_anchor(&anchor, &review.files, &review.chapters) {
        FindingAnchorRefresh::Exact => Ok(anchor),
        _ => Err(ANCHOR_NOT_IN_REVIEW.into()),
    }
}

/// Why a change to the session was refused, as a sentence for the reviewer.
fn mutation_refusal(error: &ReviewSessionMutationError) -> String {
    use ReviewSessionMutationError as Refused;
    use ReviewSessionValidationError as Invalid;
    match error {
        Refused::FindingNotFound => "That comment is no longer in the review.".into(),
        Refused::EmptyReply => "Write the reply before saving it.".into(),
        Refused::DuplicateFindingId => "That comment is already in the review.".into(),
        Refused::FindingNotDraft { .. } | Refused::FindingAlreadySubmitted => {
            "That comment has been sent, so it can no longer be changed or deleted; reply to it \
             instead."
                .into()
        }
        Refused::FindingNotStale { .. } => {
            "Only a comment whose code moved away can be moved.".into()
        }
        Refused::EmptySubmission => "Nothing is selected to send.".into(),
        Refused::DuplicateSubmissionId => "That feedback was already queued.".into(),
        Refused::SubmissionNotFound => "That feedback is no longer in the review.".into(),
        Refused::SubmissionNotQueued { .. } => {
            "That feedback has already reached the agent, so it cannot be withdrawn.".into()
        }
        Refused::SubmissionNotDispatchable { .. } | Refused::SubmissionNotDispatched { .. } => {
            "That feedback is not waiting to be sent.".into()
        }
        Refused::RemediationReportWrongReview | Refused::RemediationReportNotSubmitted => {
            "That report is not about this review.".into()
        }
        Refused::Validation(Invalid::EmptyFindingBody | Invalid::EmptyFindingTitle) => {
            "Write the comment before saving it.".into()
        }
        Refused::Validation(Invalid::InvalidLineAnchor | Invalid::MissingContextHash) => {
            ANCHOR_NOT_IN_REVIEW.into()
        }
        Refused::Validation(Invalid::UnsupportedSchema { .. } | Invalid::DuplicateId { .. }) => {
            "This change cannot be saved as written.".into()
        }
    }
}
